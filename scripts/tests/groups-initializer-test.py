#!/usr/bin/env python3
"""Fault injection for the fresh initializer; no network, SSH or real services."""
import io
import json
import os
from pathlib import Path
import signal
import subprocess
import sys
import tempfile
import unittest
from unittest.mock import Mock, patch

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "deploy"))
import initialize_groups as setup


class Process:
    pid = 987654321
    stopped = False
    def poll(self): return 0 if self.stopped else None
    def terminate(self): self.stopped = True
    def wait(self, **_kwargs): return 0
    def kill(self): self.stopped = True


class FakeNative:
    calls = []
    values = {}
    fail_create = False
    fail_restore = False
    cancel_create = None
    cancel_setup = None

    def __init__(self, _port, guard=None): self.user = None
    def api(self, method, path, data=None):
        self.calls.append((method, path))
        if path == "/api/setup":
            if method == "GET" and self.cancel_setup: os.kill(os.getpid(), self.cancel_setup)
            return {"status": False, "root_init": False}
        if path == "/api/user/":
            setup.require(8 <= len(data["password"]) <= 20 and len(data["display_name"]) <= 20,
                          "native_user_field_limits")
            if self.cancel_create: os.kill(os.getpid(), self.cancel_create)
            if self.fail_create: raise setup.BootstrapError("ambiguous_create")
        return None
    def login(self, name, _password, role):
        self.user = 1 if role == 100 else 2
        return {"id": self.user, "username": name, "role": role, "status": 1, "quota": 100}
    def options(self): return dict(self.values)
    def set_option(self, key, value):
        if key == "QuotaForNewUser" and value == "0" and self.fail_restore:
            raise setup.BootstrapError("restore_failed")
        self.values[key] = value


class InitializerTests(unittest.TestCase):
    def setUp(self):
        self.dir = tempfile.TemporaryDirectory(prefix="qt-sgm7-initializer-unit-")
        self.root = Path(self.dir.name)
        self.native = self.root / "native-fixture"
        self.app = self.root / "app-fixture"
        for path in (self.native, self.app):
            path.write_bytes(b"synthetic-not-executed")
            path.chmod(0o700)
        self.target = self.root / "install"
        self.process = Process()
        FakeNative.calls = []
        FakeNative.values = {"QuotaForNewUser": "0"}
        FakeNative.fail_create = FakeNative.fail_restore = False
        FakeNative.cancel_create = None
        FakeNative.cancel_setup = None

    def tearDown(self): self.dir.cleanup()

    def initialize(self, popen=None):
        # All effects outside the private temp directory are replaced by explicit fixtures.
        with patch.object(setup, "NATIVE_SHA256", setup.digest(self.native)), \
             patch.object(setup, "Native", FakeNative), \
             patch.object(setup, "own_process", return_value=True), \
             patch.object(setup.subprocess, "Popen", side_effect=popen or (lambda *_a, **_k: self.process)), \
             patch.object(setup.socket, "socket"):
            return setup.initialize_fresh(self.target, self.native, self.app, 19990, 19991, "synthetic")

    def test_success_publishes_only_private_config_and_restores_default(self):
        result = self.initialize()
        self.assertTrue(result["initialized"])
        self.assertTrue(self.process.stopped)
        self.assertEqual(FakeNative.values["QuotaForNewUser"], "0")
        self.assertFalse((self.target / "config.pending.toml").exists())
        self.assertEqual((self.target / "config.toml").stat().st_mode & 0o777, 0o600)
        self.assertEqual(FakeNative.calls.count(("POST", "/api/user/")), 1)

    def test_lost_create_ack_is_never_reposted_and_keeps_recovery_material(self):
        FakeNative.fail_create = True
        with self.assertRaises(setup.BootstrapError): self.initialize()
        self.assertEqual(FakeNative.calls.count(("POST", "/api/user/")), 1)
        self.assertEqual(FakeNative.values["QuotaForNewUser"], "0")
        self.assertTrue((self.target / "config.pending.toml").exists())
        self.assertFalse((self.target / "config.toml").exists())
        self.assertTrue(self.process.stopped)

    def test_restore_failure_never_publishes_configuration(self):
        FakeNative.fail_restore = True
        with self.assertRaises(setup.BootstrapError): self.initialize()
        self.assertFalse((self.target / "config.toml").exists())
        self.assertTrue((self.target / "config.pending.toml").exists())
        self.assertTrue(self.process.stopped)

    def test_normal_signal_during_create_restores_and_stops_owned_child(self):
        for sig in (signal.SIGTERM, signal.SIGHUP):
            self.target = self.root / f"cancel-{sig}"
            self.process = Process()
            FakeNative.cancel_create = sig
            FakeNative.calls = []
            with self.assertRaises(setup.BootstrapError): self.initialize()
            self.assertEqual(FakeNative.calls.count(("POST", "/api/user/")), 1)
            self.assertEqual(FakeNative.values["QuotaForNewUser"], "0")
            self.assertTrue(self.process.stopped)
            self.assertFalse((self.target / "config.toml").exists())

    def test_signal_during_spawn_still_acquires_and_stops_child(self):
        def spawn(*_args, **_kwargs):
            os.kill(os.getpid(), signal.SIGTERM)
            return self.process
        with self.assertRaises(setup.BootstrapError): self.initialize(spawn)
        self.assertTrue(self.process.stopped)
        self.assertEqual(FakeNative.calls, [])

    def test_signal_during_setup_read_is_not_swallowed_as_readiness_retry(self):
        for sig in (signal.SIGTERM, signal.SIGHUP):
            self.target = self.root / f"setup-cancel-{sig}"
            self.process = Process()
            FakeNative.calls = []
            FakeNative.cancel_setup = sig
            with self.assertRaises(setup.BootstrapError): self.initialize()
            self.assertEqual(FakeNative.calls, [("GET", "/api/setup")])
            self.assertTrue(self.process.stopped)
            self.assertTrue((self.target / "config.pending.toml").is_file())
            self.assertFalse((self.target / "config.toml").exists())

    def test_ownership_loss_forbids_credentials_and_subsequent_restore(self):
        owner = iter((True, False))
        api = setup.Native(19990, lambda: next(owner, False))
        response = Mock()
        response.__enter__ = Mock(return_value=response)
        response.__exit__ = Mock(return_value=False)
        response.status = 200
        response.read.return_value = b'{"success":true,"data":null}'
        api.opener.open = Mock(return_value=response)
        with self.assertRaises(setup.BootstrapError): api.api("POST", "/api/user/", {"password": "synthetic"})
        with self.assertRaises(setup.BootstrapError): api.set_option("QuotaForNewUser", "0")
        self.assertEqual(api.opener.open.call_count, 1)

    def test_resolved_system_or_transient_unit_prevents_install(self):
        for fragment in ("/etc/systemd/user/quota-throttle-sgm.service", "/run/user/1/systemd/transient/quota-throttle-sgm.service"):
            output = f"LoadState=loaded\nFragmentPath={fragment}\nDropInPaths=\nActiveState=inactive\n"
            with patch.object(setup.subprocess, "run", return_value=subprocess.CompletedProcess([], 0, output, "")) as run:
                with self.assertRaises(setup.BootstrapError): setup.service_absent()
                self.assertEqual(run.call_count, 1)

    def test_residual_unit_dropin_or_wants_prevents_install(self):
        absent = subprocess.CompletedProcess([], 0, "LoadState=not-found\nFragmentPath=\nDropInPaths=\nActiveState=inactive\n", "")
        paths = subprocess.CompletedProcess([], 0, str(self.root) + "\n", "")
        residue = self.root / (setup.UNIT + ".d")
        residue.mkdir()
        with patch.object(setup.subprocess, "run", side_effect=[absent, paths]):
            with self.assertRaises(setup.BootstrapError): setup.service_absent()

    def test_account_write_follows_directory_durability(self):
        events = []
        original_sync = setup.sync_directory
        original_api = FakeNative.api
        def sync(path):
            events.append(("sync", str(path), (self.target / "config.pending.toml").exists()))
            original_sync(path)
        def api(instance, method, path, data=None):
            if method == "POST" and path == "/api/setup":
                self.assertTrue(any(event[1] == str(self.target.parent) for event in events))
                self.assertIn(("sync", str(self.target), True), events)
                self.assertTrue((self.target / "config.pending.toml").is_file())
            return original_api(instance, method, path, data)
        with patch.object(setup, "sync_directory", side_effect=sync), patch.object(FakeNative, "api", api):
            self.initialize()


if __name__ == "__main__": unittest.main(verbosity=2)
