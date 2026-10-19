#!/usr/bin/env python3
"""C harness must fail closed and clean only its own resources."""
import os
from pathlib import Path
import sys
import tempfile
import unittest
from unittest.mock import Mock, patch

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "probes"))
import groups_runtime_probe as probe


class CleanupTests(unittest.TestCase):
    def test_group_selection_waits_for_real_priority_readback(self):
        panel = probe.Panel(19995)
        def state(observed, dirty):
            return {"catalog": {"groups": [{"group": {"group_id": "1", "ready": True}, "selected_member_id": "2"}],
                "memberships": [{"dirty": dirty, "membership": {"group_id": "1", "membership_id": "2",
                    "observed_status": 1, "observed_priority": observed, "desired_priority": 100}}]}}
        with patch.object(panel, "state", side_effect=[state(10, True), state(10, False), state(100, False)]) as read, \
             patch.object(probe.time, "sleep"):
            panel.settled_group("1", "2")
        self.assertEqual(read.call_count, 3)

    def test_clear_waits_for_new_runtime_revision_without_reposting(self):
        panel = probe.Panel(19995)
        old = {"source": {"config_revision": 1}, "volcano": {"runtime": {"revision": 2}}}
        new = {"source": {"config_revision": 1}, "volcano": {"runtime": {"revision": 3}}}
        with patch.object(panel, "source", side_effect=[old, old, new]) as read, \
             patch.object(panel, "operation") as submit, patch.object(probe.time, "sleep"):
            panel.clear("2")
        submit.assert_called_once()
        self.assertEqual(read.call_count, 3)

    def test_state_known_503_is_read_unavailability_only(self):
        panel = probe.Panel(19995)
        for code in ("snapshot_unavailable", "overloaded"):
            with patch.object(panel, "raw", return_value=(503, {"error": code})):
                with self.assertRaises(probe.ReadUnavailable):
                    panel.state()
                with self.assertRaises(probe.ProbeError) as error:
                    panel.call("POST", "/tokens/1/reveal", {})
                self.assertNotIsInstance(error.exception, probe.ReadUnavailable)

    def test_unknown_read_error_is_not_retried(self):
        panel = probe.Panel(19995)
        for status, code in ((503, "storage"), (500, "snapshot_unavailable")):
            with patch.object(panel, "raw", return_value=(status, {"error": code})):
                with self.assertRaises(probe.ProbeError) as error:
                    panel.state()
                self.assertNotIsInstance(error.exception, probe.ReadUnavailable)

    def test_until_never_swallows_semantic_failures_or_write_io_errors(self):
        for error in (probe.ProbeError("assertion"), OSError("write_unknown")):
            read = Mock(side_effect=error)
            with self.assertRaises(type(error)):
                probe.until(read)
            read.assert_called_once()

    def test_until_retries_only_explicit_read_unavailability(self):
        read = Mock(side_effect=[probe.ReadUnavailable("temporary"), True])
        with patch.object(probe.time, "sleep"):
            self.assertTrue(probe.until(read))
        self.assertEqual(read.call_count, 2)

    def test_controller_cleanup_failure_still_closes_upstream_and_cannot_pass(self):
        report, upstream = {"success": True}, Mock()
        app = Mock()
        app.poll.return_value = None
        with tempfile.TemporaryDirectory() as directory, patch.object(probe, "stop_owned", side_effect=OSError):
            probe.cleanup(report, app, None, Path(directory), upstream)
        self.assertFalse(report["success"])
        self.assertFalse(report["isolated_controller_stopped"])
        upstream.shutdown.assert_called_once()
        upstream.server_close.assert_called_once()

    def test_foreign_process_is_not_signalled_and_cannot_pass(self):
        report, upstream = {"success": True}, Mock()
        with tempfile.TemporaryDirectory() as directory, patch.object(probe.signal, "pidfd_send_signal") as send:
            probe.cleanup(report, None, os.getpid(), Path(directory), upstream)
        send.assert_not_called()
        self.assertFalse(report["success"])
        self.assertFalse(report["isolated_native_stopped"])

    def test_successful_cleanup_does_not_turn_a_failed_test_green(self):
        report, upstream = {"success": False, "error": "original_failure"}, Mock()
        with tempfile.TemporaryDirectory() as directory:
            probe.cleanup(report, None, None, Path(directory), upstream)
        self.assertFalse(report["success"])
        self.assertEqual(report["error"], "original_failure")

    def test_upstream_shutdown_failure_still_closes_socket_and_fails(self):
        report, upstream = {"success": True}, Mock()
        upstream.shutdown.side_effect = OSError
        with tempfile.TemporaryDirectory() as directory:
            probe.cleanup(report, None, None, Path(directory), upstream)
        upstream.server_close.assert_called_once()
        self.assertFalse(report["success"])


if __name__ == "__main__":
    unittest.main(verbosity=2)
