#!/usr/bin/env python3
"""Window/control readiness regressions; run under the UI tests' isolated X display."""

import hashlib
import json
import os
import subprocess
import sys
import tempfile
import time
import unittest
from pathlib import Path
from unittest.mock import Mock, call, patch

from ui_harness import (
    NativeUI,
    click_button,
    delayed_input,
    focus_request,
    focus_window,
    palette_command,
    read_json_lines,
    stop_fixture_agents,
    stop_process,
    viewport_state_path,
    wait_for_button,
    wait_for_project,
    wait_for_ui_state,
    wait_for_window,
)


class FixtureAcknowledgementTests(unittest.TestCase):
    def test_delayed_input_always_resumes_only_its_owned_process_after_failure(self):
        process = subprocess.Popen([sys.executable, "-c", "import time;time.sleep(30)"])
        status = Path(f"/proc/{process.pid}/stat")
        try:
            with patch.dict(os.environ, RODE_TEST_SLOW_UI="1"):
                with self.assertRaisesRegex(ValueError, "input failed"):
                    with delayed_input(process):
                        deadline = time.monotonic() + 2
                        while status.read_text().rsplit(") ", 1)[1].split()[0] != "T":
                            self.assertLess(time.monotonic(), deadline)
                            time.sleep(0.01)
                        raise ValueError("input failed")
                deadline = time.monotonic() + 2
                while status.read_text().rsplit(") ", 1)[1].split()[0] == "T":
                    self.assertLess(time.monotonic(), deadline)
                    time.sleep(0.01)
                self.assertIsNone(process.poll())
        finally:
            process.terminate()
            process.wait(timeout=5)

    def test_agent_cleanup_only_ends_the_owned_project_in_its_private_runtime(self):
        with tempfile.TemporaryDirectory() as directory:
            base = Path(directory)
            runtime = base / "runtime"
            runtime.mkdir()
            project = base / "project"
            environment = {"RODE_SESSION_DIR": str(runtime)}
            agents = {
                "kind": "agents",
                "data": [
                    dict(id="own", root=str(project), policy="own-policy"),
                    dict(id="other", root="/other/project", policy="other-policy"),
                ],
            }
            with patch("ui_harness.subprocess.run") as run:
                run.side_effect = [
                    Mock(stdout=json.dumps(agents).encode()),
                    Mock(stdout=b'{"kind":"ok"}'),
                ]
                self.assertTrue(stop_fixture_agents(environment, project))
                requests = [json.loads(call.kwargs["input"]) for call in run.call_args_list]
                self.assertEqual(
                    requests,
                    [
                        dict(action="agents"),
                        dict(action="end_agent", id="own", policy="own-policy"),
                    ],
                )
                run.assert_called_with(
                    run.call_args.args[0],
                    input=json.dumps(requests[-1]).encode(),
                    env=environment,
                    capture_output=True,
                    check=True,
                    timeout=5,
                )

    def test_agent_cleanup_cannot_use_an_ambient_service(self):
        with patch("ui_harness.subprocess.run") as run:
            for environment in ({}, {"RODE_SESSION_DIR": "/ambient/runtime"}):
                with self.assertRaisesRegex(ValueError, "temporary fixture"):
                    stop_fixture_agents(environment, Path("/tmp/fixture/project"))
            run.assert_not_called()

    def test_agent_cleanup_timeout_preserves_the_active_test_failure(self):
        with tempfile.TemporaryDirectory() as directory:
            base = Path(directory)
            runtime = base / "runtime"
            runtime.mkdir()
            with (
                patch(
                    "ui_harness.subprocess.run", side_effect=subprocess.TimeoutExpired("service", 5)
                ),
                patch("ui_harness.sys.stderr"),
            ):
                try:
                    raise AssertionError("original test failure")
                except AssertionError as original:
                    self.assertFalse(
                        stop_fixture_agents({"RODE_SESSION_DIR": str(runtime)}, base / "project")
                    )
                    self.assertIs(sys.exc_info()[1], original)

    def test_cleanup_escalates_a_child_ignoring_termination_without_another_ui_close(self):
        child = subprocess.Popen(
            [
                sys.executable,
                "-c",
                "import signal,time;signal.signal(signal.SIGTERM,signal.SIG_IGN);print('ready',flush=True);time.sleep(600)",
            ],
            stdout=subprocess.PIPE,
            text=True,
        )
        try:
            self.assertEqual(child.stdout.readline().strip(), "ready")
            self.assertTrue(stop_process(child, timeout=0.1))
            self.assertIsNotNone(child.poll())
        finally:
            if child.poll() is None:
                child.kill()
                child.wait()
            child.stdout.close()

    def test_missing_log_has_no_acknowledgements(self):
        with tempfile.TemporaryDirectory() as directory:
            self.assertEqual(read_json_lines(Path(directory) / "missing.jsonl"), [])

    def test_partial_json_and_utf8_wait_for_the_completed_record(self):
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "rpc.jsonl"
            first = b'{"id":1}\n'
            path.write_bytes(first + b'{"text":"\xce')
            self.assertEqual(read_json_lines(path), [{"id": 1}])
            path.write_bytes(first + '{"text":"\u03bb"}'.encode())
            self.assertEqual(read_json_lines(path), [{"id": 1}])
            with path.open("ab") as stream:
                stream.write(b"\n")
            self.assertEqual(read_json_lines(path), [{"id": 1}, {"text": "\u03bb"}])

    def test_malformed_completed_record_is_not_hidden(self):
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "rpc.jsonl"
            path.write_bytes(b'{"id":1}\nmalformed\n')
            with self.assertRaises(json.JSONDecodeError):
                read_json_lines(path)


class WindowReadinessTests(unittest.TestCase):
    def setUp(self):
        self.process = Mock(pid=42)
        self.process.poll.return_value = None
        self.tree = patch("ui_harness.subprocess.check_output", return_value='0x123 "RODE"')
        self.attributes = patch("ui_harness.subprocess.run")
        self.sleep = patch("ui_harness.time.sleep")
        self.query_tree = self.tree.start()
        self.query_attributes = self.attributes.start()
        self.sleep.start()
        self.owner = patch("ui_harness.window_pid", return_value=42).start()
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

    def test_mapped_window_from_another_process_is_not_the_test_window(self):
        self.query_tree.return_value = '0x123 "RODE other process"\n0x456 "RODE fixture"'
        self.owner.side_effect = [41, 42]
        self.query_attributes.return_value = self.state("IsViewable")
        self.assertEqual(wait_for_window(self.process, "startup.log"), 0x456)
        self.assertEqual(self.query_attributes.call_count, 1)

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
        with (
            patch("ui_harness.focus_request", return_value=True) as focus,
        ):
            focus_window(0x123)
            self.assertEqual(self.query_attributes.call_count, 3)
            focus.assert_called_once_with(0x123)

    def test_mapping_race_during_focus_is_retried_without_replaying_input(self):
        self.query_attributes.return_value = self.state("IsViewable")
        with patch("ui_harness.focus_request", side_effect=[False, True]) as focus:
            focus_window(0x123)
            self.assertEqual(focus.call_count, 2)

    def test_disappeared_window_does_not_terminate_the_python_test_process(self):
        self.assertFalse(focus_request(0))

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


class ViewportReadinessTests(unittest.TestCase):
    def test_equal_sized_windows_are_selected_by_title_and_process(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory) / "widgets.json"
            state = {"pid": 42, "title": "RODE · Chat", "size": [900, 760]}
            root.write_text(json.dumps(state))
            (Path(str(root) + ".a.json")).write_text(
                json.dumps(dict(state, title="RODE · Terminal"))
            )
            (Path(str(root) + ".b.json")).write_text(json.dumps(dict(state, pid=41)))
            expected = Path(str(root) + ".c.json")
            expected.write_text(json.dumps(state))
            process = Mock(pid=42)
            process.poll.return_value = None
            self.assertEqual(viewport_state_path(process, "log", root, "RODE · Chat"), expected)

    def test_missing_child_snapshot_reports_observed_current_process_titles(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory) / "widgets.json"
            Path(str(root) + ".child.json").write_text(
                json.dumps({"pid": 42, "title": "RODE · Terminal"})
            )
            process = Mock(pid=42)
            process.poll.return_value = None
            with (
                patch("ui_harness.time.monotonic", side_effect=[0, 0, 13]),
                patch("ui_harness.time.sleep"),
                self.assertRaisesRegex(AssertionError, "Chat.*Terminal.*log"),
            ):
                viewport_state_path(process, "log", root, "RODE · Chat")


class ButtonReadinessTests(unittest.TestCase):
    def setUp(self):
        self.process = Mock(pid=42)
        self.process.poll.return_value = None
        self.state = {
            "pid": 42,
            "pass": 10,
            "size": [800, 600],
            "pixels_per_point": 1.5,
            "buttons": [
                {
                    "name": "Install Codex",
                    "enabled": True,
                    "hovered": True,
                    "rect": [10, 20, 110, 40],
                }
            ],
        }
        self.read = patch("ui_harness.Path.read_text", return_value=json.dumps(self.state)).start()
        patch("ui_harness.time.sleep").start()
        patch("ui_harness.window_geometry", return_value=(100, 200, 1200, 900)).start()
        self.move = patch("ui_harness.move_pointer").start()
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

    def test_disabled_placeholder_resolution_does_not_accept_an_enabled_tab(self):
        self.read.side_effect = [
            json.dumps(self.state),
            json.dumps(dict(self.state, buttons=[dict(self.state["buttons"][0], enabled=False)])),
        ]
        self.assertEqual(
            wait_for_button(0x123, self.process, "log", "state", "Install Codex", enabled=False),
            (190, 245),
        )
        self.assertEqual(self.read.call_count, 2)

    def test_click_uses_native_input_once_after_readiness(self):
        self.read.side_effect = [json.dumps(self.state), json.dumps({**self.state, "pass": 11})]
        with patch("ui_harness.focus_window") as focus, patch("ui_harness.click") as click:
            click_button(0x123, self.process, "startup.log", "widgets.json", "Install Codex")
            focus.assert_called_once_with(0x123)
            click.assert_called_once_with(190, 245, button=1)

    def test_click_waits_for_hover_acknowledgement_before_activating_once(self):
        waiting = dict(self.state, buttons=[dict(self.state["buttons"][0], hovered=False)])
        self.read.side_effect = [
            json.dumps(waiting),
            json.dumps(waiting),
            json.dumps({**self.state, "pass": 11}),
        ]
        with patch("ui_harness.focus_window"), patch("ui_harness.click") as click:
            click.side_effect = lambda *_, **__: self.assertEqual(self.read.call_count, 3)
            click_button(0x123, self.process, "log", "state", "Install Codex")
            self.move.assert_called_once_with(190, 245)
            click.assert_called_once_with(190, 245, button=1)

    def test_hover_follows_a_repositioned_control_without_retrying_clicks(self):
        waiting = dict(self.state, buttons=[dict(self.state["buttons"][0], hovered=False)])
        moved = dict(self.state, buttons=[dict(self.state["buttons"][0], rect=[30, 40, 130, 60])])
        moving = {**moved, "buttons": [dict(moved["buttons"][0], hovered=False)], "pass": 11}
        self.read.side_effect = [
            json.dumps(waiting),
            json.dumps(moving),
            json.dumps({**moved, "pass": 12}),
        ]
        with patch("ui_harness.focus_window"), patch("ui_harness.click") as click:
            click_button(0x123, self.process, "log", "state", "Install Codex")
            self.assertEqual(self.move.call_args_list, [call(190, 245), call(220, 275)])
            click.assert_called_once_with(220, 275, button=1)

    def test_context_menu_waits_for_the_row_after_it_moves_before_right_clicking_once(self):
        waiting = dict(self.state, buttons=[dict(self.state["buttons"][0], hovered=False)])
        moved = dict(self.state, buttons=[dict(self.state["buttons"][0], rect=[30, 40, 130, 60])])
        self.read.side_effect = [
            json.dumps(waiting),
            json.dumps({**moved, "pass": 11}),
            json.dumps({**moved, "pass": 12}),
        ]
        with patch("ui_harness.focus_window"), patch("ui_harness.click") as click:
            click_button(0x123, self.process, "log", "state", "Install Codex", mouse_button=3)
            self.assertEqual(self.move.call_args_list, [call(190, 245), call(220, 275)])
            click.assert_called_once_with(220, 275, button=3)

    def test_old_hover_is_not_an_acknowledgement_of_the_new_pointer_motion(self):
        self.read.side_effect = [
            json.dumps(self.state),
            json.dumps(self.state),
            json.dumps({**self.state, "pass": 11}),
        ]
        with patch("ui_harness.focus_window"), patch("ui_harness.click") as click:
            click.side_effect = lambda *_, **__: self.assertEqual(self.read.call_count, 3)
            click_button(0x123, self.process, "log", "state", "Install Codex")
            click.assert_called_once_with(190, 245, button=1)

    def test_copy_click_wakes_idle_terminal_before_activating(self):
        # Rejected terminal input need not schedule another frame. Only the
        # pointer motion advances this snapshot and acknowledges Copy's hover.
        self.state["buttons"][0].update(name="terminal-copy-output", hovered=False)
        idle = json.dumps(self.state)
        self.read.return_value = idle

        def native_motion(*_):
            self.assertEqual(self.read.return_value, idle)
            self.read.return_value = json.dumps(
                {
                    **self.state,
                    "pass": 11,
                    "buttons": [dict(self.state["buttons"][0], hovered=True)],
                }
            )

        self.move.side_effect = native_motion
        with patch("ui_harness.focus_window"), patch("ui_harness.click") as click:
            click_button(0x123, self.process, "log", "state", "terminal-copy-output")
            self.move.assert_called_once_with(190, 245)
            self.assertEqual(self.read.call_count, 2)
            click.assert_called_once_with(190, 245, button=1)

    def test_modal_blocked_control_cannot_be_clicked_until_it_accepts_hover(self):
        blocked = dict(self.state, buttons=[dict(self.state["buttons"][0], enabled=False)])
        self.read.side_effect = [
            json.dumps(blocked),
            json.dumps({**blocked, "pass": 11}),
            json.dumps({**self.state, "pass": 12}),
        ]
        with patch("ui_harness.focus_window"), patch("ui_harness.click") as click:
            click.side_effect = lambda *_, **__: self.assertEqual(self.read.call_count, 3)
            click_button(0x123, self.process, "log", "state", "Install Codex")
            click.assert_called_once_with(190, 245, button=1)

    def test_unacknowledged_hover_times_out_without_activating(self):
        self.state["buttons"][0]["hovered"] = False
        self.read.return_value = json.dumps(self.state)
        with (
            patch("ui_harness.focus_window"),
            patch("ui_harness.click") as click,
            patch("ui_harness.time.monotonic", side_effect=[0, 0, 21]),
        ):
            with self.assertRaisesRegex(AssertionError, "Install Codex"):
                click_button(0x123, self.process, "log", "state", "Install Codex")
            click.assert_not_called()

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


class NativeInteractionTests(unittest.TestCase):
    def test_effect_waits_treat_only_missing_files_as_transient(self):
        process = Mock()
        process.poll.return_value = None
        ui = NativeUI(1, process, "log", "state")
        with patch("ui_harness.time.sleep"):
            check = Mock(side_effect=[FileNotFoundError(), False, "acknowledged"])
            self.assertEqual(ui.wait(check, "No result"), "acknowledged")
            with self.assertRaises(PermissionError):
                ui.wait(Mock(side_effect=PermissionError()), "Denied")

    def test_keyboard_actions_never_send_input_before_the_named_control_has_focus(self):
        ui = NativeUI(1, Mock(), "log", "state")
        with patch("ui_harness.focus_window"), patch("ui_harness.key") as key:
            with patch.object(ui, "control", side_effect=AssertionError("Not focused")):
                with self.assertRaisesRegex(AssertionError, "Not focused"):
                    ui.press("explorer-confirm-delete:folder", "Return")
            key.assert_not_called()


class PaletteReadinessTests(unittest.TestCase):
    def setUp(self):
        self.process = Mock(pid=42)
        self.process.poll.return_value = None
        self.read = patch("ui_harness.Path.read_text").start()
        patch("ui_harness.time.sleep").start()
        patch("ui_harness.window_geometry", return_value=(0, 0, 800, 600)).start()
        patch("ui_harness.focus_window").start()
        patch("ui_harness.move_pointer").start()
        self.key = patch("ui_harness.key").start()
        self.text = patch("ui_harness.text").start()
        self.click = patch("ui_harness.click").start()
        self.addCleanup(patch.stopall)

    @staticmethod
    def state(*names, focused=True, pid=42, render_pass=1):
        return json.dumps(
            {
                "pid": pid,
                "pass": render_pass,
                "size": [800, 600],
                "pixels_per_point": 1,
                "buttons": [
                    {
                        "name": name,
                        "enabled": True,
                        "hovered": True,
                        "focused": focused,
                        "rect": [10, 20, 110, 40],
                    }
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
            self.state(input_name, "command-match:Terminal: New terminal", render_pass=2),
            self.state(input_name),  # Click is queued; do not start the next operation yet.
            self.state(),
        ]
        self.text.side_effect = lambda _: self.assertEqual(self.read.call_count, 2)
        self.click.side_effect = lambda *_, **__: self.assertEqual(self.read.call_count, 6)
        self.command()
        self.assertEqual(self.read.call_count, 8)
        self.key.assert_called_once_with("p", ("Control_L", "Shift_L"))
        self.text.assert_called_once_with("Terminal: New terminal")
        self.click.assert_called_once_with(60, 30, button=1)

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


class NativeUIReadinessTests(unittest.TestCase):
    def setUp(self):
        self.process = Mock(pid=42)
        self.process.poll.return_value = None
        self.ui = NativeUI(0x123, self.process, "log", "state", "/project")

    def test_complete_path_not_just_the_first_edit_is_required_before_submission(self):
        path = "folder/file with spaces.txt"
        digest = hashlib.sha256(path.encode()).hexdigest()
        with (
            patch.object(self.ui, "command") as command,
            patch.object(self.ui, "control") as control,
            patch.object(self.ui, "layout", return_value={"pass": 10}),
            patch.object(self.ui, "absent") as absent,
            patch("ui_harness.key") as key,
            patch("ui_harness.text") as text,
        ):

            def ready(name, **_):
                if name == "file-dialog-path-value:" + digest:
                    self.assertNotIn(call("Return"), key.call_args_list)

            control.side_effect = ready
            self.ui.open_file(path)
            command.assert_called_once_with("Open file by path")
            text.assert_called_once_with(path)
            key.assert_has_calls([call("a", ("Control_L",)), call("Return")])
            self.assertIn(
                call("file-dialog-path-value:" + digest, focused=True), control.call_args_list
            )
            absolute = hashlib.sha256(("/project/" + path).encode()).hexdigest()
            control.assert_any_call("active-document:" + absolute)
            absent.assert_called_once_with("file-dialog-path", after_pass=10)

    def test_partial_or_unfocused_path_times_out_without_submitting_or_retrying(self):
        with (
            patch.object(self.ui, "command"),
            patch.object(self.ui, "control") as control,
            patch.object(self.ui, "layout", return_value={"pass": 10}),
            patch("ui_harness.key") as key,
            patch("ui_harness.text"),
        ):

            def ready(name, **_):
                if name.startswith("file-dialog-path-value:"):
                    raise AssertionError("Complete path did not arrive")

            control.side_effect = ready
            with self.assertRaisesRegex(AssertionError, "Complete path"):
                self.ui.open_file("file.txt")
            key.assert_called_once_with("a", ("Control_L",))

    def test_absence_cannot_acknowledge_a_snapshot_from_before_the_action(self):
        states = [
            {"pid": 41, "pass": 20, "buttons": []},
            {"pid": 42, "pass": 10, "buttons": []},
            {"pid": 42, "pass": 11, "buttons": [{"name": "dialog"}]},
            {"pid": 42, "pass": 12, "buttons": []},
        ]
        with (
            patch("ui_harness.Path.read_text", side_effect=[json.dumps(s) for s in states]) as read,
            patch("ui_harness.time.sleep"),
        ):
            self.assertEqual(self.ui.absent("dialog", after_pass=10)["pass"], 12)
            self.assertEqual(read.call_count, 4)


if __name__ == "__main__":
    unittest.main()
