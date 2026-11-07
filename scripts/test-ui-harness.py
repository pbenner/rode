#!/usr/bin/env python3
"""Window/control readiness regressions; run under the UI tests' isolated X display."""

import json
import subprocess
import tempfile
import unittest
from pathlib import Path
from unittest.mock import Mock, patch

from ui_harness import (
    click_button,
    focus_window,
    palette_command,
    wait_for_button,
    wait_for_project,
    wait_for_ui_state,
    wait_for_window,
)


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

    def test_project_waits_past_initial_mapping_and_pending_preparation(self):
        for suffix in ("", " [Sandbox]"):
            with self.subTest(suffix=suffix):
                self.query_attributes.reset_mock()
                self.query_attributes.side_effect = [
                    subprocess.CompletedProcess(
                        [], 0, stdout=f'xwininfo: Window id: 0x123 "{title}"\n'
                    )
                    for title in (
                        "RODE",
                        "RODE · project-other",
                        "RODE · project [Opening project]",
                        "RODE · project" + suffix,
                    )
                ]
                wait_for_project(0x123, self.process, "startup.log", "/tmp/project")
                self.assertEqual(self.query_attributes.call_count, 4)

    def test_project_timeout_reports_pending_title(self):
        self.query_attributes.return_value = subprocess.CompletedProcess(
            [], 0, stdout='xwininfo: Window id: 0x123 "RODE · project [Opening project]"\n'
        )
        with patch("ui_harness.time.monotonic", side_effect=[0, 0, 21]):
            with self.assertRaisesRegex(AssertionError, r"Opening project.*startup.log"):
                wait_for_project(0x123, self.process, "startup.log", "/tmp/project")

    def test_focus_timeout_reports_the_state_without_calling_xsetinputfocus(self):
        self.query_attributes.return_value = self.state("IsUnMapped")
        with (
            patch("ui_harness.time.monotonic", side_effect=[0, 0, 21]),
            patch("ui_harness.x11.XSetInputFocus") as focus,
        ):
            with self.assertRaisesRegex(AssertionError, "0x123.*IsUnMapped"):
                focus_window(0x123)
            focus.assert_not_called()


class ButtonReadinessTests(unittest.TestCase):
    def setUp(self):
        self.process = Mock(pid=42)
        self.process.poll.return_value = None
        self.state = {
            "pid": 42,
            "pass": 10,
            "size": [800, 600],
            "pixels_per_point": 1.5,
            "buttons": [{"name": "Install Codex", "enabled": True, "rect": [10, 20, 110, 40]}],
        }
        self.read = patch("ui_harness.Path.read_text", return_value=json.dumps(self.state)).start()
        patch("ui_harness.time.sleep").start()
        patch("ui_harness.window_geometry", return_value=(100, 200, 1200, 900)).start()
        self.addCleanup(patch.stopall)

    def wait(self):
        return wait_for_button(0x123, self.process, "startup.log", "widgets.json", "Install Codex")

    def test_waits_for_current_process_and_enabled_visible_control(self):
        self.read.side_effect = [
            FileNotFoundError(),
            json.dumps(dict(self.state, pid=41)),
            json.dumps(dict(self.state, buttons=[])),
            json.dumps(dict(self.state, buttons=[dict(self.state["buttons"][0], enabled=False)])),
            json.dumps(
                dict(self.state, buttons=[dict(self.state["buttons"][0], rect=[0, 0, 0, 0])])
            ),
            json.dumps(self.state),
        ]
        self.assertEqual(self.wait(), (190, 245))
        self.assertEqual(self.read.call_count, 6)

    def test_waits_for_resized_layout_before_resolving_scaled_coordinates(self):
        self.read.side_effect = [
            json.dumps(dict(self.state, size=[600, 400])),
            json.dumps(self.state),
        ]
        self.assertEqual(self.wait(), (190, 245))
        self.assertEqual(self.read.call_count, 2)

    def test_click_uses_native_input_once_after_readiness(self):
        with patch("ui_harness.focus_window") as focus, patch("ui_harness.click") as click:
            click_button(0x123, self.process, "startup.log", "widgets.json", "Install Codex")
            focus.assert_called_once_with(0x123)
            click.assert_called_once_with(190, 245)

    def test_focused_control_waits_for_final_focus(self):
        focused = dict(self.state, buttons=[dict(self.state["buttons"][0], focused=True)])
        self.read.side_effect = [json.dumps(self.state), json.dumps(focused)]
        self.assertEqual(
            wait_for_button(0x123, self.process, "log", "state", "Install Codex", focused=True),
            (190, 245),
        )
        self.assertEqual(self.read.call_count, 2)

    def test_edit_barrier_waits_for_a_new_change_not_just_another_repaint(self):
        def snapshot(render_pass, changed):
            return json.dumps(
                {
                    **self.state,
                    "pass": render_pass,
                    "buttons": [dict(self.state["buttons"][0], changed_pass=changed)],
                }
            )

        self.read.side_effect = [
            snapshot(11, None),
            snapshot(12, 9),
            snapshot(13, 10),
            snapshot(14, 11),
        ]
        self.assertEqual(
            wait_for_button(0x123, self.process, "log", "state", "Install Codex", changed_after=10),
            (190, 245),
        )
        self.assertEqual(self.read.call_count, 4)

    def test_missing_edit_times_out_instead_of_sending_the_next_action(self):
        with patch("ui_harness.time.monotonic", side_effect=[0, 0, 21]):
            with self.assertRaisesRegex(AssertionError, "Install Codex.*log"):
                wait_for_button(
                    0x123, self.process, "log", "state", "Install Codex", changed_after=10
                )

    def test_timeout_identifies_disabled_control_and_log(self):
        self.state["buttons"][0]["enabled"] = False
        self.read.return_value = json.dumps(self.state)
        with patch("ui_harness.time.monotonic", side_effect=[0, 0, 21]):
            with self.assertRaisesRegex(AssertionError, "Install Codex.*False.*startup.log"):
                self.wait()


class PaletteReadinessTests(unittest.TestCase):
    def setUp(self):
        self.process = Mock(pid=42)
        self.process.poll.return_value = None
        self.read = patch("ui_harness.Path.read_text").start()
        patch("ui_harness.time.sleep").start()
        patch("ui_harness.window_geometry", return_value=(0, 0, 800, 600)).start()
        patch("ui_harness.focus_window").start()
        self.key = patch("ui_harness.key").start()
        self.text = patch("ui_harness.text").start()
        self.click = patch("ui_harness.click").start()
        self.addCleanup(patch.stopall)

    @staticmethod
    def state(*names, focused=True, pid=42):
        return json.dumps(
            {
                "pid": pid,
                "pass": 1,
                "size": [800, 600],
                "pixels_per_point": 1,
                "buttons": [
                    {"name": name, "enabled": True, "focused": focused, "rect": [10, 20, 110, 40]}
                    for name in names
                ],
            }
        )

    def command(self):
        palette_command(0x123, self.process, "log", "state", "Terminal: New terminal")

    def test_delayed_focus_and_partial_query_never_submit_the_default_command(self):
        input_name = "command-palette-input"
        self.read.side_effect = [
            self.state(input_name, focused=False),
            self.state(input_name),
            self.state(input_name),  # An unprocessed or partial query is not an acknowledgement.
            self.state(input_name, "command-match:New file"),
            self.state(input_name, "command-match:Terminal: New terminal"),
            self.state(input_name),  # Click is queued; do not start the next operation yet.
            self.state(),
        ]
        self.text.side_effect = lambda _: self.assertEqual(self.read.call_count, 2)
        self.click.side_effect = lambda *_: self.assertEqual(self.read.call_count, 5)
        self.command()
        self.assertEqual(self.read.call_count, 7)
        self.key.assert_called_once_with("p", ("Control_L", "Shift_L"))
        self.text.assert_called_once_with("Terminal: New terminal")
        self.click.assert_called_once_with(60, 30)

    def test_missing_complete_query_fails_without_click_or_return_or_retry(self):
        self.read.return_value = self.state("command-palette-input")
        with patch("ui_harness.time.monotonic", side_effect=[0, 0, 0, 0, 13]):
            with self.assertRaisesRegex(AssertionError, "command-match:Terminal: New terminal"):
                self.command()
        self.click.assert_not_called()
        self.key.assert_called_once_with("p", ("Control_L", "Shift_L"))
        self.text.assert_called_once()

    def test_absence_checks_reject_missing_malformed_and_other_process_snapshots(self):
        self.read.side_effect = [
            FileNotFoundError(),
            "{",
            self.state(pid=41),
            self.state("still-open"),
            self.state(),
        ]
        wait_for_ui_state(
            self.process, "log", "state", lambda state: not state["buttons"], "Dialog stayed open"
        )
        self.assertEqual(self.read.call_count, 5)


if __name__ == "__main__":
    unittest.main()
