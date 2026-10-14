#!/usr/bin/env python3
"""隔离验证已安装 NewAPI 的共享来源契约；不读取项目配置或生产数据库。

python3 scripts/probes/newapi_shared_source_probe.py --binary /absolute/new-api
仅使用随机端口、全新临时 SQLite、临时凭据和本机假上游。退出时停止自建进程。
"""

import argparse
import http.cookiejar
import json
import os
from pathlib import Path
import secrets
import socket
import subprocess
import tempfile
import threading
import time
import urllib.error
import urllib.request
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer


MODEL = "gpt-4o-mini"
SECOND_MODEL = "gpt-4o"


class MockUpstream(BaseHTTPRequestHandler):
    failures = set()
    hits = []

    def log_message(self, *_args):
        pass

    def do_POST(self):
        body = json.loads(self.rfile.read(int(self.headers.get("Content-Length", 0))))
        source = self.path.strip("/").split("/")[0]
        self.hits.append(source)
        if source in self.failures:
            status, result = 503, {"error": {"message": "mock upstream unavailable",
                                          "type": "server_error", "code": "unavailable"}}
        else:
            status, result = 200, {
                "id": "chatcmpl-probe", "object": "chat.completion", "created": int(time.time()),
                "model": body["model"], "choices": [{"index": 0, "finish_reason": "stop",
                    "message": {"role": "assistant", "content": source}}],
                "usage": {"prompt_tokens": 10, "completion_tokens": 2, "total_tokens": 12},
            }
        raw = json.dumps(result).encode()
        self.send_response(status)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(raw)))
        self.end_headers()
        self.wfile.write(raw)


class Client:
    def __init__(self, base):
        self.base = base
        self.user = None
        self.opener = urllib.request.build_opener(
            urllib.request.ProxyHandler({}),
            urllib.request.HTTPCookieProcessor(http.cookiejar.CookieJar()),
        )

    def request(self, method, path, data=None, headers=None):
        hdr = {"Content-Type": "application/json"}
        if self.user is not None:
            hdr["New-Api-User"] = str(self.user)
        hdr.update(headers or {})
        req = urllib.request.Request(self.base + path,
            data=None if data is None else json.dumps(data).encode(), headers=hdr, method=method)
        try:
            resp = self.opener.open(req, timeout=5)
        except urllib.error.HTTPError as err:
            resp = err
        with resp:
            raw = resp.read()
            try:
                result = json.loads(raw)
            except json.JSONDecodeError:
                result = {"non_json": True}
            return resp.code, result

    def api(self, method, path, data=None):
        status, result = self.request(method, path, data)
        if status >= 400 or not result.get("success"):
            raise RuntimeError(f"{method} {path}: HTTP {status}, {result.get('message', '')}")
        return result.get("data")


def items(data):
    return data if isinstance(data, list) else data["items"]


def run(binary):
    work = Path(tempfile.mkdtemp(prefix="quota-throttle-newapi-audit-"))
    with socket.socket() as reserve:
        reserve.bind(("127.0.0.1", 0))
        port = reserve.getsockname()[1]
    assert port not in (3000, 3001)
    upstream = ThreadingHTTPServer(("127.0.0.1", 0), MockUpstream)
    thread = threading.Thread(target=upstream.serve_forever, daemon=True)
    thread.start()
    env = {
        "PATH": os.defpath, "PORT": str(port), "GIN_MODE": "release", "GOMAXPROCS": "2",
        "SQLITE_PATH": str(work / "one-api.db"), "SESSION_SECRET": secrets.token_hex(24),
        "GLOBAL_API_RATE_LIMIT_ENABLE": "false", "CRITICAL_RATE_LIMIT_ENABLE": "false",
        "ERROR_LOG_ENABLED": "true",
    }
    result = {"temporary_directory": str(work), "port": port, "checks": []}
    process = None
    with (work / "server.log").open("wb") as log:
        try:
            process = subprocess.Popen([str(binary)], cwd=work, env=env,
                stdin=subprocess.DEVNULL, stdout=log, stderr=log, start_new_session=True)
            result["pid"] = process.pid
            api = Client(f"http://127.0.0.1:{port}")
            deadline = time.monotonic() + 30
            while time.monotonic() < deadline:
                if process.poll() is not None:
                    raise RuntimeError("隔离 NewAPI 提前退出，参见临时 server.log")
                try:
                    if api.request("GET", "/api/status")[0] == 200:
                        break
                except (OSError, urllib.error.URLError):
                    pass
                time.sleep(0.2)
            else:
                raise RuntimeError("隔离 NewAPI 启动超时")
            listening = subprocess.check_output(
                ["ss", "-ltnp", f"sport = :{port}"], text=True)
            assert f"pid={process.pid}," in listening, "端口不属于本次启动的进程，拒绝操作"
            assert (work / "one-api.db").is_file(), "未发现本次独立数据库，拒绝操作"
            password = secrets.token_urlsafe(20)
            api.api("POST", "/api/setup", {"username": "qtaudit", "password": password,
                "confirmPassword": password, "SelfUseModeEnabled": True})
            login = api.api("POST", "/api/user/login", {"username": "qtaudit", "password": password})
            api.user = login["id"]
            for name, value in {
                "GroupRatio": json.dumps({"default": 1, "audit-a": 1, "audit-b": 1}),
                "UserUsableGroups": json.dumps({"default": "default", "audit-a": "A", "audit-b": "B"}),
                "RetryTimes": "1",
                "LogConsumeEnabled": "true",
            }.items():
                api.api("PUT", "/api/option/", {"key": name, "value": value})

            channel_ids = {}
            for name, source, group, priority in [
                ("S-A", "S", "audit-a", 100), ("T-A", "T", "audit-a", 10),
                ("S-B", "S", "audit-b", 10), ("T-B", "T", "audit-b", 100),
            ]:
                api.api("POST", "/api/channel/", {"mode": "single", "channel": {
                    "name": name, "type": 8, "key": "fake-upstream-" + source,
                    "base_url": f"http://127.0.0.1:{upstream.server_port}/{source}/chat/completions",
                    "models": MODEL + "," + SECOND_MODEL, "group": group, "priority": priority,
                    "weight": 10, "status": 2, "auto_ban": 0,
                }})
            for ch in items(api.api("GET", "/api/channel/?p=1&page_size=100")):
                channel_ids[ch["name"]] = ch["id"]
                assert ch["status"] == 2, "创建时的禁用状态未保留"
            result["checks"].append("create_disabled_channels")

            copied = api.api("POST", f"/api/channel/copy/{channel_ids['S-A']}?suffix=-copy", {})
            clone = api.api("GET", f"/api/channel/{copied['id']}")
            assert (clone["group"], clone["priority"], clone["status"]) == ("audit-a", 100, 2)
            api.api("DELETE", f"/api/channel/{copied['id']}")
            result["checks"].append("copy_inherits_group_priority_and_status")

            keys = {}
            for group in ("audit-a", "audit-b"):
                api.api("POST", "/api/token/", {"name": group, "group": group,
                    "unlimited_quota": True, "expired_time": -1, "cross_group_retry": False})
            token_ids = {}
            for token in items(api.api("GET", "/api/token/?p=1&page_size=100")):
                token_ids[token["name"]] = token["id"]
                keys[token["name"]] = api.api("POST", f"/api/token/{token['id']}/key", {})["key"]

            def inference(group, claude=False, model=MODEL):
                path = "/v1/messages" if claude else "/v1/chat/completions"
                body = {"model": model, "max_tokens": 16, "stream": False,
                        "messages": [{"role": "user", "content": "probe"}]}
                status, response = api.request("POST", path, body,
                    {"Authorization": "Bearer sk-" + keys[group], "anthropic-version": "2023-06-01"})
                return status, response

            status, _ = inference("audit-a")
            assert status >= 400 and not MockUpstream.hits, "禁用渠道意外收到推理请求"
            result["checks"].append("disabled_channels_not_routed")
            api.api("POST", "/api/channel/status/batch", {"ids": list(channel_ids.values()), "status": 1})
            for group, expected in (("audit-a", "S"), ("audit-b", "T")):
                status, response = inference(group)
                assert status == 200, f"{group} OpenAI HTTP {status}: {response.get('error', {})}"
                assert response["choices"][0]["message"]["content"] == expected
                status, response = inference(group, claude=True)
                assert status == 200, f"{group} Claude HTTP {status}: {response.get('error', {})}"
                assert response["content"][0]["text"] == expected
            result["checks"].extend(["independent_group_priority", "same_token_both_protocols"])

            def priority(name, value):
                channel = api.api("GET", f"/api/channel/{channel_ids[name]}")
                channel.pop("status", None)
                channel["priority"] = value
                api.api("PUT", "/api/channel/", channel)

            priority("S-A", 10)
            priority("T-A", 100)
            assert api.api("GET", f"/api/channel/{channel_ids['S-B']}")["priority"] == 10
            assert api.api("GET", f"/api/channel/{channel_ids['T-B']}")["priority"] == 100
            assert inference("audit-a")[1]["choices"][0]["message"]["content"] == "T"
            result["checks"].append("priority_update_does_not_change_other_group")
            priority("T-A", 10)
            priority("S-A", 100)
            priority("T-B", 10)
            priority("S-B", 100)
            assert inference("audit-b")[1]["choices"][0]["message"]["content"] == "S"
            priority("S-B", 10)
            priority("T-B", 100)
            assert inference("audit-a", model=SECOND_MODEL)[0] == 200

            MockUpstream.failures.add("S")
            status, response = inference("audit-a")
            assert status == 200 and response["choices"][0]["message"]["content"] == "T"
            MockUpstream.failures.clear()
            assert inference("audit-a")[1]["choices"][0]["message"]["content"] == "S"
            result["checks"].append("retry_is_temporary_not_persistent_switch")

            api.api("POST", "/api/channel/status/batch", {
                "ids": [channel_ids["S-A"], channel_ids["S-B"]], "status": 2})
            for group in ("audit-a", "audit-b"):
                assert inference(group)[1]["choices"][0]["message"]["content"] == "T"
            result["checks"].append("pause_all_shared_source_channels")

            changed = api.api("POST", "/api/channel/status/batch", {
                "ids": [channel_ids["S-B"], 999999], "status": 1})
            assert changed == 1, "批量状态接口返回语义发生变化"
            assert api.api("GET", f"/api/channel/{channel_ids['S-B']}")["status"] == 1
            api.api("POST", f"/api/channel/{channel_ids['S-B']}/status", {"status": 2})
            result["checks"].append("batch_status_accepts_missing_id_requires_readback")

            logs = []
            for _ in range(15):
                logs = items(api.api("GET", "/api/log/?p=1&page_size=100"))
                consumed = [x for x in logs if x.get("type") == 2]
                errors = [x for x in logs if x.get("type") == 5]
                if len(consumed) >= 11 and errors:
                    break
                time.sleep(0.2)
            assert consumed and errors, "消费/错误日志没有按预期出现"
            required = {"id", "created_at", "channel", "token_id", "group", "model_name",
                        "prompt_tokens", "completion_tokens", "request_id"}
            assert all(required.issubset(x) for x in consumed)
            source_errors = [x for x in errors if x.get("channel") == channel_ids["S-A"]]
            assert source_errors, "首选失败事件未保留实际渠道归属"
            assert any(c["request_id"] == e["request_id"] and c["channel"] == channel_ids["T-A"]
                       for c in consumed for e in source_errors), "成功兜底与原源错误无法关联"
            result["checks"].append("logs_preserve_channel_token_model_and_error_metadata")
            unique = {row["id"]: row for row in consumed + consumed}
            source_by_channel = {cid: name.split("-")[0] for name, cid in channel_ids.items()}
            by_source, by_token, by_model = {}, {}, {}
            for row in unique.values():
                tokens = row["prompt_tokens"] + row["completion_tokens"]
                for output, key in ((by_source, source_by_channel[row["channel"]]),
                                    (by_token, str(row["token_id"])), (by_model, row["model_name"])):
                    output[key] = output.get(key, 0) + tokens
            total = sum(by_source.values())
            assert len(unique) == 11 and total == 132
            assert sum(by_token.values()) == total == sum(by_model.values())
            assert {MODEL, SECOND_MODEL} == set(by_model)
            assert {channel_ids["S-A"], channel_ids["S-B"]}.issubset({x["channel"] for x in consumed})
            result["checks"].append("shared_source_token_and_model_totals_reconcile")
            result["usage"] = {"total": total, "source_totals": by_source,
                               "token_totals": by_token, "model_totals": by_model}
            result["log_counts"] = {"consume": len(consumed), "error": len(errors)}
            result["channel_ids"] = channel_ids
            result["success"] = True
        except Exception as exc:
            result["success"] = False
            result["error"] = str(exc)
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
            result["isolated_process_stopped"] = process is None or process.poll() is not None
    print(json.dumps(result, ensure_ascii=False, indent=2))
    return 0 if result.get("success") else 1


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", required=True, type=Path)
    args = parser.parse_args()
    binary_path = args.binary.resolve(strict=True)
    raise SystemExit(run(binary_path))
