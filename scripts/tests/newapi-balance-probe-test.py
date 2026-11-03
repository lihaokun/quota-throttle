#!/usr/bin/env python3
"""No network: native balance probe admission and initialization regression checks."""
import contextlib
import io
import json
from pathlib import Path
import sys
import tempfile
import unittest
from unittest.mock import Mock, patch

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "probes"))
import newapi_balance_probe as probe


class StopAfterAccountPayload(Exception):
    pass


class BalanceProbeTests(unittest.TestCase):
    def test_ordinary_account_payload_obeys_pinned_native_user_bounds(self):
        root = Mock()
        root.api.side_effect = StopAfterAccountPayload
        for _ in range(8):
            report = {"checks": []}
            with self.assertRaises(StopAfterAccountPayload):
                probe.exercise(root, report)
            method, path, payload = root.api.call_args.args
            self.assertEqual((method, path), ("POST", "/api/user/"))
            self.assertTrue(8 <= len(payload["password"]) <= 20)
            self.assertTrue(1 <= len(payload["display_name"]) <= 20)
            self.assertTrue(1 <= len(payload["username"]) <= 20)
            self.assertEqual(payload["role"], 1)
            self.assertEqual(report["stage"], "create_ordinary_account")
            self.assertEqual(report["checks"], [])

    def test_wrong_binary_reports_fixed_stage_without_starting_a_process(self):
        with tempfile.TemporaryDirectory(prefix="balance-probe-admission-") as directory:
            binary = Path(directory) / "synthetic-not-native"
            binary.write_bytes(b"SYNTHETIC_PRIVATE_SENTINEL")
            output = io.StringIO()
            with patch.object(probe.subprocess, "Popen") as spawn, contextlib.redirect_stdout(output):
                self.assertEqual(probe.run(binary), 1)
            spawn.assert_not_called()
            report = json.loads(output.getvalue())
            self.assertEqual(report["stage"], "verify_binary")
            self.assertEqual(report["error"], "pinned_rc20_sha256")
            self.assertFalse(report["success"])
            self.assertNotIn("SYNTHETIC_PRIVATE_SENTINEL", output.getvalue())


if __name__ == "__main__":
    unittest.main(verbosity=2)
