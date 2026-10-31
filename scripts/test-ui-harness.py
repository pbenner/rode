#!/usr/bin/env python3
"""Window readiness regressions; run under the UI tests' isolated X display."""

import subprocess
import tempfile
import unittest
from pathlib import Path
from unittest.mock import Mock, patch

from ui_harness import focus_window, wait_for_window


class WindowReadinessTests(unittest.TestCase):
    def setUp(self):
        self.process = Mock()
        self.process.poll.return_value = None
        self.tree = patch("ui_harness.subprocess.check_output", return_value='0x123 "RODE"')
        self.attributes = patch("ui_harness.subprocess.run")
        self.sleep = patch("ui_harness.time.sleep")
        self.query_tree = self.tree.start()
        self.query_attributes = self.attributes.start()
        self.sleep.start()
        self.addCleanup(patch.stopall)

    @staticmethod
    def state(value, returncode=0):
        return subprocess.CompletedProcess([], returncode, stdout="Map State: " + value)

    def test_waits_for_mapping_and_viewable_ancestors(self):
        self.query_attributes.side_effect = [
            self.state("IsUnMapped"),
            self.state("IsUnviewable"),
            self.state("IsViewable"),
        ]
        self.assertEqual(wait_for_window(self.process, "startup.log"), 0x123)
        self.assertEqual(self.query_attributes.call_count, 3)

    def test_checks_all_candidates_instead_of_picking_a_hidden_window(self):
        self.query_tree.return_value = '0x123 "RODE hidden"\n0x456 "RODE visible"'
        self.query_attributes.side_effect = [self.state("IsUnMapped"), self.state("IsViewable")]
        self.assertEqual(wait_for_window(self.process, "startup.log"), 0x456)

    def test_retries_a_window_that_disappeared_during_discovery(self):
        self.query_attributes.side_effect = [self.state("", 1), self.state("IsViewable")]
        self.assertEqual(wait_for_window(self.process, "startup.log"), 0x123)
        self.assertEqual(self.query_attributes.call_count, 2)

    def test_timeout_reports_map_state_and_log_path(self):
        self.query_attributes.return_value = self.state("IsUnMapped")
        with patch("ui_harness.time.monotonic", side_effect=[0, 0, 21]):
            with self.assertRaisesRegex(AssertionError, "IsUnMapped.*startup.log"):
                wait_for_window(self.process, "startup.log")

    def test_early_exit_reports_startup_log_before_querying_x(self):
        self.process.poll.return_value = 1
        with tempfile.TemporaryDirectory() as directory:
            log = Path(directory) / "startup.log"
            log.write_text("renderer failed")
            with self.assertRaisesRegex(AssertionError, "renderer failed"):
                wait_for_window(self.process, log)
        self.query_tree.assert_not_called()

    def test_focus_waits_for_viewable_state_instead_of_raising_an_x11_badmatch(self):
        self.query_attributes.side_effect = [
            self.state("IsUnMapped"),
            self.state("IsUnviewable"),
            self.state("IsViewable"),
        ]
        with patch("ui_harness.x11.XSetInputFocus") as focus, patch("ui_harness.x11.XFlush"):
            focus_window(0x123)
            self.assertEqual(self.query_attributes.call_count, 3)
            focus.assert_called_once()

    def test_focus_timeout_reports_the_state_without_calling_xsetinputfocus(self):
        self.query_attributes.return_value = self.state("IsUnMapped")
        with (
            patch("ui_harness.time.monotonic", side_effect=[0, 0, 21]),
            patch("ui_harness.x11.XSetInputFocus") as focus,
        ):
            with self.assertRaisesRegex(AssertionError, "0x123.*IsUnMapped"):
                focus_window(0x123)
            focus.assert_not_called()


if __name__ == "__main__":
    unittest.main()
