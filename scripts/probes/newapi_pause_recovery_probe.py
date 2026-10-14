#!/usr/bin/env python3
"""在全新 NewAPI 实例中复现“测试开始后人工暂停”的原生恢复竞争。

只读使用显式传入的二进制；每个场景使用独立临时 SQLite、随机端口、
临时管理员及假上游。绝不读取生产 config.toml、数据库或 key。
"""

import argparse
import json
import os
from pathlib import Path
import secrets
import socket
import subprocess
import sys
import tempfile
import threading
import time
import urllib.error
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

# 诊断脚本不在仓库留下导入字节码缓存。
sys.dont_write_bytecode = True
from newapi_shared_source_probe import Client, MODEL, items


class GatedUpstream(BaseHTTPRequestHandler):
    fail = True
    hold = False
    entered = threading.Event()
    release = threading.Event()
    hits = 0

    def log_message(self, *_args):
        pass

    def do_POST(self):
        body = json.loads(self.rfile.read(int(self.headers.get("Content-Length", 0))))
        type(self).hits += 1
        if type(self).fail:
            status, result = 401, {"error": {"message": "mock invalid key",
                "type": "invalid_request_error", "code": "invalid_api_key"}}
        else:
            if type(self).hold:
                type(self).entered.set()
                if not type(self).release.wait(15):
                    self.send_error(504)
                    return
            status, result = 200, {
                "id": "chatcmpl-pause-probe", "object": "chat.completion",
                "created": int(time.time()), "model": body["model"],
                "choices": [{"index": 0, "finish_reason": "stop",
                    "message": {"role": "assistant", "content": "probe ok"}}],
                "usage": {"prompt_tokens": 5, "completion_tokens": 3, "total_tokens": 8},
            }
        raw = json.dumps(result).encode()
        self.send_response(status)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(raw)))
        self.end_headers()
        self.wfile.write(raw)


def poll_until(read, predicate, description, timeout=12):
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        value = read()
        if predicate(value):
            return value
        time.sleep(0.05)
    raise RuntimeError(description)


def run_case(binary, automatic_enable, auto_ban_at_recovery):
    work = Path(tempfile.mkdtemp(prefix="quota-throttle-pause-race-"))
    with socket.socket() as reserve:
        reserve.bind(("127.0.0.1", 0))
        port = reserve.getsockname()[1]
    assert port not in (3000, 3001)
    GatedUpstream.fail = True
    GatedUpstream.hold = False
    GatedUpstream.hits = 0
    GatedUpstream.entered.clear()
    GatedUpstream.release.clear()
    upstream = ThreadingHTTPServer(("127.0.0.1", 0), GatedUpstream)
    threading.Thread(target=upstream.serve_forever, daemon=True).start()
    env = {
        "PATH": os.defpath, "PORT": str(port), "GIN_MODE": "release", "GOMAXPROCS": "2",
        "SQLITE_PATH": str(work / "one-api.db"), "SESSION_SECRET": secrets.token_hex(24),
        "GLOBAL_API_RATE_LIMIT_ENABLE": "false", "CRITICAL_RATE_LIMIT_ENABLE": "false",
        "ERROR_LOG_ENABLED": "true",
    }
    result = {"automatic_enable": automatic_enable,
              "auto_ban_at_recovery": auto_ban_at_recovery,
              "directory": str(work), "port": port}
    process = None
    with (work / "server.log").open("wb") as log:
        try:
            process = subprocess.Popen([str(binary)], cwd=work, env=env,
                stdin=subprocess.DEVNULL, stdout=log, stderr=log, start_new_session=True)
            result["pid"] = process.pid
            api = Client(f"http://127.0.0.1:{port}")

            def ready():
                if process.poll() is not None:
                    raise RuntimeError("隔离 NewAPI 提前退出")
                try:
                    return api.request("GET", "/api/status")[0] == 200
                except (OSError, urllib.error.URLError):
                    return False

            poll_until(ready, bool, "隔离进程启动超时", timeout=30)
            listening = subprocess.check_output(["ss", "-ltnp", f"sport = :{port}"], text=True)
            assert f"pid={process.pid}," in listening, "端口不是本次进程，拒绝任何写入"
            assert (work / "one-api.db").is_file(), "未发现独立数据库，拒绝任何写入"
            password = secrets.token_urlsafe(20)
            api.api("POST", "/api/setup", {"username": "pauseaudit", "password": password,
                "confirmPassword": password, "SelfUseModeEnabled": True})
            api.user = api.api("POST", "/api/user/login",
                {"username": "pauseaudit", "password": password})["id"]
            for name, value in {
                "AutomaticDisableChannelEnabled": "true",
                "AutomaticEnableChannelEnabled": str(automatic_enable).lower(),
                "RetryTimes": "0",
            }.items():
                api.api("PUT", "/api/option/", {"key": name, "value": value})
            options = {row["key"]: row["value"] for row in items(api.api("GET", "/api/option/"))}
            assert options["AutomaticEnableChannelEnabled"] == str(automatic_enable).lower()
            api.api("POST", "/api/channel/", {"mode": "single", "channel": {
                "name": "pause-race-source", "type": 8, "key": "fake-pause-upstream-key",
                "base_url": f"http://127.0.0.1:{upstream.server_port}/chat/completions",
                "models": MODEL, "test_model": MODEL, "group": "default",
                "priority": 100, "weight": 10, "status": 1, "auto_ban": 1,
            }})
            channels = items(api.api("GET", "/api/channel/?p=1&page_size=100"))
            assert len(channels) == 1
            channel_id = channels[0]["id"]
            result["channel_id"] = channel_id
            api.api("POST", "/api/token/", {"name": "pause-race-consumer", "group": "default",
                "unlimited_quota": True, "expired_time": -1, "cross_group_retry": False})
            token_id = items(api.api("GET", "/api/token/?p=1&page_size=100"))[0]["id"]
            key = api.api("POST", f"/api/token/{token_id}/key", {})["key"]
            status, _ = api.request("POST", "/v1/chat/completions", {
                "model": MODEL, "max_tokens": 16, "stream": False,
                "messages": [{"role": "user", "content": "probe"}]},
                {"Authorization": "Bearer sk-" + key})
            assert status == 401 and GatedUpstream.hits > 0, "未实际产生假上游 401"

            def channel():
                if process.poll() is not None:
                    raise RuntimeError("隔离进程已退出，停止查询")
                return api.api("GET", f"/api/channel/{channel_id}")

            before = poll_until(channel, lambda c: c["status"] == 3, "未出现原生自动禁用")
            if auto_ban_at_recovery != 1:
                changed = dict(before)
                changed.pop("status", None)
                changed["auto_ban"] = auto_ban_at_recovery
                api.api("PUT", "/api/channel/", changed)
                assert channel()["auto_ban"] == auto_ban_at_recovery
            previous_test_time = channel()["test_time"]
            # 秒级 test_time 必须能区分新测试；不是用 sleep 猜后台任务完成。
            poll_until(lambda: int(time.time()), lambda t: t > previous_test_time,
                "测试时间戳无法推进", timeout=3)
            GatedUpstream.fail = False
            GatedUpstream.hold = True
            task = api.api("GET", "/api/channel/test")
            result["task_id"] = task.get("task_id") if isinstance(task, dict) else None
            assert GatedUpstream.entered.wait(12), "后台测试未进入受控假上游"
            # 测试此时已捕获 status=3，尚未得到成功结果。
            api.api("POST", f"/api/channel/{channel_id}/status", {"status": 2})
            assert channel()["status"] == 2
            result["paused_status"] = 2
            GatedUpstream.release.set()
            # UpdateResponseTime 位于恢复判断之后，test_time 推进证明该判断已执行。
            after = poll_until(channel, lambda c: c["test_time"] > previous_test_time,
                "测试尚未完成恢复判定")
            result["after_recovery_status"] = after["status"]
            result["test_time_advanced"] = True
            expected = 1 if automatic_enable else 2
            assert after["status"] == expected, "实测不符合预期的恢复竞争结果"
            result["success"] = True
        except Exception as exc:
            result["success"] = False
            result["error"] = str(exc)
        finally:
            GatedUpstream.release.set()
            if process is not None and process.poll() is None:
                process.terminate()
                try:
                    process.wait(timeout=5)
                except subprocess.TimeoutExpired:
                    process.kill()
                    process.wait(timeout=5)
            upstream.shutdown()
            upstream.server_close()
            result["isolated_process_stopped"] = process is None or process.poll() is not None
    return result


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", required=True, type=Path)
    args = parser.parse_args()
    binary = args.binary.resolve(strict=True)
    digest = subprocess.check_output(["sha256sum", str(binary)], text=True).split()[0]
    cases = [run_case(binary, enabled, auto_ban)
             for enabled, auto_ban in [(True, 1), (False, 1), (True, 0)]]
    print(json.dumps({"binary_sha256": digest, "cases": cases}, ensure_ascii=False, indent=2))
    raise SystemExit(0 if all(c["success"] and c["isolated_process_stopped"] for c in cases) else 1)
