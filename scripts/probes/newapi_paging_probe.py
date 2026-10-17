#!/usr/bin/env python3
"""SGM-4 B2 隔离分页探针；只操作本次自建进程和临时 SQLite。

显式指定固定 binary；不读取生产配置/数据库/日志/凭据。
停止自建进程后种合成数据，再通过管理员 API 验证，不是生产 DB 采集器。
"""
import argparse
import hashlib
import json
import os
from pathlib import Path
import secrets
import socket
import sqlite3
import subprocess
import tempfile
import time
import urllib.error

from newapi_shared_source_probe import Client

SHA256 = "90c333eb7ace489d46e25cb687767c25c3e28584ce80384f111c03ff5f5b1205"
BASE_TIME = 1700000000


def run(binary, scale=False):
    assert hashlib.file_digest(binary.open("rb"), "sha256").hexdigest() == SHA256
    work = Path(tempfile.mkdtemp(prefix="quota-throttle-paging-probe-"))
    with socket.socket() as reserve:
        reserve.bind(("127.0.0.1", 0))
        port = reserve.getsockname()[1]
    assert port not in (3000, 3001)
    database = work / "one-api.db"
    assert not database.exists()
    # 与初始环境相冲突，实测 Load 不会覆盖显式 local。
    (work / ".env").write_text(
        "SQL_DSN=postgres://invalid:invalid@127.0.0.1:1/invalid\n"
        "LOG_SQL_DSN=clickhouse://127.0.0.1:1/invalid\n")
    env = {"PATH": os.defpath, "PORT": str(port), "GIN_MODE": "release",
           "GOMAXPROCS": "2", "SQLITE_PATH": str(database), "SQL_DSN": "local",
           "LOG_SQL_DSN": "local", "SESSION_SECRET": secrets.token_hex(24),
           "GLOBAL_API_RATE_LIMIT_ENABLE": "false", "CRITICAL_RATE_LIMIT_ENABLE": "false"}
    api = Client(f"http://127.0.0.1:{port}")
    process = None
    result = {"directory": str(work), "port": port, "checks": [], "scale": []}
    stage = "startup"
    with (work / "server.log").open("wb") as log:
        def owned():
            assert process is not None and process.poll() is None, "probe process exited"
            lines = subprocess.check_output(
                ["ss", "-H", "-ltnp", f"sport = :{port}"], text=True).strip().splitlines()
            assert len(lines) == 1 and lines[0].count("pid=") == 1
            assert f"pid={process.pid}," in lines[0], "listener not owned by probe"

        def start():
            nonlocal process
            process = subprocess.Popen([str(binary)], cwd=work, env=env,
                stdin=subprocess.DEVNULL, stdout=log, stderr=log, start_new_session=True)
            deadline = time.monotonic() + 30
            while time.monotonic() < deadline:
                assert process.poll() is None, "probe process exited"
                try:
                    if api.request("GET", "/api/status")[0] == 200:
                        owned()
                        return
                except (OSError, urllib.error.URLError):
                    pass
                time.sleep(0.1)
            raise AssertionError("probe startup timed out")

        def stop():
            if process is not None and process.poll() is None:
                process.terminate()
                try:
                    process.wait(timeout=10)
                except subprocess.TimeoutExpired:
                    process.kill()
                    process.wait(timeout=5)

        def seed(count, timestamp, first_id):
            assert process.poll() is not None, "must stop own process before fixture write"
            assert database.parent == work and database.is_file() and not database.is_symlink()
            with sqlite3.connect(database) as db:
                assert db.execute("SELECT count(*) FROM users WHERE id=?", (api.user,)).fetchone()[0] == 1
                db.executemany(
                    "INSERT INTO logs (id,user_id,created_at,type,content,username,token_name,"
                    "model_name,quota,prompt_tokens,completion_tokens,use_time,is_stream,"
                    "channel_id,token_id,`group`,ip,request_id,upstream_request_id,other) "
                    "VALUES (?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?)",
                    ((first_id+i, api.user, timestamp+i//100, (2, 5, 3)[i % 3], "", "", "",
                      "probe-model", 0, 1 if i % 3 == 0 else 0, 2 if i % 3 == 0 else 0,
                      0, 0, 0, 0, "default", "", f"probe_{first_id+i}", "", "")
                     for i in range(count)))

        def page(number, start_at, end_at, user=False):
            path = "/api/log/self" if user else "/api/log/"
            data = api.api("GET", f"{path}?type=0&p={number}&page_size=100"
                           f"&start_timestamp={start_at}&end_timestamp={end_at}")
            assert data["page"] == number and data["page_size"] == 100
            return data

        try:
            start()
            owned()
            password = secrets.token_urlsafe(20)
            api.api("POST", "/api/setup", {"username": "pagingaudit", "password": password,
                    "confirmPassword": password, "SelfUseModeEnabled": True})
            api.user = api.api("POST", "/api/user/login",
                              {"username": "pagingaudit", "password": password})["id"]
            stop()
            stage = "fixture"
            seed(300, BASE_TIME, 1000000)
            seed(1, BASE_TIME - 1, 2000000)
            seed(1, BASE_TIME + 3, 2000001)
            start()
            stage = "paging"
            pages = [page(n, BASE_TIME, BASE_TIME + 2) for n in range(1, 5)]
            assert [len(p["items"]) for p in pages] == [100, 100, 100, 0]
            assert all(p["total"] == 300 for p in pages)
            rows = [row for p in pages for row in p["items"]]
            assert [r["id"] for r in rows] == list(range(1000299, 999999, -1))
            assert {r["type"] for r in rows} == {2, 3, 5}
            assert [(r["created_at"], r["id"]) for r in rows] == sorted(
                [(r["created_at"], r["id"]) for r in rows], reverse=True)
            assert page(1, BASE_TIME, BASE_TIME + 2)["items"] == pages[0]["items"]
            result["checks"] += ["three_full_pages_and_empty_tail", "mixed_types_native_ids_stable",
                                 "time_then_id_descending"]
            same_second = page(1, BASE_TIME + 1, BASE_TIME + 1)
            assert same_second["total"] == 100
            assert {r["created_at"] for r in same_second["items"]} == {BASE_TIME + 1}
            assert page(1, BASE_TIME - 1, BASE_TIME - 1)["items"][0]["id"] == 2000000
            assert page(1, BASE_TIME + 3, BASE_TIME + 3)["items"][0]["id"] == 2000001
            assert page(1, BASE_TIME + 4, BASE_TIME + 4)["items"] == []
            # 0 是不筛选，而不是 epoch 0；不能用 end=0 编码 [0,1)。
            assert page(1, BASE_TIME, 0)["total"] >= 301
            assert page(1, 0, BASE_TIME - 1)["total"] == 1
            result["checks"] += ["inclusive_time_endpoints", "zero_disables_timestamp_filter"]
            user = page(1, BASE_TIME, BASE_TIME + 2, user=True)
            assert [r["id"] for r in user["items"]] == list(range(1, 101))
            result["checks"] += ["user_display_ids_are_not_native_ids", "explicit_local_overrides_dotenv"]
            if scale:
                for count, timestamp, first_id in ((100000, BASE_TIME-10000, 3000000),
                                                   (200000, BASE_TIME-20000, 4000000)):
                    stop()
                    stage = f"scale_{count}"
                    seed(count, timestamp, first_id)
                    start()
                    begin = time.monotonic()
                    received = 0
                    tail_ms = None
                    for n in range(1, count//100 + 2):
                        before = time.monotonic()
                        data = page(n, timestamp, timestamp+(count-1)//100)
                        assert data["total"] == count
                        ids = [r["id"] for r in data["items"]]
                        assert ids == list(range(first_id+count-1-received,
                                                 first_id+count-1-received-len(ids), -1))
                        received += len(ids)
                        tail_ms = round((time.monotonic()-before)*1000, 2)
                    assert received == count and not data["items"]
                    rss = next(line.split()[1] for line in
                               Path(f"/proc/{process.pid}/status").read_text().splitlines()
                               if line.startswith("VmRSS:"))
                    result["scale"].append({"rows": count, "pages": count//100+1,
                        "elapsed_seconds": round(time.monotonic()-begin, 2),
                        "empty_tail_ms": tail_ms, "server_rss_kib": int(rss)})
            result["success"] = True
        except Exception as exc:
            result.update(success=False, stage=stage, error_type=type(exc).__name__)
        finally:
            stop()
            result["owned_process_stopped"] = process is None or process.poll() is not None
    (work / "result.json").write_text(json.dumps(result, indent=2) + "\n")
    print(json.dumps(result, indent=2))
    return 0 if result.get("success") else 1


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, required=True)
    parser.add_argument("--scale", action="store_true")
    args = parser.parse_args()
    raise SystemExit(run(args.binary.resolve(strict=True), args.scale))
