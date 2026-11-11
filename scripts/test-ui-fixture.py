#!/usr/bin/env python3
"""Owned UI lifecycle and diagnostics regressions; run on the private test display."""

import tempfile
import unittest
from pathlib import Path
from unittest.mock import Mock, patch

from ui_fixture import native_session


class SessionLifetimeTests(unittest.TestCase):
    def setUp(self):
        self.directory = tempfile.TemporaryDirectory()
        self.addCleanup(self.directory.cleanup)
        self.base = Path(self.directory.name)
        self.state = self.base / "state.json"
        self.state.write_text('{"pid":42}')
        self.environment = {"RODE_UI_TEST_STATE": str(self.state)}
        self.process = Mock(returncode=0)
        self.process.poll.return_value = None
        self.ui = Mock(window=123)
        self.ui.process = self.process
        for name, value in [
            ("ARTIFACTS", self.base),
            ("subprocess.Popen", Mock(return_value=self.process)),
            ("project_ui", Mock(return_value=self.ui)),
            ("close_window", Mock()),
            ("stop_process", Mock()),
            ("screenshot", Mock()),
        ]:
            mock = patch("ui_fixture." + name, value)
            mock.start()
            self.addCleanup(mock.stop)
            setattr(self, name.rsplit(".", 1)[-1], value)

    def session(self):
        return native_session(["rode"], self.environment, self.base, "fixture")

    def test_startup_failure_keeps_diagnostics_and_reaps_the_spawned_process(self):
        self.project_ui.side_effect = AssertionError("project never ready")
        with self.assertRaisesRegex(AssertionError, "project never ready"):
            with self.session():
                self.fail("Unready process reached the scenario")
        self.assertEqual((self.base / "fixture-failure.json").read_bytes(), self.state.read_bytes())
        self.stop_process.assert_called_once_with(self.process)
        self.close_window.assert_not_called()

    def test_capture_failure_does_not_replace_the_original_assertion(self):
        self.screenshot.side_effect = RuntimeError("display vanished")
        with patch("ui_fixture.sys.stderr"):
            with self.assertRaisesRegex(AssertionError, "missing output"):
                with self.session():
                    raise AssertionError("missing output")
        self.close_window.assert_not_called()
        self.stop_process.assert_called_once_with(self.process)

    def test_interrupt_also_retains_diagnostics_and_cleans_up(self):
        with self.assertRaises(KeyboardInterrupt):
            with self.session():
                raise KeyboardInterrupt
        self.screenshot.assert_called_once_with(123, "fixture-failure.png")
        self.stop_process.assert_called_once_with(self.process)
        self.close_window.assert_not_called()

    def test_success_checks_normal_shutdown_once(self):
        with self.session() as ui:
            self.assertIs(ui, self.ui)
        self.close_window.assert_called_once_with(123, self.process)
        self.stop_process.assert_called_once_with(self.process)

    def test_scenario_that_closed_its_window_is_not_closed_again(self):
        with self.session():
            self.process.poll.return_value = 0
        self.close_window.assert_not_called()
        self.stop_process.assert_called_once_with(self.process)

    def test_abnormal_exit_cannot_pass_as_completed_scenario(self):
        with self.assertRaisesRegex(AssertionError, "RODE exited with 7"):
            with self.session():
                self.process.poll.return_value = 7
                self.process.returncode = 7
        self.close_window.assert_not_called()
        self.stop_process.assert_called_once_with(self.process)


if __name__ == "__main__":
    unittest.main()
