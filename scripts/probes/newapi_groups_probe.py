#!/usr/bin/env python3
"""固定NewAPI版本的组配置隔离探针；不读取运行配置/数据库或真实凭据。"""
import argparse
import hashlib
import json
import os
from pathlib import Path
import secrets
import socket
import subprocess
import tempfile
import time

from newapi_shared_source_probe import Client
from newapi_token_probe import SHA256


def run(binary):
    with binary.open("rb") as handle:
        assert hashlib.file_digest(handle, "sha256").hexdigest() == SHA256
    work = Path(tempfile.mkdtemp(prefix="quota-throttle-sgm5-c-native-"))
    with socket.socket() as reserve:
        reserve.bind(("127.0.0.1", 0))
        port = reserve.getsockname()[1]
    assert port not in (3000, 3001)
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
                listener = subprocess.check_output(["ss", "-H", "-ltnp", f"sport = :{port}"], text=True)
                if f"pid={process.pid}," in listener and (work / "test.db").is_file():
                    break
                time.sleep(0.1)
            else:
                raise RuntimeError("cannot prove own isolated listener and database")
            password = secrets.token_hex(9)
            root.api("POST", "/api/setup", {"username": "qtaudit", "password": password,
                                            "confirmPassword": password, "SelfUseModeEnabled": True})
            root.user = root.api("POST", "/api/user/login", {"username": "qtaudit", "password": password})["id"]

            def options():
                return {row["key"]: row["value"] for row in root.api("GET", "/api/option/")}

            def put(key, value):
                root.api("PUT", "/api/option/", {"key": key, "value": json.dumps(value)})

            old = options()
            alias = "group_ratio_setting.group_ratio"
            special_key = "group_ratio_setting.group_special_usable_group"
            assert {"GroupRatio", "GroupGroupRatio", "UserUsableGroups", alias, special_key} <= old.keys()
            base = json.loads(old["GroupRatio"])
            merged = {**base, "sgm-c-test": 1.75}
            put("GroupRatio", merged)
            after = options()
            assert json.loads(after["GroupRatio"]) == merged
            assert "sgm-c-test" in root.api("GET", "/api/group/")
            report["ratio_alias_tracks_legacy_put"] = json.loads(after[alias]) == merged
            report["checks"].append("legacy_ratio_write_and_group_readback")
            # 仅新临时实例赠测试额度；实现不改账号余额或赠额。
            root.api("PUT", "/api/option/", {"key": "QuotaForNewUser", "value": "100000"})
            signer_password = secrets.token_hex(9)
            root.api("POST", "/api/user/", {"username": "qtsigner", "password": signer_password,
                                            "display_name": "isolated signer", "role": 1})
            issuer = Client(root.base)
            issuer.user = issuer.api("POST", "/api/user/login", {"username": "qtsigner", "password": signer_password})["id"]
            profile = issuer.api("GET", "/api/user/self")
            account_group = profile["group"]
            assert "sgm-c-test" not in issuer.api("GET", "/api/user/self/groups")
            special = json.loads(after[special_key])
            scoped = {**special, account_group: {**special.get(account_group, {}), "sgm-c-test": "isolated"}}
            put(special_key, scoped)
            checked = options()
            assert json.loads(checked[special_key]) == scoped
            assert checked["UserUsableGroups"] == old["UserUsableGroups"]
            assert checked["GroupGroupRatio"] == old["GroupGroupRatio"]
            groups = issuer.api("GET", "/api/user/self/groups")
            assert groups["sgm-c-test"]["ratio"] == 1.75
            assert groups["sgm-c-test"]["desc"] == "isolated"
            report["checks"].append("scoped_permission_without_global_usable_or_ratio_override_changes")
            denied = {**scoped, account_group: {**scoped[account_group], "-:sgm-c-test": "denied"}}
            put(special_key, denied)
            # 正负同名条目由Go map迭代处理，不能把这种冲突作为可靠授权。
            report["checks"].append("conflicting_scoped_entries_are_representable")
            put(special_key, scoped)
            assert "sgm-c-test" in issuer.api("GET", "/api/user/self/groups")
            put(alias, {**merged, "alias-only": 2.5})
            final = options()
            assert "alias-only" in root.api("GET", "/api/group/")
            report["legacy_tracks_namespaced_put"] = "alias-only" in json.loads(final["GroupRatio"])
            report["checks"].append("namespaced_ratio_write_and_legacy_alias_observation")
        finally:
            if process is not None and process.poll() is None:
                process.terminate()
                try:
                    process.wait(timeout=10)
                except subprocess.TimeoutExpired:
                    process.kill()
                    process.wait(timeout=5)
    return report


if __name__ == "__main__":
    parser = argparse.ArgumentParser()
    parser.add_argument("--binary", required=True, type=Path)
    args = parser.parse_args()
    print(json.dumps(run(args.binary.resolve()), ensure_ascii=False, indent=2))
