#!/usr/bin/env python3
"""Real serve/NewAPI/HTTP/ledger combination, entirely private and synthetic."""
import argparse
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
import http.client
import json
import os
from pathlib import Path
import signal
import socket
import subprocess
import sys
import tempfile
import threading
import time
import tomllib
import uuid

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "deploy"))
from initialize_groups import BootstrapError, Native, initialize_fresh, stop_owned

MODELS = ["gpt-4o-mini", "gpt-4o"]
CHECKS = ["fresh_start_and_real_backend", "model_discovery_and_no_duplicate_source",
          "disabled_creation_then_group_check", "independent_shared_group_priority",
          "stale_and_invalid_commands_write_nothing",
          "same_key_both_protocols_and_streams", "ordinary_owner_group_model_isolation",
          "real_logs_and_model_token_marginals", "success_token_flow_clears_failures",
          "shared_error_exhaustion_and_manual_clear", "shared_soft_token_limit",
          "pause_survives_error_clear_and_reset", "glm_95_policy_unchanged",
          "token_enable_disable_delete_and_history", "restart_preserves_identity_and_usage"]


class ProbeError(Exception):
    pass


class ReadUnavailable(ProbeError):
    pass


def ensure(value, label):
    if not value:
        raise ProbeError(label)


def until(read, predicate=lambda value: bool(value), seconds=180):
    deadline = time.monotonic() + seconds
    while time.monotonic() < deadline:
        try:
            value = read()
            if predicate(value):
                return value
        except ReadUnavailable:
            pass
        time.sleep(0.5)
    raise ProbeError("bounded_observation_deadline")


class Upstream(BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.1"

    def log_message(self, *_args):
        pass

    def reply(self, status, value):
        raw = json.dumps(value).encode()
        self.send_response(status)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(raw)))
        self.end_headers()
        self.wfile.write(raw)

    def do_GET(self):
        source = self.path.strip("/").split("/")[0]
        if self.path.endswith("/models"):
            self.server.models_read.append(source)
            self.reply(200, {"object": "list", "data": [{"id": model, "object": "model"} for model in MODELS]})
        else:
            pct = self.server.percentages.get(source, 5)
            self.reply(200, {"success": True, "data": {"level": "fixture", "limits": [
                {"type": "TOKENS_LIMIT", "percentage": pct, "unit": 3, "number": 5,
                 "nextResetTime": int(time.time() * 1000) + 3600000},
                {"type": "TOKENS_LIMIT", "percentage": 5, "unit": 6, "number": 1,
                 "nextResetTime": int(time.time() * 1000) + 172800000}]}})

    def do_POST(self):
        body = json.loads(self.rfile.read(int(self.headers.get("Content-Length", "0"))))
        source = self.path.strip("/").split("/")[0]
        self.server.hits.append((source, body["model"]))
        if source in self.server.failures:
            self.reply(503, {"error": {"type": "server_error", "code": "unavailable", "message": "synthetic unavailable"}})
            return
        if not body.get("stream"):
            self.reply(200, {"id": "chatcmpl-sgm7", "object": "chat.completion", "created": int(time.time()),
                "model": body["model"], "choices": [{"index": 0, "finish_reason": "stop",
                "message": {"role": "assistant", "content": source}}],
                "usage": {"prompt_tokens": 10, "completion_tokens": 2, "total_tokens": 12}})
            return
        self.send_response(200)
        self.send_header("Content-Type", "text/event-stream")
        self.send_header("Connection", "close")
        self.end_headers()
        chunk = {"id": "chatcmpl-sgm7", "object": "chat.completion.chunk", "created": int(time.time()), "model": body["model"]}
        for delta, reason in [({"role": "assistant", "content": source}, None), ({"content": " "}, None), ({}, "stop")]:
            chunk["choices"] = [{"index": 0, "delta": delta, "finish_reason": reason}]
            self.wfile.write(("data: " + json.dumps(chunk) + "\n\n").encode())
        chunk["choices"] = []
        chunk["usage"] = {"prompt_tokens": 10, "completion_tokens": 2, "total_tokens": 12}
        self.wfile.write(("data: " + json.dumps(chunk) + "\n\ndata: [DONE]\n\n").encode())
        self.wfile.flush()
        self.close_connection = True


class Panel:
    def __init__(self, port):
        self.port = port
        self.base = f"http://127.0.0.1:{port}"

    def raw(self, method, path, body=None):
        conn = http.client.HTTPConnection("127.0.0.1", self.port, timeout=40)
        try:
            data = None if body is None else json.dumps({"schema_version": 1, **body})
            conn.request(method, "/api/admin/v1" + path, data,
                         {"X-QT-Panel": "1", "Origin": self.base, "Content-Type": "application/json"})
            response = conn.getresponse()
            raw = response.read(32 * 1024 * 1024 + 1)
            ensure(len(raw) <= 32 * 1024 * 1024, "panel_response_bound")
            return response.status, json.loads(raw)
        except OSError:
            if method == "GET" or path == "/usage/query":
                raise ReadUnavailable("read_transport_unavailable") from None
            raise
        finally:
            conn.close()

    def call(self, method, path, body=None):
        status, value = self.raw(method, path, body)
        if status not in (200, 201, 202):
            if method == "GET" and path == "/state" and status == 503 and value.get("error") in ("snapshot_unavailable", "overloaded"):
                raise ReadUnavailable("state_not_yet_published")
            if (method == "GET" or path == "/usage/query") and status == 503 and value.get("error") in ("queue_full", "not_ready"):
                raise ReadUnavailable("read_temporarily_unavailable")
            raise ProbeError(f"panel_{method}_{path}_status_{status}_{value.get('error', 'unknown')}")
        return value

    def state(self):
        return self.call("GET", "/state")

    def entity(self, family, key, ident):
        return next((entry for entry in self.state()["catalog"][family] if entry[key][key + "_id"] == ident), None)

    def source(self, ident):
        return self.entity("sources", "source", ident)

    def group(self, ident):
        return self.entity("groups", "group", ident)

    def member(self, ident):
        return self.entity("memberships", "membership", ident)

    def settled_group(self, ident, preferred):
        def settled():
            snapshot = self.state()["catalog"]
            group = next(g for g in snapshot["groups"] if g["group"]["group_id"] == ident)
            members = [m for m in snapshot["memberships"] if m["membership"]["group_id"] == ident]
            return group["group"]["ready"] and group["selected_member_id"] == preferred and bool(members) and all(
                not m["dirty"] and m["membership"]["observed_priority"] == m["membership"]["desired_priority"]
                and (m["membership"]["membership_id"] != preferred or m["membership"]["observed_status"] == 1)
                for m in members)
        until(settled)

    def operation(self, method, path, body, wait=True):
        ident = "qt_" + str(uuid.uuid4())
        result = self.call(method, path, {"operation_id": ident, **body})
        ensure(result["operation_id"] == ident, "operation_identity")
        if wait:
            fields = ("operation_id", "kind", "target_type", "target_id", "target_revision")
            binding = {key: result[key] for key in fields}
            last_retry = 0
            def observe():
                nonlocal last_retry
                receipt = self.call("GET", f"/operations/{ident}")
                ensure({key: receipt[key] for key in fields} == binding, "operation_binding")
                if receipt["phase"] == "waiting" and receipt.get("error_code") in ("queue_full", "not_ready", "revision_conflict") and time.monotonic() - last_retry >= 10:
                    # 模拟已有UI的显式重试；原operation不变，未知交付绝不自动重建。
                    status, retried = self.raw("POST", f"/operations/{ident}/retry", {})
                    ensure(status == 200, "explicit_retry_response")
                    ensure({key: retried[key] for key in fields} == binding, "explicit_retry_binding")
                    print("EXPLICIT OPERATION RETRY", receipt["kind"], receipt["error_code"], status, flush=True)
                    last_retry = time.monotonic()
                return receipt
            receipt = until(observe,
                            lambda value: value["phase"] in ("succeeded", "failed"))
            ensure(receipt["phase"] == "succeeded", f"operation_{receipt.get('error_code')}")
        return result

    def pause(self, ident, paused):
        source = until(lambda: self.source(ident))["source"]
        self.operation("PUT", f"/sources/{ident}/paused", {"expected_revision": source["config_revision"], "paused": paused})
        until(lambda: self.source(ident), lambda value: value["source"]["manual_paused"] == paused)

    def policy(self, ident, **changes):
        source = until(lambda: self.source(ident))
        policy = {**source["volcano"]["policy"], **changes}
        self.operation("PUT", f"/sources/{ident}/volcano-policy", {
            "expected_revision": source["source"]["config_revision"],
            "expected_runtime_revision": source["volcano"]["runtime"]["revision"], "policy": policy})
        until(lambda: self.source(ident), lambda value: value["volcano"]["policy"] == policy)

    def clear(self, ident):
        source = until(lambda: self.source(ident))
        previous_runtime = source["volcano"]["runtime"]["revision"]
        self.operation("POST", f"/sources/{ident}/clear-error", {
            "expected_revision": source["source"]["config_revision"],
            "expected_runtime_revision": previous_runtime})
        until(lambda: self.source(ident), lambda value: value["volcano"]["runtime"]["revision"] > previous_runtime)

    def usage(self, **filters):
        now = int(time.time())
        return self.call("POST", "/usage/query", {"from": now - 86400, "to": now + 1,
            "utc_offset_minutes": 480, "grain": "hour", "filter": {
                key: {"mode": "exact", "value": value} for key, value in filters.items()}})


def free_port():
    with socket.socket() as listener:
        listener.bind(("127.0.0.1", 0))
        port = listener.getsockname()[1]
    ensure(port not in (3000, 3001), "nonproduction_port")
    return port


def inference(port, key, claude=False, stream=False, model=MODELS[0], group="outside"):
    conn = http.client.HTTPConnection("127.0.0.1", port, timeout=40)
    headers = {"Content-Type": "application/json"}
    headers.update({"x-api-key": key, "anthropic-version": "2023-06-01"} if claude else {"Authorization": "Bearer " + key})
    body = {"model": model, "max_tokens": 16, "stream": stream,
            "messages": [{"role": "user", "content": "synthetic integration"}], "group": group}
    if stream and not claude:
        body["stream_options"] = {"include_usage": True}
    try:
        conn.request("POST", "/v1/messages" if claude else "/v1/chat/completions", json.dumps(body), headers)
        response = conn.getresponse()
        return response.status, response.read(1024 * 1024)
    finally:
        conn.close()


def cleanup(report, app, native_pid, target, upstream):
    # 一项回收失败不能跳过其余资源；身份不符宁可报失败，也不发裸PID信号。
    clean = True
    try:
        stop_owned(app)
        clean = app is None or app.poll() == 0
    except Exception:
        clean = False
    report["isolated_controller_stopped"] = app is None or app.poll() is not None
    try:
        if native_pid is None and (target / "native/new-api.pid").is_file():
            value = (target / "native/new-api.pid").read_text().strip()
            if value.isdecimal():
                native_pid = int(value)
        if native_pid is not None:
            proc = Path(f"/proc/{native_pid}")
            try:
                descriptor = os.pidfd_open(native_pid)
            except ProcessLookupError:
                descriptor = None
            if descriptor is not None:
                try:
                    if (proc / "exe").exists() and (proc / "exe").resolve() == (target / "native" / "new-api-v1.0.0-rc.20").resolve() and (proc / "cwd").resolve() == (target / "native").resolve():
                        signal.pidfd_send_signal(descriptor, signal.SIGTERM)
                        deadline = time.monotonic() + 10
                        while (proc / "exe").exists() and time.monotonic() < deadline:
                            time.sleep(0.05)
                finally:
                    os.close(descriptor)
            report["isolated_native_stopped"] = not (proc / "exe").exists()
        else:
            report["isolated_native_stopped"] = True
        clean = clean and report["isolated_native_stopped"]
    except Exception:
        report["isolated_native_stopped"] = False
        clean = False
    try:
        upstream.shutdown()
    except Exception:
        clean = False
    finally:
        try:
            upstream.server_close()
        except Exception:
            clean = False
    if not clean:
        report["success"] = False
        report["cleanup_error"] = "isolated_cleanup_incomplete"


def run(native_binary, app_binary, browser):
    work = Path(tempfile.mkdtemp(prefix="quota-throttle-sgm7-combination-"))
    api_port, panel_port = free_port(), free_port()
    while panel_port == api_port:
        panel_port = free_port()
    upstream = ThreadingHTTPServer(("127.0.0.1", 0), Upstream)
    upstream.daemon_threads = True
    upstream.hits, upstream.models_read, upstream.failures, upstream.percentages = [], [], set(), {}
    threading.Thread(target=upstream.serve_forever, daemon=True).start()
    upstream_base = f"http://127.0.0.1:{upstream.server_port}"
    report = {"directory": str(work), "checks": [], "scope": "full" if browser else "runtime_only", "success": False}
    app, native_pid = None, None
    target = work / "instance"
    panel = Panel(panel_port)
    stage = "initialize"

    def mark(name):
        ensure(name in CHECKS and name not in report["checks"], "check_manifest")
        report["checks"].append(name)
        print(f"PASS {name}", flush=True)

    def start_app():
        nonlocal app
        with (work / "app.log").open("ab") as log:
            process = subprocess.Popen([str(target / "bin/quota-throttle"), "serve", str(target / "config.toml")],
                cwd=target, stdin=subprocess.DEVNULL, stdout=log, stderr=log, start_new_session=True)
        app = process  # 保留句柄后才等待ready，启动失败也由finally回收。
        def state():
            if process.poll() is not None:
                raise RuntimeError("app_exited_before_ready")
            return panel.state()
        until(state, lambda value: value["backend_ready"])
        ensure(process.poll() is None, "app_exited")
        return process

    def source(label, kind):
        settings = {"type": kind, "upstream": {"channel_type": 8,
            "base_url": f"{upstream_base}/{label}/chat/completions",
            "model_discovery": {"url": f"{upstream_base}/{label}/models", "auth": "bearer"}}}
        if kind == "glm":
            settings["quota"] = {"quota_url": f"{upstream_base}/{label}/quota?type=2"}
        result = panel.call("POST", "/sources", {"name": label, "credential": f"SYNTHETIC_{label}_ONLY", "settings": settings})
        ensure(result["model_origin"] == "discovered", "model_list_authority")
        return result["source"]["source_id"], settings

    def group(label, kind, sources):
        created = panel.call("POST", "/groups", {"name": label, "newapi_group": label, "type": kind})["group"]
        gid = created["group_id"]
        members = []
        for sid in sources:
            mid = panel.call("POST", "/memberships", {"source_id": sid, "group_id": gid})["membership"]["membership_id"]
            item = until(lambda: panel.member(mid))
            panel.operation("POST", f"/memberships/{mid}/ensure", {"expected_revision": item["control_revision"]}, wait=False)
            until(lambda: panel.member(mid), lambda value: value["membership"]["channel_id"] is not None)
            members.append(mid)
        ensure(not panel.group(gid)["group"]["ready"], "group_not_ready_before_registration")
        for mid in members:
            channel = panel.member(mid)["membership"]["channel_id"]
            ensure(root.api("GET", f"/api/channel/{channel}")["status"] == 2, "native_created_disabled")
        # No fabricated ready/permit/SQL: disabled creation must bootstrap the native group.
        deadline = time.monotonic() + 180
        while True:
            current = until(lambda: panel.group(gid))["group"]
            status, checked = panel.raw("POST", f"/groups/{gid}/check", {"expected_revision": current["revision"], "register": True})
            if status == 200 and checked["ready"]:
                break
            print("GROUP CHECK", gid, status, checked.get("error", checked.get("error_code")), flush=True)
            ensure(time.monotonic() < deadline and checked.get("error", checked.get("error_code"))
                   in ("queue_full", "revision_conflict", "not_ready", "transport_unknown"), "group_bootstrap")
            time.sleep(1)
        for mid in members:
            until(lambda: panel.member(mid), lambda value: value["membership"]["observed_status"] == 1 and not value["dirty"])
        return gid, members

    def token(gid, name):
        current = until(lambda: panel.group(gid))["group"]
        receipt = panel.operation("POST", "/tokens", {"group_id": gid, "expected_group_revision": current["revision"], "display_name": name, "expires_at": None})
        tid = receipt["target_id"]
        identity = panel.call("GET", f"/tokens/{tid}")
        until(panel.state, lambda value: value["backend_ready"])
        key = panel.call("POST", f"/tokens/{tid}/reveal", {"expected_revision": identity["revision"]})["key"]
        return {"id": tid, "native": identity["native_token_id"], "key": key}

    try:
        initialize_fresh(target, native_binary, app_binary, api_port, panel_port, "isolated-working-source")
        stage = "start_serve"
        app = start_app()
        native_pid = int((target / "native/new-api.pid").read_text())
        mark("fresh_start_and_real_backend")
        credentials = tomllib.loads((target / "config.toml").read_text())
        root = Native(api_port)
        root.login("qtadmin", credentials["new_api"]["root_password"], 100)
        stage = "catalog_and_groups"
        a, settings = source("volcano-A", "volcano")
        b, _ = source("volcano-B", "volcano")
        c, _ = source("glm-C", "glm")
        d, _ = source("glm-D", "glm")
        status, duplicate = panel.raw("POST", "/sources", {"name": "duplicate", "credential": "SYNTHETIC_volcano-A_ONLY", "settings": settings})
        ensure(status == 409 and duplicate["existing_source_id"] == a, "credential_dedup")
        ensure(set(upstream.models_read) >= {"volcano-A", "volcano-B", "glm-C", "glm-D"}, "real_models_requests")
        mark("model_discovery_and_no_duplicate_source")
        g1, m1 = group("qt-volcano-one", "volcano", [a, b])
        g2, m2 = group("qt-volcano-two", "volcano", [b, a])
        gg, mg = group("qt-glm", "glm", [c, d])
        mark("disabled_creation_then_group_check")
        # 启动/注册时观察可能暂失新鲜度，原游标恢复会轮转；不假定首选永远是首个成员。
        # 仍必须证明两组对共享A/B保持不同首选及独立原生渠道，然后固定后续轨迹角色。
        def preferences():
            snapshot = panel.state()["catalog"]
            members = {m["membership"]["membership_id"]: m["membership"] for m in snapshot["memberships"]}
            groups = {g["group"]["group_id"]: g for g in snapshot["groups"]}
            selected = {gid: members.get(groups[gid]["selected_member_id"], {}).get("source_id") for gid in (g1, g2)}
            converged = all(groups[gid]["group"]["ready"] for gid in (g1, g2)) and all(
                not m["dirty"] and m["membership"]["observed_status"] == 1
                and m["membership"]["observed_priority"] == m["membership"]["desired_priority"]
                for m in snapshot["memberships"] if m["membership"]["group_id"] in (g1, g2))
            return selected, members, groups, converged
        selected, members, groups, _ = until(preferences, lambda value: value[3] and set(value[0].values()) == {a, b})
        if selected[g1] == b:
            g1, g2 = g2, g1
        m1 = [next(m["membership_id"] for m in members.values() if m["group_id"] == g1 and m["source_id"] == sid) for sid in (a, b)]
        m2 = [next(m["membership_id"] for m in members.values() if m["group_id"] == g2 and m["source_id"] == sid) for sid in (b, a)]
        ensure(groups[g1]["selected_member_id"] == m1[0] and groups[g2]["selected_member_id"] == m2[0], "independent_preferred_sources")
        ensure(len({panel.member(mid)["membership"]["channel_id"] for mid in m1 + m2}) == 4, "independent_native_channels")
        mark("independent_shared_group_priority")
        stage = "rejected_controls"
        control_source, _ = source("control-fixture", "volcano")
        old_revision = until(lambda: panel.source(control_source))["source"]["config_revision"]
        panel.pause(control_source, True)  # 无成员来源的真实修改，取得确实过时而非未来的revision。
        original = panel.source(control_source)["source"]
        ensure(original["config_revision"] > old_revision, "actual_old_revision")
        native_channel = panel.member(m1[0])["membership"]["channel_id"]
        native_before = root.api("GET", f"/api/channel/{native_channel}")
        writes = lambda: sum("[GIN]" in line and ("PUT /api/channel/" in line or "POST /api/channel/" in line)
                             for line in (target / "native/new-api.log").read_text().splitlines())
        before_writes = writes()
        for invalid in (False, True):
            op = "qt_" + str(uuid.uuid4())
            body = {"operation_id": op, "expected_revision": original["config_revision"] if invalid else old_revision, "paused": False}
            if invalid:
                body["unrecognized_privilege"] = True
            status, rejected = panel.raw("PUT", f"/sources/{control_source}/paused", body)
            ensure(status == (400 if invalid else 409), "invalid_or_stale_rejected")
            ensure(panel.raw("GET", f"/operations/{op}")[0] == 404, "rejected_operation_not_persisted")
        ensure(panel.source(control_source)["source"] == original and writes() == before_writes, "rejected_commands_no_write")
        native_after = root.api("GET", f"/api/channel/{native_channel}")
        ensure(all(native_before[k] == native_after[k] for k in ("id", "group", "priority", "status")), "rejected_native_unchanged")
        mark("stale_and_invalid_commands_write_nothing")
        stage = "token_creation"
        one, two, glm = token(g1, "consumer-one"), token(g2, "consumer-two"), token(gg, "consumer-glm")
        stage = "dual_protocol"
        for client, expected in [(one, "volcano-A"), (two, "volcano-B")]:
            for claude in (False, True):
                for stream in (False, True):
                    before = len(upstream.hits)
                    status, raw = inference(api_port, client["key"], claude, stream, MODELS[int(stream)])
                    ensure(status == 200 and upstream.hits[before:] == [(expected, MODELS[int(stream)])], "dual_protocol_source")
                    if stream:
                        ensure((b"message_stop" if claude else b"[DONE]") in raw, "stream_completion")
        mark("same_key_both_protocols_and_streams")
        for claude in (False, True):
            before = len(upstream.hits)
            outside = panel.member(mg[0])["membership"]["channel_id"]
            ensure(inference(api_port, one["key"] + "-" + outside, claude)[0] >= 400, "specified_channel_rejected")
            ensure(inference(api_port, one["key"], claude, model="model-outside-catalog", group="qt-glm")[0] >= 400, "unknown_model_rejected")
            ensure(len(upstream.hits) == before, "isolation_no_upstream")
        mark("ordinary_owner_group_model_isolation")
        stage = "usage"
        usage = until(lambda: panel.usage(), lambda value: value["usage"] is not None and value["usage"]["total"]["consumption_rows"] >= 8)
        ensure({row["model"] for row in usage["usage"]["models"]} == set(MODELS), "all_models_present")
        total = usage["usage"]["total"]["total"]
        ensure(sum(row["usage"]["total"] for row in usage["usage"]["models"]) == total
               and sum(row["usage"]["total"] for row in usage["usage"]["tokens"]) == total, "joint_marginals")
        native_rows = root.api("GET", "/api/log/?p=1&page_size=100")["items"]
        expected = sum(row["prompt_tokens"] + row["completion_tokens"] for row in native_rows if row["type"] == 2)
        ensure(total == expected, "native_log_total_matches_ledger")
        mark("real_logs_and_model_token_marginals")
        stage = "observed_breaker"
        upstream.failures.add("volcano-A")
        ensure(inference(api_port, one["key"])[0] == 200, "native_retry_fallback")
        until(lambda: panel.source(a), lambda value: value["volcano"]["runtime"]["consecutive_failures"] >= 1)
        failed_period = panel.source(a)["volcano"]["runtime"]["period_id"]
        upstream.failures.remove("volcano-A")
        before = len(upstream.hits)
        ensure(inference(api_port, one["key"])[0] == 200 and upstream.hits[before:] == [("volcano-A", MODELS[0])], "recovered_inference")
        until(lambda: panel.source(a), lambda value: value["volcano"]["runtime"]["consecutive_failures"] == 0)
        ensure(panel.source(a)["volcano"]["runtime"]["period_id"] == failed_period, "success_not_period_reset")
        mark("success_token_flow_clears_failures")
        upstream.failures.add("volcano-A")
        for _ in range(3):
            ensure(inference(api_port, one["key"])[0] == 200, "failure_fallback")
        until(lambda: panel.source(a), lambda value: value["volcano"]["runtime"]["error_exhausted"])
        panel.settled_group(g1, m1[1])
        panel.settled_group(g2, m2[0])
        panel.clear(a)
        upstream.failures.remove("volcano-A")
        until(lambda: panel.source(a), lambda value: not value["volcano"]["runtime"]["error_exhausted"])
        mark("shared_error_exhaustion_and_manual_clear")
        stage = "token_limit_and_pause"
        runtime = until(lambda: panel.source(b))["volcano"]["runtime"]
        panel.policy(b, daily_token_limit=runtime["used_input"] + runtime["used_output"] + 1)
        ensure(inference(api_port, two["key"])[0] == 200, "soft_limit_final_request")
        until(lambda: panel.source(b), lambda value: value["volcano"]["runtime"]["limit_exhausted"])
        panel.settled_group(g1, m1[0])
        panel.settled_group(g2, m2[1])
        mark("shared_soft_token_limit")
        panel.pause(b, True)
        panel.clear(b)
        ensure(panel.source(b)["volcano"]["runtime"]["limit_exhausted"], "error_clear_does_not_clear_token_limit")
        reset = ((int(time.time()) // 60) + 480 + 2) % 1440
        panel.policy(b, reset_minute=reset)
        before_reset = panel.source(b)["volcano"]["runtime"]
        until(lambda: panel.source(b), lambda value: int(time.time()) >= before_reset["next_reset_at"]
              and value["volcano"]["runtime"]["period_id"] > before_reset["period_id"], seconds=200)
        ensure(panel.source(b)["source"]["manual_paused"], "reset_preserves_manual_pause")
        panel.pause(a, True)
        before = len(upstream.hits)
        for claude in (False, True):
            ensure(inference(api_port, one["key"], claude, group="qt-glm")[0] >= 400, "whole_group_outage")
        ensure(len(upstream.hits) == before, "outage_never_crosses_group")
        panel.pause(a, False)
        mark("pause_survives_error_clear_and_reset")
        stage = "glm_policy"
        panel.settled_group(gg, mg[0])
        before = len(upstream.hits)
        ensure(inference(api_port, glm["key"])[0] == 200 and upstream.hits[before:] == [("glm-C", MODELS[0])], "glm_initial_source")
        upstream.percentages["glm-C"] = 96
        upstream.percentages["glm-D"] = 10
        panel.settled_group(gg, mg[1])
        before = len(upstream.hits)
        ensure(inference(api_port, glm["key"])[0] == 200 and upstream.hits[before:] == [("glm-D", MODELS[0])], "glm_95_switch")
        mark("glm_95_policy_unchanged")
        stage = "token_lifecycle"
        for enabled in (False, True):
            identity = panel.call("GET", f"/tokens/{one['id']}")
            panel.operation("PUT", f"/tokens/{one['id']}/enabled", {"expected_revision": identity["revision"], "enabled": enabled})
            ensure((inference(api_port, one["key"])[0] == 200) == enabled, "token_enabled_state")
        identity = panel.call("GET", f"/tokens/{one['id']}")
        panel.operation("DELETE", f"/tokens/{one['id']}", {"expected_revision": identity["revision"]})
        historical = panel.call("GET", f"/tokens/{one['id']}")
        ensure(historical["deleted_at"] is not None and historical["native_token_id"] == one["native"], "deleted_identity_preserved")
        before = len(upstream.hits)
        ensure(inference(api_port, one["key"])[0] >= 400 and len(upstream.hits) == before, "deleted_key_rejected")
        until(lambda: panel.usage(native_token_id=one["native"]), lambda value: value["usage"] is not None and value["usage"]["total"]["total"] > 0)
        mark("token_enable_disable_delete_and_history")
        stage = "restart"
        time.sleep(20)
        before = panel.usage()["usage"]["total"]
        app.send_signal(signal.SIGTERM)
        ensure(app.wait(timeout=90) == 0, "app_graceful_shutdown")
        ensure(Path(f"/proc/{native_pid}/exe").exists(), "native_survives_controller_stop")
        app = start_app()
        ensure(int((target / "native/new-api.pid").read_text()) == native_pid, "native_not_restarted")
        until(lambda: panel.usage(), lambda value: value["usage"] is not None and value["usage"]["total"] == before)
        time.sleep(12)
        ensure(panel.usage()["usage"]["total"] == before, "replay_does_not_duplicate_usage")
        ensure(panel.call("GET", f"/tokens/{one['id']}")["native_token_id"] == one["native"], "restart_identity")
        mark("restart_preserves_identity_and_usage")
        if browser:
            stage = "real_browser"
            result = subprocess.run(["node", "scripts/tests/groups-browser.mjs", panel.base, upstream_base, str(work)],
                cwd=Path(__file__).resolve().parents[2], timeout=180, check=False)
            ensure(result.returncode == 0, "real_browser")
            report["browser"] = json.loads((work / "browser-report.json").read_text())
            ensure(report["browser"]["expected"] == 4 and report["browser"]["passed"] == 4
                   and report["browser"]["apiMocks"] is False, "full_browser_manifest")
        ensure(report["checks"] == CHECKS, "complete_check_manifest")
        report["success"] = True
    except Exception as error:
        report["stage"] = stage
        report["error"] = str(error) if isinstance(error, (ProbeError, BootstrapError)) else type(error).__name__
    finally:
        cleanup(report, app, native_pid, target, upstream)
        (work / "report.json").write_text(json.dumps(report, indent=2))
    print(json.dumps(report, indent=2))
    return 0 if report["success"] else 1


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--newapi-binary", required=True, type=Path)
    parser.add_argument("--app-binary", required=True, type=Path)
    parser.add_argument("--browser", action="store_true")
    args = parser.parse_args()
    cancelling = False
    def cancel(_signal, _frame):
        global cancelling
        if not cancelling:
            cancelling = True
            raise ProbeError("probe_cancelled")
    previous = {sig: signal.signal(sig, cancel) for sig in (signal.SIGINT, signal.SIGTERM, signal.SIGHUP)}
    try:
        raise SystemExit(run(args.newapi_binary.resolve(strict=True), args.app_binary.resolve(strict=True), args.browser))
    finally:
        for sig, handler in previous.items():
            signal.signal(sig, handler)
