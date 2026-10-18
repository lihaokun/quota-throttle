#!/usr/bin/env python3
"""SGM-5令牌接口隔离实证；只读指定二进制，不读取现行配置或数据库。"""
import argparse
import hashlib
import json
import os
from pathlib import Path
import secrets
import socket
import subprocess
import tempfile
import threading
import time

from newapi_shared_source_probe import Client, MockUpstream, MODEL, items, ThreadingHTTPServer

SHA256 = "90c333eb7ace489d46e25cb687767c25c3e28584ce80384f111c03ff5f5b1205"


def run(binary):
    with binary.open("rb") as handle:
        assert hashlib.file_digest(handle, "sha256").hexdigest() == SHA256
    work = Path(tempfile.mkdtemp(prefix="quota-throttle-sgm5-native-"))
    with socket.socket() as reserve:
        reserve.bind(("127.0.0.1", 0))
        port = reserve.getsockname()[1]
    assert port not in (3000, 3001)
    upstream = ThreadingHTTPServer(("127.0.0.1", 0), MockUpstream)
    threading.Thread(target=upstream.serve_forever, daemon=True).start()
    env = {"PATH": os.defpath, "PORT": str(port), "GIN_MODE": "release", "GOMAXPROCS": "2",
           "SQLITE_PATH": str(work / "test.db"), "SQL_DSN": "local", "LOG_SQL_DSN": "local",
           "SESSION_SECRET": secrets.token_hex(24), "GLOBAL_API_RATE_LIMIT_ENABLE": "false",
           "CRITICAL_RATE_LIMIT_ENABLE": "false", "SEARCH_RATE_LIMIT_ENABLE": "false"}
    report = {"directory": str(work), "binary_sha256": SHA256, "checks": []}
    process = None
    with (work / "server.log").open("wb") as log:
        try:
            process = subprocess.Popen([str(binary)], cwd=work, env=env, stdin=subprocess.DEVNULL,
                                       stdout=log, stderr=log, start_new_session=True)
            root = Client(f"http://127.0.0.1:{port}")
            deadline = time.monotonic() + 30
            while time.monotonic() < deadline:
                assert process.poll() is None, "own test process exited"
                listening = subprocess.check_output(["ss", "-H", "-ltnp", f"sport = :{port}"], text=True)
                if f"pid={process.pid}," in listening and (work / "test.db").is_file():
                    break
                time.sleep(0.1)
            else:
                raise RuntimeError("cannot prove own isolated listener and database")
            password = secrets.token_hex(9)
            root.api("POST", "/api/setup", {"username": "qtaudit", "password": password,
                                            "confirmPassword": password, "SelfUseModeEnabled": True})
            root.user = root.api("POST", "/api/user/login", {"username": "qtaudit", "password": password})["id"]
            # 只在新测试实例设置新用户赠额；生产实现不修改余额或默认赠额。
            for key, value in {"QuotaForNewUser": "10000000", "RetryTimes": "1",
                               "GroupRatio": json.dumps({"default": 1, "sgm-a": 1, "sgm-b": 1}),
                               "UserUsableGroups": json.dumps({"default": "default", "sgm-a": "A", "sgm-b": "B"})}.items():
                root.api("PUT", "/api/option/", {"key": key, "value": value})
            ordinary_password = secrets.token_hex(9)
            root.api("POST", "/api/user/", {"username": "qtsigner", "password": ordinary_password,
                                            "display_name": "test signer", "role": 1})
            issuer = Client(root.base)
            issuer.user = issuer.api("POST", "/api/user/login", {"username": "qtsigner", "password": ordinary_password})["id"]
            profile = issuer.api("GET", "/api/user/self")
            assert profile["role"] == 1 and profile["status"] == 1 and profile["quota"] > 0
            assert {"sgm-a", "sgm-b"} <= set(issuer.api("GET", "/api/user/self/groups"))
            report["checks"].append("ordinary_owner_and_available_groups")
            owner_access = issuer.api("GET", "/api/user/token")
            assert isinstance(owner_access, str) and len(owner_access) >= 16
            bearer = Client(root.base)
            bearer.user = issuer.user
            auth = {"Authorization": "Bearer " + owner_access}
            status, body = bearer.request("GET", "/api/user/self", headers=auth)
            assert status == 200 and body["success"] and body["data"]["id"] == issuer.user
            bearer.user = root.user
            assert bearer.request("GET", "/api/user/self", headers=auth)[0] == 401
            report["checks"].append("bearer_owner_header_must_match_actual_account")
            status, body = issuer.request("GET", "/api/channel/?p=1&page_size=10")
            assert status >= 400 or body.get("success") is False
            report["checks"].append("ordinary_identity_cannot_manage_channels")
            channels = {}
            for group, source in [("sgm-a", "A"), ("sgm-b", "B")]:
                root.api("POST", "/api/channel/", {"mode": "single", "channel": {
                    "name": source, "type": 8, "key": "fake-upstream", "models": MODEL,
                    "base_url": f"http://127.0.0.1:{upstream.server_port}/{source}/chat/completions",
                    "group": group, "priority": 100, "weight": 1, "status": 1, "auto_ban": 0}})
            for row in items(root.api("GET", "/api/channel/?p=1&page_size=100")):
                channels[row["name"]] = row["id"]
            marker = "qt-create-0123456789abcdef"
            draft = {"name": marker, "group": "sgm-a", "status": 2, "expired_time": -1,
                     "unlimited_quota": True, "remain_quota": 0, "cross_group_retry": False,
                     "model_limits_enabled": False, "model_limits": "", "allow_ips": ""}
            issuer_id = issuer.user
            try:
                issuer.user = root.user
                assert issuer.request("POST", "/api/token/", draft)[0] == 401
            finally:
                issuer.user = issuer_id
            assert issuer.api("GET", "/api/token/?p=1&page_size=100")["total"] == 0
            assert root.api("GET", "/api/token/?p=1&page_size=100")["total"] == 0
            report["checks"].append("cookie_owner_header_mismatch_rejected_before_creation")
            status, created = issuer.request("POST", "/api/token/", draft)
            assert status == 200 and created["success"] is True and "data" not in created
            found = issuer.api("GET", f"/api/token/search?keyword={marker}&p=1&page_size=2")
            assert found["total"] == 1 and len(found["items"]) == 1
            token = found["items"][0]
            token_id = token["id"]
            assert token["user_id"] == issuer.user and token["name"] == marker
            assert token["status"] == 1, "native create ignores supplied disabled status"
            assert "*" in token["key"] and token["group"] == "sgm-a" and not token["cross_group_retry"]
            report["checks"].extend(["create_ack_has_no_id", "exact_marker_search", "create_ignores_disabled_status", "list_masks_key"])
            all_page = issuer.api("GET", "/api/token/?p=1&page_size=100")
            assert all_page["page"] == 1 and all_page["page_size"] == 100 and all_page["total"] == 1
            assert len(all_page["items"]) == 1
            detail = issuer.api("GET", f"/api/token/{token_id}")
            assert detail == token
            status, foreign = root.request("GET", f"/api/token/{token_id}")
            assert status == 200 and foreign["success"] is False
            report["not_found_message"] = foreign["message"]
            assert foreign["message"] == "record not found"
            assert root.api("GET", "/api/token/?p=1&page_size=100")["total"] == 0
            report["checks"].append("token_endpoints_owner_scoped_and_missing_is_200_false")
            secret = issuer.api("POST", f"/api/token/{token_id}/key", {})["key"]
            assert secret.isascii() and secret.isalnum() and len(secret) >= 16
            assert not secret.startswith("sk-") and "*" not in secret
            report["checks"].append("explicit_reveal_returns_bare_key")

            def inference(key, claude=False, extra=None):
                payload = {"model": MODEL, "max_tokens": 16, "stream": False,
                           "messages": [{"role": "user", "content": "isolated"}]}
                payload.update(extra or {})
                headers = {"x-api-key": "sk-" + key, "anthropic-version": "2023-06-01"} if claude else {"Authorization": "Bearer sk-" + key}
                return Client(root.base).request("POST", "/v1/messages" if claude else "/v1/chat/completions", payload, headers)

            for claude in (False, True):
                before = len(MockUpstream.hits)
                status, _ = inference(secret, claude, {"group": "sgm-b"})
                assert status == 200 and MockUpstream.hits[before:] == ["A"]
                before = len(MockUpstream.hits)
                status, _ = inference(secret + "-" + str(channels["B"]), claude)
                assert status == 403 and len(MockUpstream.hits) == before
            report["checks"].append("same_key_both_protocols_reject_channel_override_and_ignore_payload_group")
            root.api("POST", "/api/channel/status/batch", {"ids": [channels["A"]], "status": 2})
            assert root.api("GET", f"/api/channel/{channels['A']}")["status"] == 2
            before = len(MockUpstream.hits)
            assert inference(secret)[0] != 200 and len(MockUpstream.hits) == before
            report["checks"].append("unavailable_group_does_not_fall_through_other_group")
            issuer.api("PUT", "/api/token/?status_only=true", {"id": token_id, "status": 2})
            stopped = issuer.api("GET", f"/api/token/{token_id}")
            assert stopped["status"] == 2
            for field in ("group", "cross_group_retry", "expired_time", "unlimited_quota", "model_limits", "model_limits_enabled", "allow_ips", "name"):
                assert stopped[field] == detail[field], field
            issuer.api("PUT", "/api/token/?status_only=true", {"id": token_id, "status": 1})
            assert issuer.api("GET", f"/api/token/{token_id}")["status"] == 1
            report["checks"].append("status_only_preserves_group_and_limits")
            # 同名并不唯一；不得任意认领最新项。
            issuer.api("POST", "/api/token/", draft)
            duplicate = issuer.api("GET", f"/api/token/search?keyword={marker}&p=1&page_size=2")
            assert duplicate["total"] == 2 and len(duplicate["items"]) == 2
            report["checks"].append("duplicate_marker_requires_ambiguity_not_latest_item")
            assert issuer.api("POST", "/api/token/batch", {"ids": [token_id]}) == 1
            assert issuer.api("POST", "/api/token/batch", {"ids": [token_id]}) == 0
            status, absent = issuer.request("GET", f"/api/token/{token_id}")
            assert status == 200 and absent["success"] is False and absent["message"] == "record not found"
            report["checks"].append("single_id_batch_delete_is_idempotent_and_readback_absent")
            report["success"] = True
        except Exception as exc:
            report["success"] = False
            report["error"] = str(exc)
        finally:
            if process is not None and process.poll() is None:
                process.terminate()
                try:
                    process.wait(timeout=5)
                except subprocess.TimeoutExpired:
                    process.kill()
                    process.wait(timeout=5)
            upstream.shutdown()
            upstream.server_close()
            report["isolated_process_stopped"] = process is None or process.poll() is not None
    print(json.dumps(report, ensure_ascii=False, indent=2))
    return 0 if report.get("success") else 1


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", required=True, type=Path)
    args = parser.parse_args()
    raise SystemExit(run(args.binary.resolve(strict=True)))
