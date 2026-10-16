#!/usr/bin/env python3
"""验证固定 NewAPI 的日志事实边界；只启动私有临时实例和回环假上游。

显式 --binary；不读取项目配置、生产数据库、真实 key 或运行端口。
结果只含白名单元数据，详细原生日志留在临时目录；finally 停止自建进程。
"""
import argparse
from collections import deque
from concurrent.futures import ThreadPoolExecutor
from datetime import datetime, timezone
import hashlib
import http.client
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
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
import urllib.parse
import urllib.request

from newapi_shared_source_probe import Client, MODEL, items

BINARY_SHA256 = "90c333eb7ace489d46e25cb687767c25c3e28584ce80384f111c03ff5f5b1205"
REQUEST_ID = "X-Oneapi-Request-Id"


class State:
    def __init__(self):
        self.lock = threading.Lock()
        self.actions = {}
        self.hits = []
        self.entered = threading.Event()
        self.release = threading.Event()
        self.later_entered = threading.Event()
        self.later_release = threading.Event()

    def reset(self, **actions):
        with self.lock:
            self.actions = {source: deque(values) for source, values in actions.items()}
            self.hits = []
            self.entered.clear()
            self.release.clear()
            self.later_entered.clear()
            self.later_release.clear()

    def next(self, source):
        with self.lock:
            self.hits.append(source)
            queue = self.actions.get(source)
            return queue.popleft() if queue else "ok"


class Upstream(BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.1"

    def log_message(self, *_args):
        pass

    def do_POST(self):
        try:
            self.handle_post()
        except (BrokenPipeError, ConnectionResetError):
            pass  # 取消是探针的预期路径。

    def handle_post(self):
        body = json.loads(self.rfile.read(int(self.headers.get("Content-Length", 0))))
        source = self.path.strip("/").split("/")[0]
        action = self.server.state.next(source)
        if action == "fail_hold":
            self.server.state.entered.set()
            assert self.server.state.release.wait(8), "fake failure gate timed out"
        if action in ("fail", "fail_hold"):
            self.json_reply(503, {"error": {"message": "fake upstream unavailable",
                "type": "server_error", "code": "unavailable"}})
            return
        if action == "hold":
            self.server.state.entered.set()
            assert self.server.state.release.wait(8), "fake response gate timed out"
        if action == "hold_later":
            self.server.state.later_entered.set()
            assert self.server.state.later_release.wait(8), "fake later gate timed out"
        if action.startswith("stream"):
            self.send_response(200)
            self.send_header("Content-Type", "text/event-stream")
            self.send_header("Connection", "close")
            self.end_headers()
            chunk = {"id": "chatcmpl-facts", "object": "chat.completion.chunk",
                     "created": int(time.time()), "model": body["model"],
                     "choices": [{"index": 0, "delta": {"role": "assistant", "content": "probe"},
                                  "finish_reason": None}]}
            self.wfile.write(("data: " + json.dumps(chunk) + "\n\n").encode())
            # 原生转换器可能保留最后一个 chunk；第二个 chunk 让第一段可实际下发。
            chunk["choices"][0]["delta"] = {"content": " "}
            self.wfile.write(("data: " + json.dumps(chunk) + "\n\n").encode())
            self.wfile.flush()
            if action == "stream_hold":
                self.server.state.entered.set()
                assert self.server.state.release.wait(8), "fake stream gate timed out"
            if action != "stream_partial":
                chunk["choices"] = [{"index": 0, "delta": {}, "finish_reason": "stop"}]
                self.wfile.write(("data: " + json.dumps(chunk) + "\n\n").encode())
                chunk["choices"] = []
                chunk["usage"] = {"prompt_tokens": 10, "completion_tokens": 2, "total_tokens": 12}
                self.wfile.write(("data: " + json.dumps(chunk) + "\n\ndata: [DONE]\n\n").encode())
                self.wfile.flush()
            self.close_connection = True
            return
        self.json_reply(200, {"id": "chatcmpl-facts", "object": "chat.completion",
            "created": int(time.time()), "model": body["model"],
            "choices": [{"index": 0, "message": {"role": "assistant", "content": source},
                         "finish_reason": "stop"}],
            "usage": {"prompt_tokens": 10, "completion_tokens": 2, "total_tokens": 12}})

    def json_reply(self, status, body):
        raw = json.dumps(body).encode()
        self.send_response(status)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(raw)))
        self.end_headers()
        self.wfile.write(raw)


def inference(base, key, stream=False):
    body = {"model": MODEL, "max_tokens": 16, "stream": stream,
            "messages": [{"role": "user", "content": "probe"}]}
    if stream:
        body["stream_options"] = {"include_usage": True}
    request = urllib.request.Request(base + "/v1/chat/completions", json.dumps(body).encode(),
        {"Authorization": "Bearer sk-" + key, "Content-Type": "application/json"}, method="POST")
    opener = urllib.request.build_opener(urllib.request.ProxyHandler({}))
    try:
        response = opener.open(request, timeout=12)
    except urllib.error.HTTPError as error:
        response = error
    with response:
        raw = response.read()
        return {"status": response.status, "request_id": response.headers.get(REQUEST_ID),
                "stream": stream, "has_done": b"data: [DONE]" in raw, "body_bytes": len(raw)}


def safe_row(row):
    other = json.loads(row.get("other") or "{}")
    admin = other.get("admin_info") or {}
    allowed = ("id", "created_at", "type", "channel", "token_id", "group", "model_name",
               "request_id", "prompt_tokens", "completion_tokens", "use_time", "is_stream")
    out = {key: row.get(key) for key in allowed}
    out.update({"other_keys": sorted(other), "admin_keys": sorted(admin),
                "used_channels": admin.get("use_channel"), "status_code": other.get("status_code"),
                "error_type": other.get("error_type"), "error_code": other.get("error_code")})
    out["stream_status"] = other.get("stream_status")
    return out


def wait_for(predicate, seconds=5):
    deadline = time.monotonic() + seconds
    while time.monotonic() < deadline:
        value = predicate()
        if value:
            return value
        time.sleep(0.05)
    raise AssertionError("isolated observation timed out")


def run(binary):
    assert hashlib.sha256(binary.read_bytes()).hexdigest() == BINARY_SHA256, "unexpected native binary"
    work = Path(tempfile.mkdtemp(prefix="quota-throttle-facts-probe-"))
    with socket.socket() as reserve:
        reserve.bind(("127.0.0.1", 0))
        port = reserve.getsockname()[1]
    assert port not in (3000, 3001)
    state = State()
    upstream = ThreadingHTTPServer(("127.0.0.1", 0), Upstream)
    upstream.daemon_threads = True
    upstream.state = state
    threading.Thread(target=upstream.serve_forever, daemon=True).start()
    env = {"PATH": os.defpath, "PORT": str(port), "GIN_MODE": "release", "TZ": "UTC",
           "GOMAXPROCS": "2", "SQLITE_PATH": str(work / "one-api.db"),
           "SESSION_SECRET": secrets.token_hex(24), "ERROR_LOG_ENABLED": "true",
           "GLOBAL_API_RATE_LIMIT_ENABLE": "false", "CRITICAL_RATE_LIMIT_ENABLE": "false"}
    result = {"temporary_directory": str(work), "binary_sha256": BINARY_SHA256, "port": port,
              "cases": {}, "checks": [], "success": False}
    process = None
    with (work / "server.log").open("wb") as log:
        try:
            process = subprocess.Popen([str(binary)], cwd=work, env=env, stdin=subprocess.DEVNULL,
                                       stdout=log, stderr=log, start_new_session=True)
            base = f"http://127.0.0.1:{port}"
            api = Client(base)
            def healthy():
                assert process.poll() is None, "isolated native process exited"
                try:
                    return api.request("GET", "/api/status")[0] == 200
                except (OSError, urllib.error.URLError):
                    return False
            wait_for(healthy, 30)
            listening = subprocess.check_output(["ss", "-ltnp", f"sport = :{port}"], text=True)
            assert f"pid={process.pid}," in listening and (work / "one-api.db").is_file()
            password = secrets.token_urlsafe(20)
            api.api("POST", "/api/setup", {"username": "qtfacts", "password": password,
                    "confirmPassword": password, "SelfUseModeEnabled": True})
            api.user = api.api("POST", "/api/user/login", {"username": "qtfacts", "password": password})["id"]
            for name, value in {"GroupRatio": json.dumps({"default": 1, "facts-one": 1, "facts-two": 1}),
                    "UserUsableGroups": json.dumps({"default": "D", "facts-one": "One", "facts-two": "Two"}),
                    "RetryTimes": "2", "LogConsumeEnabled": "true"}.items():
                api.api("PUT", "/api/option/", {"key": name, "value": value})
            for name, source, group, priority in [("S-one", "S", "facts-one", 100),
                    ("S-two", "S", "facts-two", 100), ("T-two", "T", "facts-two", 10)]:
                api.api("POST", "/api/channel/", {"mode": "single", "channel": {"name": name,
                    "type": 8, "key": "fake-upstream-" + source,
                    "base_url": f"http://127.0.0.1:{upstream.server_port}/{source}/chat/completions",
                    "models": MODEL, "group": group, "priority": priority, "weight": 10,
                    "status": 2, "auto_ban": 0}})
            channels = {row["name"]: row["id"] for row in items(api.api("GET", "/api/channel/?p=1&page_size=100"))}
            api.api("POST", "/api/channel/status/batch", {"ids": list(channels.values()), "status": 1})
            for group in ("facts-one", "facts-two"):
                api.api("POST", "/api/token/", {"name": group, "group": group,
                    "unlimited_quota": True, "expired_time": -1, "cross_group_retry": False})
            keys = {row["name"]: api.api("POST", f"/api/token/{row['id']}/key", {})["key"]
                    for row in items(api.api("GET", "/api/token/?p=1&page_size=100"))}
            def logs(request_id=None):
                path = "/api/log/?p=1&page_size=100"
                if request_id:
                    path += "&request_id=" + urllib.parse.quote(request_id)
                return [row for row in items(api.api("GET", path)) if row["type"] in (2, 5)]
            def record_case(name, response, expected_types=(2,), expected_errors=0):
                rid = response["request_id"]
                assert rid, "missing native request ID"
                first = logs(rid)
                def enough_rows():
                    rows = logs(rid)
                    return rows if set(expected_types).issubset({row["type"] for row in rows}) and sum(
                        row["type"] == 5 for row in rows) >= expected_errors else None
                rows = wait_for(enough_rows)
                result["cases"][name] = {**response, "hits": list(state.hits),
                                         "first_visible_types": sorted(row["type"] for row in first),
                                         "logs": [safe_row(row) for row in rows]}
                return rows

            state.reset(S=["ok"])
            normal = record_case("normal", inference(base, keys["facts-one"]))
            assert normal[0]["type"] == 2 and normal[0]["prompt_tokens"] == 10 and normal[0]["completion_tokens"] == 2
            state.reset(S=["fail", "ok"])
            same = record_case("same_source_fail_then_success", inference(base, keys["facts-one"]), (2, 5))
            assert {row["type"] for row in same} == {2, 5} and len({row["request_id"] for row in same}) == 1
            result["checks"].append("same_source_error_is_not_final_failure")

            state.reset(S=["fail"] * 3)
            all_failed_response = inference(base, keys["facts-one"])
            failed = record_case("all_attempts_fail", all_failed_response, (5,), 3)
            assert all_failed_response["status"] == 503 and len(failed) == 3 and all(row["type"] == 5 for row in failed)

            state.reset(S=["fail"], T=["hold"])
            previous = {row["id"] for row in logs()}
            with ThreadPoolExecutor(max_workers=1) as pool:
                future = pool.submit(inference, base, keys["facts-two"])
                assert state.entered.wait(4), "fallback not reached"
                pending = wait_for(lambda: [row for row in logs() if row["id"] not in previous])
                assert not future.done() and all(row["type"] == 5 for row in pending)
                result["pending_error_before_terminal"] = [safe_row(row) for row in pending]
                state.release.set()
                response = future.result(timeout=5)
            fallback = record_case("fallback_success", response, (2, 5))
            assert {row["type"] for row in fallback} == {2, 5}
            assert len({row["channel"] for row in fallback}) == 2
            result["checks"].append("error_rows_visible_while_request_still_running")

            # 不能用当前 RetryTimes + 已记录尝试数推断请求终态。
            api.api("PUT", "/api/option/", {"key": "RetryTimes", "value": "0"})
            state.reset(S=["fail_hold", "hold_later"])
            previous = {row["id"] for row in logs()}
            with ThreadPoolExecutor(max_workers=1) as pool:
                future = pool.submit(inference, base, keys["facts-one"])
                assert state.entered.wait(4), "first attempt not reached"
                api.api("PUT", "/api/option/", {"key": "RetryTimes", "value": "1"})
                state.release.set()
                assert state.later_entered.wait(4), "changed retry budget not used"
                api.api("PUT", "/api/option/", {"key": "RetryTimes", "value": "0"})
                retry_value = next(row["value"] for row in api.api("GET", "/api/option/")
                                   if row["key"] == "RetryTimes")
                pending = wait_for(lambda: [row for row in logs() if row["id"] not in previous])
                assert retry_value == "0" and not future.done() and len(pending) == 1
                assert pending[0]["type"] == 5
                assert len(safe_row(pending[0])["used_channels"]) == int(retry_value) + 1
                result["retry_budget_during_request"] = {"initial": 0, "during_first_attempt": 1,
                    "current_while_second_attempt_pending": int(retry_value),
                    "pending_rows": [safe_row(row) for row in pending]}
                state.later_release.set()
                response = future.result(timeout=5)
            changed = record_case("retry_budget_changed_then_success", response, (2, 5))
            assert response["status"] == 200 and state.hits == ["S", "S"]
            assert len(changed) == 2
            api.api("PUT", "/api/option/", {"key": "RetryTimes", "value": "2"})
            result["checks"].append("current_retry_budget_does_not_prove_terminal")

            for name, action in (("stream_complete", "stream"), ("stream_partial_eof", "stream_partial")):
                state.reset(S=[action])
                response = inference(base, keys["facts-one"], True)
                rows = record_case(name, response)
                assert response["status"] == 200 and response["has_done"] and len(rows) == 1
                assert safe_row(rows[0])["stream_status"] == {
                    "status": "ok", "end_reason": "done" if action == "stream" else "eof"}

            state.reset(S=["stream_hold"])
            connection = http.client.HTTPConnection("127.0.0.1", port, timeout=5)
            connection.request("POST", "/v1/chat/completions", json.dumps({"model": MODEL, "max_tokens": 16,
                "stream": True, "stream_options": {"include_usage": True},
                "messages": [{"role": "user", "content": "probe"}]}),
                {"Authorization": "Bearer sk-" + keys["facts-one"], "Content-Type": "application/json"})
            response = connection.getresponse()
            rid = response.getheader(REQUEST_ID)
            assert state.entered.wait(4) and response.readline().startswith(b"data:") and rid
            before = logs(rid)
            assert not before, "unexpected terminal/consumption row for held stream"
            result["stream_in_flight_rows"] = [safe_row(row) for row in before]
            stamp = datetime.now(timezone.utc).strftime("%Y/%m/%d - %H:%M:%S")
            forged = f"[GIN] {stamp} | relay | {rid} | 503 | 1s | 127.0.0.1 | POST /v1/chat/completions"
            path = urllib.parse.quote("/facts-not-found\n" + forged + "\n", safe="/")
            api.request("GET", path)
            result["console_terminal_line_spoofed_by_path"] = any(
                line == forged for line in (work / "server.log").read_text(errors="replace").splitlines())
            assert result["console_terminal_line_spoofed_by_path"], "console spoof observation changed"
            response.close()
            connection.close()
            state.release.set()
            cancelled = wait_for(lambda: logs(rid))
            assert len(cancelled) == 1 and cancelled[0]["type"] == 2
            cancel_status = safe_row(cancelled[0])["stream_status"]
            assert cancel_status["status"] == "error" and cancel_status["end_reason"] == "client_gone"
            result["cases"]["client_cancel_stream"] = {"request_id": rid,
                "hits": list(state.hits), "logs": [safe_row(row) for row in cancelled]}
            result["channel_ids"] = channels
            result["checks"].append("stream_and_cancel_metadata_observed")
            result["success"] = True
        except Exception as error:
            result["error"] = f"{type(error).__name__}: {error}"
        finally:
            state.release.set()
            state.later_release.set()
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
    (work / "result.json").write_text(json.dumps(result, ensure_ascii=False, indent=2))
    print(json.dumps(result, ensure_ascii=False, indent=2))
    return 0 if result["success"] else 1


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", required=True, type=Path)
    args = parser.parse_args()
    raise SystemExit(run(args.binary.resolve(strict=True)))
