#!/usr/bin/env python3
"""SGM-8 T-B13: explicit pinned rc.20 binary, fresh private synthetic instance only.

python3 scripts/probes/newapi_balance_probe.py --binary /absolute/new-api
No download, existing service/config/database access, or production credentials.
"""
import argparse
import hashlib
import json
import math
import os
from pathlib import Path
import re
import secrets
import shutil
import signal
import socket
import subprocess
import tempfile
import time

from newapi_shared_source_probe import Client

SHA256 = "90c333eb7ace489d46e25cb687767c25c3e28584ce80384f111c03ff5f5b1205"
# 固定rc.20二进制内auth.insufficient_privilege的英文/中文文案。
# 不把未登录、错误身份头或任意非空错误误当普通用户的权限拒绝。
PERMISSION_DENIALS = frozenset({
    "Unauthorized, insufficient privileges", "无权进行此操作，权限不足",
})
CHECKS = [
    "positive_add_is_acknowledged",
    "nonpositive_values_are_rejected",
    "ordinary_owner_is_denied_with_correct_identity",
    "missing_positive_id_acknowledges_without_changing_accounts",
    "zero_id_targets_root",
    "status_exposes_valid_quota_unit",
    "root_can_add_to_itself",
    "ack_is_immediately_visible_in_profile",
    "json_integer_above_int64_is_rejected",
    "invalid_mode_is_rejected",
]


class ProbeError(Exception):
    pass


def ensure(condition, label):
    if not condition:
        raise ProbeError(label)


def private_write(path, value):
    descriptor = os.open(path, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
    with os.fdopen(descriptor, "w") as handle:
        json.dump(value, handle)
    ensure(path.stat().st_mode & 0o777 == 0o600, "private_file_mode")


def owned_listener(process, binary, work, port):
    ensure(process.poll() is None, "isolated_process_exited")
    proc = Path(f"/proc/{process.pid}")
    ensure(proc.stat().st_uid == os.getuid(), "isolated_process_owner")
    ensure((proc / "exe").resolve(strict=True) == binary, "isolated_process_executable")
    ensure((proc / "cwd").resolve(strict=True) == work, "isolated_process_directory")
    listening = subprocess.check_output(
        ["ss", "-H", "-ltnp", f"sport = :{port}"], text=True,
        stderr=subprocess.DEVNULL,
    )
    return f"pid={process.pid}," in listening and (work / "test.db").is_file()


def exercise(root, report):
    def mark(name):
        ensure(name in CHECKS and name not in report["checks"], "check_manifest")
        report["checks"].append(name)

    report["stage"] = "create_ordinary_account"
    # rc.20 User校验要求password 8..20字符、display_name至多20字符。
    ordinary_password = secrets.token_hex(9)
    root.api("POST", "/api/user/", {
        "username": "qtfixture", "password": ordinary_password,
        "display_name": "balance fixture", "role": 1,
    })
    ordinary = Client(root.base)
    report["stage"] = "login_ordinary_account"
    ordinary.user = ordinary.api("POST", "/api/user/login", {
        "username": "qtfixture", "password": ordinary_password,
    })["id"]
    ensure(type(ordinary.user) is int and ordinary.user > 0 and ordinary.user != root.user,
           "distinct_positive_owner_ids")
    report["stage"] = "verify_ordinary_account"
    profile = ordinary.api("GET", "/api/user/self")
    ensure(profile["id"] == ordinary.user and profile["role"] == 1 and profile["status"] == 1,
           "ordinary_owner_profile")

    def balances():
        values = tuple(client.api("GET", "/api/user/self")["quota"] for client in (root, ordinary))
        ensure(all(type(value) is int for value in values), "integer_profile_balances")
        return values

    def manage(client, ident, value, success, mode="add"):
        status, body = client.request("POST", "/api/user/manage", {
            "id": ident, "action": "add_quota", "mode": mode, "value": value,
        })
        ensure(status == 200 and isinstance(body, dict) and body.get("success") is success,
               "manage_response_contract")
        return body

    report["stage"] = CHECKS[0]
    before = balances()
    manage(root, ordinary.user, 17, True)
    ensure(balances() == (before[0], before[1] + 17), "positive_add_exact_effect")
    mark("positive_add_is_acknowledged")

    report["stage"] = CHECKS[1]
    before = balances()
    for value in (0, -1):
        manage(root, ordinary.user, value, False)
        ensure(balances() == before, "nonpositive_add_no_effect")
    mark("nonpositive_values_are_rejected")

    report["stage"] = CHECKS[2]
    # Client supplies the logged-in ordinary user's own New-Api-User header.
    denied = manage(ordinary, ordinary.user, 19, False)
    message = denied.get("message")
    ensure(isinstance(message, str) and message in PERMISSION_DENIALS,
           "ordinary_permission_explanation")
    ensure(balances() == before, "ordinary_add_no_effect")
    mark("ordinary_owner_is_denied_with_correct_identity")

    report["stage"] = CHECKS[3]
    users = root.api("GET", "/api/user/?p=1&page_size=100")
    rows = users if isinstance(users, list) else users["items"]
    ensure({row["id"] for row in rows} == {root.user, ordinary.user}, "only_synthetic_accounts")
    manage(root, 2**63 - 1, 23, True)
    ensure(balances() == before, "missing_id_no_account_effect")
    mark("missing_positive_id_acknowledges_without_changing_accounts")

    report["stage"] = CHECKS[4]
    manage(root, 0, 29, True)
    ensure(balances() == (before[0] + 29, before[1]), "zero_id_exact_root_effect")
    mark("zero_id_targets_root")

    report["stage"] = CHECKS[5]
    unit = root.api("GET", "/api/status")
    ratio, display = unit.get("quota_per_unit"), unit.get("quota_display_type")
    ensure(type(ratio) in (int, float) and math.isfinite(ratio) and ratio > 0
           and isinstance(display, str) and re.fullmatch(r"[A-Za-z0-9]{1,16}", display) is not None,
           "valid_status_quota_unit")
    mark("status_exposes_valid_quota_unit")

    report["stage"] = CHECKS[6]
    before = balances()
    manage(root, root.user, 31, True)
    ensure(balances() == (before[0] + 31, before[1]), "root_self_add_exact_effect")
    mark("root_can_add_to_itself")

    report["stage"] = CHECKS[7]
    before = ordinary.api("GET", "/api/user/self")["quota"]
    manage(root, ordinary.user, 37, True)
    # The first request after ACK is this profile read: no polling or sleep.
    after = ordinary.api("GET", "/api/user/self")["quota"]
    ensure(after == before + 37, "ack_immediate_profile_exact_effect")
    mark("ack_is_immediately_visible_in_profile")

    report["stage"] = CHECKS[8]
    before = balances()
    manage(root, ordinary.user, 2**63, False)
    ensure(balances() == before, "int64_overflow_no_effect")
    mark("json_integer_above_int64_is_rejected")

    report["stage"] = CHECKS[9]
    manage(root, ordinary.user, 41, False, mode="invalid")
    ensure(balances() == before, "invalid_mode_no_effect")
    mark("invalid_mode_is_rejected")
    ensure(report["checks"] == CHECKS, "complete_check_manifest")


def run(binary):
    report = {"binary_sha256": SHA256, "checks": [], "success": False, "stage": "verify_binary"}
    process = None
    old_mask = os.umask(0o077)
    try:
        with binary.open("rb") as handle:
            ensure(hashlib.file_digest(handle, "sha256").hexdigest() == SHA256, "pinned_rc20_sha256")
        shred = shutil.which("shred")
        ensure(shred is not None and shutil.which("ss") is not None, "cleanup_and_ownership_tools_required")
        report["stage"] = "create_private_instance"
        with tempfile.TemporaryDirectory(prefix="quota-throttle-balance-native-") as directory:
            work = Path(directory).resolve()
            ensure(work.stat().st_mode & 0o777 == 0o700, "private_directory_mode")
            with socket.socket() as reserve:
                reserve.bind(("127.0.0.1", 0))
                port = reserve.getsockname()[1]
            ensure(port not in (3000, 3001), "nonproduction_port")
            credentials = work / "credentials.json"
            password, session = secrets.token_hex(16), secrets.token_hex(24)
            private_write(credentials, {"password": password, "session_secret": session})
            env = {
                "PATH": os.defpath, "PORT": str(port), "GIN_MODE": "release", "GOMAXPROCS": "2",
                "SQLITE_PATH": str(work / "test.db"), "SQL_DSN": "local", "LOG_SQL_DSN": "local",
                "SESSION_SECRET": session, "GLOBAL_API_RATE_LIMIT_ENABLE": "false",
                "CRITICAL_RATE_LIMIT_ENABLE": "false", "SEARCH_RATE_LIMIT_ENABLE": "false",
            }
            try:
                report["stage"] = "start_isolated_process"
                with (work / "server.log").open("xb") as log:
                    process = subprocess.Popen(
                        [str(binary)], cwd=work, env=env, stdin=subprocess.DEVNULL,
                        stdout=log, stderr=log, start_new_session=True,
                    )
                report["stage"] = "verify_owned_listener"
                deadline = time.monotonic() + 30
                while not owned_listener(process, binary, work, port):
                    ensure(time.monotonic() < deadline, "isolated_listener_deadline")
                    time.sleep(0.1)
                root = Client(f"http://127.0.0.1:{port}")
                report["stage"] = "setup_root_account"
                root.api("POST", "/api/setup", {
                    "username": "qtbalance", "password": password,
                    "confirmPassword": password, "SelfUseModeEnabled": True,
                })
                report["stage"] = "login_root_account"
                root.user = root.api("POST", "/api/user/login", {
                    "username": "qtbalance", "password": password,
                })["id"]
                ensure(type(root.user) is int and root.user > 0, "positive_root_id")
                exercise(root, report)
                report["success"] = True
                report["stage"] = "complete"
            finally:
                # Only the Popen handle we created is eligible for signals.
                try:
                    if process is not None and process.poll() is None:
                        process.terminate()
                        try:
                            process.wait(timeout=5)
                        except subprocess.TimeoutExpired:
                            process.kill()
                            process.wait(timeout=5)
                finally:
                    report["isolated_process_stopped"] = process is None or process.poll() is not None
                    # Credential file and any native session/database/log material stay private,
                    # are shredded before TemporaryDirectory removes this owned directory.
                    for path in work.rglob("*"):
                        if path.is_file() and not path.is_symlink():
                            subprocess.run([shred, "--remove=unlink", "--", str(path)], check=True,
                                           stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
                    report["private_material_removed"] = True
    except Exception as error:
        report["success"] = False
        report["error"] = str(error) if isinstance(error, ProbeError) else type(error).__name__
    finally:
        os.umask(old_mask)
    print(json.dumps(report, ensure_ascii=False, indent=2))
    return 0 if report["success"] else 1


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", required=True, type=Path)
    args = parser.parse_args()
    cancelling = False

    def cancel(_signal, _frame):
        global cancelling
        if not cancelling:
            cancelling = True
            raise ProbeError("probe_cancelled")

    previous = {sig: signal.signal(sig, cancel) for sig in (signal.SIGINT, signal.SIGTERM, signal.SIGHUP)}
    try:
        raise SystemExit(run(args.binary.resolve(strict=True)))
    finally:
        for sig, handler in previous.items():
            signal.signal(sig, handler)
