#!/usr/bin/env python3
"""Initialize only a new private, locally managed instance; never import old data."""
import argparse
from contextlib import contextmanager
import hashlib
import http.cookiejar
import json
import os
from pathlib import Path
import re
import secrets
import signal
import shutil
import socket
import subprocess
import time
import urllib.error
import urllib.request

NATIVE_VERSION = "v1.0.0-rc.20"
NATIVE_SHA256 = "90c333eb7ace489d46e25cb687767c25c3e28584ce80384f111c03ff5f5b1205"
UNIT = "quota-throttle-sgm.service"
CANCEL_SIGNALS = (signal.SIGINT, signal.SIGTERM, signal.SIGHUP)
_cancel_scope = None


class BootstrapError(Exception):
    """Only fixed step identifiers may be reported; no HTTP body or credentials."""


class InstallationCancelled(BootstrapError):
    pass


def require(condition, stage):
    if not condition:
        raise BootstrapError(stage)


class NoRedirect(urllib.request.HTTPRedirectHandler):
    def redirect_request(self, *_args, **_kwargs):
        return None


class Native:
    def __init__(self, port, guard=None):
        self.base = f"http://127.0.0.1:{port}"
        self.user = None
        self.guard = guard
        self.opener = urllib.request.build_opener(
            urllib.request.ProxyHandler({}), NoRedirect(),
            urllib.request.HTTPCookieProcessor(http.cookiejar.CookieJar()))

    def api(self, method, path, data=None):
        require(self.guard is None or self.guard(), "native_ownership_changed")
        headers = {"Content-Type": "application/json"}
        if self.user is not None:
            headers["New-Api-User"] = str(self.user)
        request = urllib.request.Request(self.base + path,
            None if data is None else json.dumps(data).encode(), headers, method=method)
        try:
            with self.opener.open(request, timeout=10) as response:
                raw = response.read(2 * 1024 * 1024 + 1)
                require(response.status == 200 and len(raw) <= 2 * 1024 * 1024, "native_response")
                value = json.loads(raw)
            require(self.guard is None or self.guard(), "native_ownership_changed")
            require(isinstance(value, dict) and value.get("success") is True, "native_response")
            return value.get("data")
        except (OSError, ValueError, urllib.error.URLError):
            raise BootstrapError("native_request") from None

    def login(self, name, password, role):
        reply = self.api("POST", "/api/user/login", {"username": name, "password": password})
        require(isinstance(reply, dict) and type(reply.get("id")) is int and reply["id"] > 0, "login_identity")
        self.user = reply["id"]
        profile = self.api("GET", "/api/user/self")
        require(profile.get("id") == self.user and profile.get("username") == name
                and profile.get("role") == role and profile.get("status") == 1, "owner_profile")
        return profile

    def options(self):
        rows = self.api("GET", "/api/option/")
        require(isinstance(rows, list) and len(rows) <= 10000, "options_shape")
        result = {}
        for row in rows:
            require(isinstance(row, dict) and isinstance(row.get("key"), str)
                    and row["key"] not in result and isinstance(row.get("value"), str), "options_shape")
            result[row["key"]] = row["value"]
        return result

    def set_option(self, key, value):
        self.api("PUT", "/api/option/", {"key": key, "value": value})


def digest(path):
    with Path(path).open("rb") as handle:
        return hashlib.file_digest(handle, "sha256").hexdigest()


def private_write(path, value):
    fd = os.open(path, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
    with os.fdopen(fd, "w") as handle:
        handle.write(value)
        handle.flush()
        os.fsync(handle.fileno())
    sync_directory(Path(path).parent)


def sync_directory(path):
    fd = os.open(path, os.O_RDONLY | os.O_DIRECTORY)
    try:
        os.fsync(fd)
    finally:
        os.close(fd)


def create_parents(path):
    missing = []
    cursor = path
    while not cursor.exists():
        missing.append(cursor)
        cursor = cursor.parent
    require(cursor.resolve() == cursor, "parent_symlink")
    for directory in reversed(missing):
        directory.mkdir(mode=0o700)
        sync_directory(directory.parent)


@contextmanager
def defer_cancellation():
    scope = _cancel_scope
    if scope is not None:
        scope["depth"] += 1
    try:
        yield
    finally:
        if scope is not None:
            scope["depth"] -= 1
            if scope["pending"] and scope["depth"] == 0 and not scope["unwinding"]:
                scope["unwinding"] = True
                raise InstallationCancelled("installation_cancelled")


@contextmanager
def installation_signals():
    global _cancel_scope
    require(_cancel_scope is None, "nested_initialization")
    scope = {"depth": 0, "pending": False, "unwinding": False}
    _cancel_scope = scope
    previous = {sig: signal.getsignal(sig) for sig in CANCEL_SIGNALS}
    def cancel(_sig, _frame):
        scope["pending"] = True
        if scope["depth"] == 0 and not scope["unwinding"]:
            scope["unwinding"] = True
            raise InstallationCancelled("installation_cancelled")
    try:
        for sig in CANCEL_SIGNALS:
            signal.signal(sig, cancel)
        yield
    finally:
        for sig, handler in previous.items():
            signal.signal(sig, handler)
        _cancel_scope = None


def configuration(target, api_port, panel_port, admin_password, issuer_password, admin_id, issuer_id):
    quote = json.dumps
    return f'''# Generated only for this new instance. Contains credentials; keep mode 0600.
poll_interval_secs = 10
status_addr = "127.0.0.1:{panel_port}"
keys = []
[zhipu]
[new_api]
base_url = "http://127.0.0.1:{api_port}"
root_username = "qtadmin"
root_password = {quote(admin_password)}
[new_api.manage]
version = "{NATIVE_VERSION}"
port = {api_port}
data_dir = {quote(str(target / "native"))}
[source_groups]
catalog_path = {quote(str(target / "state" / "catalog.sqlite3"))}
public_api_base_url = "http://127.0.0.1:{api_port}"
legacy_admin_user_id = {admin_id}
[source_groups.issuer]
user_id = {issuer_id}
username = "qtsigner"
password = {quote(issuer_password)}
'''


def own_process(process, native, directory, port):
    require(process.poll() is None, "native_process_exited")
    proc = Path(f"/proc/{process.pid}")
    require((proc / "exe").resolve() == native.resolve()
            and (proc / "cwd").resolve() == directory.resolve(), "native_identity")
    result = subprocess.run(["ss", "-H", "-ltnp", f"sport = :{port}"],
        check=True, capture_output=True, text=True, timeout=5)
    lines = result.stdout.strip().splitlines()
    return len(lines) == 1 and lines[0].count("pid=") == 1 and f"pid={process.pid}," in lines[0]


def stop_owned(process):
    if process is not None and process.poll() is None:
        process.terminate()
        try:
            process.wait(timeout=20)
        except subprocess.TimeoutExpired:
            process.kill()
            process.wait(timeout=5)


def initialize_fresh(target, native_binary, app_binary, api_port, panel_port, revision):
    with installation_signals():
        return _initialize_fresh(target, native_binary, app_binary, api_port, panel_port, revision)


def _initialize_fresh(target, native_binary, app_binary, api_port, panel_port, revision):
    target, native_binary, app_binary = map(Path, (target, native_binary, app_binary))
    require(target.is_absolute() and bool(re.fullmatch(r"/[A-Za-z0-9_./-]+", str(target)))
            and ".." not in target.parts and target.name not in ("", ".")
            and not os.path.lexists(target), "target_must_be_new")
    require(api_port != panel_port and all(0 < port < 65536 and port not in (3000, 3001)
            for port in (api_port, panel_port)), "dedicated_ports_required")
    require(digest(native_binary) == NATIVE_SHA256, "native_sha256")
    app_sha = digest(app_binary)
    require(app_binary.is_file() and os.access(app_binary, os.X_OK), "app_binary")
    # Reserve both IPv4 ports before creating any installation content.
    reservations = []
    try:
        for port in (api_port, panel_port):
            reserve = socket.socket()
            reservations.append(reserve)
            reserve.bind(("0.0.0.0", port))
        create_parents(target.parent)
        target.mkdir(mode=0o700)
        sync_directory(target.parent)
        for part in ("native", "state", "bin"):
            (target / part).mkdir(mode=0o700)
        sync_directory(target)
        native = target / "native" / f"new-api-{NATIVE_VERSION}"
        app = target / "bin" / "quota-throttle"
        shutil.copyfile(native_binary, native)
        shutil.copyfile(app_binary, app)
        native.chmod(0o700)
        app.chmod(0o700)
        for binary in (native, app):
            with binary.open("rb") as handle:
                os.fsync(handle.fileno())
            sync_directory(binary.parent)
        require(digest(native) == NATIVE_SHA256 and digest(app) == app_sha, "copied_binary_sha256")
        # 固定原生User约束min=8,max=20；15随机字节编码为20字符，保留120-bit熵。
        admin_password, issuer_password = secrets.token_urlsafe(15), secrets.token_urlsafe(15)
        pending = target / "config.pending.toml"
        private_write(pending, configuration(target, api_port, panel_port, admin_password, issuer_password, 0, 0))
    finally:
        for reserve in reservations:
            reserve.close()
    process, stage = None, "start_native"
    try:
        env = {"PATH": os.defpath, "PORT": str(api_port), "GIN_MODE": "release", "GOMAXPROCS": "2",
               "SQL_DSN": "local", "LOG_SQL_DSN": "local", "ERROR_LOG_ENABLED": "true",
               "SQLITE_PATH": str(target / "native" / "one-api.db"), "TZ": "UTC",
               "SESSION_SECRET": secrets.token_hex(32)}
        log_fd = os.open(target / "native" / "new-api.log", os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
        # 不阻塞/忽略子进程的信号：只延后Python抛异常，先取得真实Popen所有权。
        with defer_cancellation():
            with os.fdopen(log_fd, "wb") as log:
                process = subprocess.Popen([str(native)], cwd=target / "native", env=env,
                    stdin=subprocess.DEVNULL, stdout=log, stderr=log, start_new_session=True)
            private_write(target / "native" / "new-api.pid", str(process.pid))
        guard = lambda: own_process(process, native, target / "native", api_port)
        root = Native(api_port, guard)
        deadline = time.monotonic() + 40
        while True:
            if own_process(process, native, target / "native", api_port):
                try:
                    setup = root.api("GET", "/api/setup")
                    break
                except InstallationCancelled:
                    raise
                except BootstrapError:
                    pass
            require(time.monotonic() < deadline, "native_start_deadline")
            time.sleep(0.1)
        stage = "setup_once"
        require(setup.get("status") is False and setup.get("root_init") is False, "instance_not_fresh")
        root.api("POST", "/api/setup", {"username": "qtadmin", "password": admin_password,
                 "confirmPassword": admin_password, "SelfUseModeEnabled": True})
        root.login("qtadmin", admin_password, 100)
        stage = "fresh_instance_options"
        original = root.options()
        require("QuotaForNewUser" in original, "new_user_quota_missing")
        settings = {"AutomaticEnableChannelEnabled": "false", "LogConsumeEnabled": "true",
                    "RetryTimes": "1", "channel_affinity_setting.enabled": "false"}
        for key, value in settings.items():
            root.set_option(key, value)
        actual = root.options()
        require(all(actual.get(key) == value for key, value in settings.items()), "option_readback")
        stage = "create_issuer_once"
        try:
            root.set_option("QuotaForNewUser", "1000000000000")
            root.api("POST", "/api/user/", {"username": "qtsigner", "password": issuer_password,
                "display_name": "qt signer", "role": 1})
        finally:
            # 可捕获的取消不得打断恢复；归属失效时guard阻止向替代实例发送任何写。
            with defer_cancellation():
                root.set_option("QuotaForNewUser", original["QuotaForNewUser"])
                require(root.options().get("QuotaForNewUser") == original["QuotaForNewUser"], "quota_default_restore")
        stage = "issuer_identity_readback"
        issuer = Native(api_port, guard)
        profile = issuer.login("qtsigner", issuer_password, 1)
        require(issuer.user != root.user and type(profile.get("quota")) is int and profile["quota"] > 0, "issuer_not_ready")
        stage = "publish_private_configuration"
        ready = target / "config.ready.toml"
        private_write(ready, configuration(target, api_port, panel_port, admin_password, issuer_password, root.user, issuer.user))
        ready.rename(target / "config.toml")
        sync_directory(target)
        pending.unlink()  # Same credentials now durably retained in the final private config.
        sync_directory(target)
        report = {"target": str(target), "revision": revision, "app_sha256": app_sha,
                  "native_sha256": NATIVE_SHA256, "api_port": api_port, "panel_port": panel_port,
                  "initialized": True, "sources_registered": 0}
        private_write(target / "release.json", json.dumps(report, indent=2) + "\n")
        return report
    except Exception:
        raise BootstrapError(stage) from None
    finally:
        with defer_cancellation():
            stop_owned(process)


def service_absent():
    result = subprocess.run(["systemctl", "--user", "show", UNIT, "--no-pager",
        "--property=LoadState,FragmentPath,DropInPaths,ActiveState"],
        capture_output=True, text=True, timeout=15)
    require(result.returncode in (0, 1, 4) and len(result.stdout) <= 16384, "service_lookup_failed")
    entries = [line.split("=", 1) for line in result.stdout.splitlines() if line]
    require(all(len(entry) == 2 for entry in entries) and len(entries) == 4, "service_lookup_failed")
    values = dict(entries)
    require(values == {"LoadState": "not-found", "FragmentPath": "", "DropInPaths": "", "ActiveState": "inactive"}, "service_already_exists")
    result = subprocess.run(["systemd-analyze", "--user", "unit-paths"],
        capture_output=True, text=True, timeout=15)
    paths = result.stdout.splitlines()
    require(result.returncode == 0 and 0 < len(paths) <= 64 and len(result.stdout) <= 16384
            and all(Path(path).is_absolute() for path in paths), "unit_paths_failed")
    for directory in map(Path, paths):
        require(not os.path.lexists(directory / UNIT) and not os.path.lexists(directory / (UNIT + ".d")), "service_residue")
        if directory.exists():
            for dependency in [*directory.glob("*.wants"), *directory.glob("*.requires")]:
                require(not os.path.lexists(dependency / UNIT), "service_residue")


def install_service(target):
    target = Path(target)
    require(target.is_absolute() and bool(re.fullmatch(r"/[A-Za-z0-9_./-]+", str(target)))
            and ".." not in target.parts, "service_target")
    require((target / "config.toml").is_file() and (target / "release.json").is_file(), "installation_incomplete")
    service_absent()
    unit_dir = Path(os.environ.get("XDG_CONFIG_HOME", str(Path.home() / ".config"))) / "systemd" / "user"
    require(unit_dir.is_absolute(), "unit_paths_failed")
    create_parents(unit_dir)
    unit = unit_dir / UNIT
    require(not os.path.lexists(unit), "service_already_exists")
    contents = f'''[Unit]
Description=quota-throttle source groups
After=network-online.target
[Service]
Type=simple
WorkingDirectory={target}
ExecStart={target}/bin/quota-throttle serve {target}/config.toml
Restart=on-failure
RestartSec=5
KillMode=process
TimeoutStopSec=180
UMask=0077
Environment=RUST_LOG=info
NoNewPrivileges=true
[Install]
WantedBy=default.target
'''
    private_write(unit, contents)
    subprocess.run(["systemctl", "--user", "daemon-reload"], check=True, timeout=30)
    subprocess.run(["systemctl", "--user", "enable", "--now", UNIT], check=True, timeout=30)


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--target", required=True, type=Path)
    parser.add_argument("--newapi-binary", required=True, type=Path)
    parser.add_argument("--app-binary", required=True, type=Path)
    parser.add_argument("--api-port", type=int, default=13000)
    parser.add_argument("--panel-port", type=int, default=13001)
    parser.add_argument("--revision", required=True)
    parser.add_argument("--install-service", action="store_true")
    args = parser.parse_args()
    try:
        if args.install_service:
            service_absent()
        report = initialize_fresh(args.target, args.newapi_binary.resolve(strict=True),
            args.app_binary.resolve(strict=True), args.api_port, args.panel_port, args.revision)
        if args.install_service:
            install_service(args.target)
        report["service_install_requested"] = args.install_service
        print(json.dumps(report, indent=2))
    except Exception as error:
        print(json.dumps({"initialized": False, "stage": str(error) if isinstance(error, BootstrapError)
                          else "local_installation_error", "new_directory_retained": True}))
        raise SystemExit(1)
