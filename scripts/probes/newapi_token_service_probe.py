#!/usr/bin/env python3
"""在全新原生实例运行Rust A+B+C组合；只关闭自己创建的进程。"""
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

from newapi_shared_source_probe import Client, MockUpstream, MODEL, ThreadingHTTPServer, items
from newapi_token_probe import SHA256

TEST = "reconcile::worker::tests::token_service_tests::native::t5c_native_token_service_combination"


def run(binary, test_binary):
    with binary.open("rb") as handle:
        assert hashlib.file_digest(handle, "sha256").hexdigest() == SHA256
    assert test_binary.is_file() and os.access(test_binary, os.X_OK)
    work = Path(tempfile.mkdtemp(prefix="quota-throttle-sgm5-c-combination-"))
    with socket.socket() as reserve:
        reserve.bind(("127.0.0.1", 0))
        port = reserve.getsockname()[1]
    assert port not in (3000, 3001)
    upstream = ThreadingHTTPServer(("127.0.0.1", 0), MockUpstream)
    assert upstream.server_port not in (3000, 3001)
    MockUpstream.hits.clear()
    MockUpstream.failures.clear()
    threading.Thread(target=upstream.serve_forever, daemon=True).start()
    env = {"PATH": os.defpath, "PORT": str(port), "GIN_MODE": "release", "GOMAXPROCS": "2",
           "SQLITE_PATH": str(work / "native.sqlite3"), "SQL_DSN": "local", "LOG_SQL_DSN": "local",
           "SESSION_SECRET": secrets.token_hex(24), "GLOBAL_API_RATE_LIMIT_ENABLE": "false",
           "CRITICAL_RATE_LIMIT_ENABLE": "false", "SEARCH_RATE_LIMIT_ENABLE": "false"}
    report = {"directory": str(work), "binary_sha256": SHA256, "success": False}
    process = None
    with (work / "server.log").open("wb") as log:
        try:
            process = subprocess.Popen([str(binary)], cwd=work, env=env, stdin=subprocess.DEVNULL,
                                       stdout=log, stderr=log, start_new_session=True)
            deadline = time.monotonic() + 30
            while time.monotonic() < deadline:
                assert process.poll() is None, "own test process exited"
                listener = subprocess.check_output(["ss", "-H", "-ltnp", f"sport = :{port}"], text=True)
                if f"pid={process.pid}," in listener and (work / "native.sqlite3").is_file():
                    break
                time.sleep(0.1)
            else:
                raise RuntimeError("cannot prove isolated listener/database")
            (work / "new-api.pid").write_text(str(process.pid))
            root = Client(f"http://127.0.0.1:{port}")
            password = secrets.token_hex(9)
            root.api("POST", "/api/setup", {"username": "qtaudit", "password": password,
                                            "confirmPassword": password, "SelfUseModeEnabled": True})
            root.user = root.api("POST", "/api/user/login", {"username": "qtaudit", "password": password})["id"]
            initial = {row["key"]: row["value"] for row in root.api("GET", "/api/option/")}
            alias = "group_ratio_setting.group_ratio"
            assert json.loads(initial["GroupRatio"]) == json.loads(initial[alias])
            ratio = {**json.loads(initial["GroupRatio"]), "outside": 1.25}
            for key, value in {"QuotaForNewUser": "10000000", "RetryTimes": "1",
                               "AutomaticEnableChannelEnabled": "false", "GroupRatio": json.dumps(ratio),
                               alias: json.dumps(ratio)}.items():
                root.api("PUT", "/api/option/", {"key": key, "value": value})
            ordinary_password = secrets.token_hex(9)
            root.api("POST", "/api/user/", {"username": "qtsigner", "password": ordinary_password,
                                            "display_name": "isolated signer", "role": 1})
            issuer = Client(root.base)
            issuer.user = issuer.api("POST", "/api/user/login", {"username": "qtsigner", "password": ordinary_password})["id"]
            for name, group, source, status in [("native-A", "sgm-native", "A", 2), ("native-B", "outside", "B", 1)]:
                root.api("POST", "/api/channel/", {"mode": "single", "channel": {
                    "name": name, "type": 8, "key": "fake-upstream", "models": MODEL,
                    "base_url": f"http://127.0.0.1:{upstream.server_port}/{source}/chat/completions",
                    "group": group, "priority": 100, "weight": 1, "status": status, "auto_ban": 1}})
            channels = {row["name"]: row["id"] for row in items(root.api("GET", "/api/channel/?p=1&page_size=100"))}
            assert "sgm-native" not in root.api("GET", "/api/group/")
            spec = {"base_url": root.base, "port": port, "binary": str(binary),
                    "root_id": root.user, "issuer_id": issuer.user,
                    "root_password": password, "issuer_password": ordinary_password,
                    "channel_id": channels["native-A"], "outside_channel_id": channels["native-B"],
                    "upstream_url": f"http://127.0.0.1:{upstream.server_port}/A/chat/completions", "model": MODEL}
            spec_path = work / "input.json"
            with spec_path.open("x") as handle:
                os.chmod(spec_path, 0o600)
                json.dump(spec, handle)
            test_env = {"PATH": os.environ.get("PATH", os.defpath), "SGM5_C_NATIVE_SPEC": str(spec_path),
                        "RUST_BACKTRACE": "0"}
            with (work / "rust.log").open("wb") as rust_log:
                result = subprocess.run([str(test_binary), TEST, "--ignored", "--exact", "--nocapture"],
                                        cwd=Path(__file__).resolve().parents[2], env=test_env,
                                        stdin=subprocess.DEVNULL, stdout=rust_log, stderr=subprocess.STDOUT,
                                        timeout=360)
            output = (work / "rust.log").read_text()
            assert result.returncode == 0, "Rust native combination failed; see private rust.log"
            assert "1 passed; 0 failed; 0 ignored" in output and "NATIVE_TOKEN_SERVICE_OK" in output
            assert MockUpstream.hits == ["A", "A"], "inference escaped target group or extra inference occurred"
            report["success"] = True
            report["checks"] = ["real_local_preflight", "group_registration_from_disabled_member",
                                "native_create_persistent_lookup", "same_key_both_protocols",
                                "ordinary_key_channel_suffix_rejected", "whole_group_outage_does_not_cross",
                                "native_disable_enable_delete_readback", "history_and_reference_preserved"]
        except Exception as exc:
            report["error"] = str(exc)
        finally:
            if process is not None and process.poll() is None:
                process.terminate()
                try:
                    process.wait(timeout=10)
                except subprocess.TimeoutExpired:
                    process.kill()
                    process.wait(timeout=5)
            upstream.shutdown()
            upstream.server_close()
            report["isolated_process_stopped"] = process is None or process.poll() is not None
    print(json.dumps(report, ensure_ascii=False, indent=2))
    return 0 if report["success"] else 1


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", required=True, type=Path)
    parser.add_argument("--test-binary", required=True, type=Path)
    args = parser.parse_args()
    raise SystemExit(run(args.binary.resolve(strict=True), args.test_binary.resolve(strict=True)))
