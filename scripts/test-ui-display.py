#!/usr/bin/env python3
"""Headless-launch regressions; no display server or GUI is started by these tests."""

import contextlib
import io
import os
import signal
import subprocess
import unittest
from unittest.mock import Mock, patch

import ui_display


class HeadlessDisplayTests(unittest.TestCase):
    def test_missing_xvfb_never_starts_the_command_on_an_inherited_desktop(self):
        for missing in ("xvfb-run", "Xvfb", "xauth"):
            with (
                self.subTest(missing=missing),
                patch.dict(os.environ, DISPLAY=":0", WAYLAND_DISPLAY="wayland-0"),
                patch(
                    "ui_display.shutil.which",
                    side_effect=lambda name: None if name == missing else "/bin/" + name,
                ),
                patch("ui_display.subprocess.Popen") as start,
                contextlib.redirect_stderr(io.StringIO()) as error,
                self.assertRaises(SystemExit) as stopped,
            ):
                ui_display.main(["--", "rode"])
            self.assertEqual(stopped.exception.code, 2)
            start.assert_not_called()
            self.assertIn("no test was started", error.getvalue())

    def test_owned_display_removes_desktop_routes_and_preserves_test_arguments(self):
        child = Mock(pid=12345)
        child.wait.return_value = 7
        with (
            patch.dict(
                os.environ,
                DISPLAY=":0",
                WAYLAND_DISPLAY="wayland-0",
                WAYLAND_SOCKET="5",
                XAUTHORITY="/desktop/auth",
                RODE_TEST_BINARY="/test/rode",
            ),
            patch("ui_display.shutil.which", side_effect=lambda name: "/bin/" + name),
            patch("ui_display.subprocess.Popen", return_value=child) as start,
            patch("ui_display.os.killpg") as kill,
        ):
            result = ui_display.main(
                ["--screen", "3000x2000x24", "--", "python3", "fixture.py", "argument with spaces"]
            )
        self.assertEqual(result, 7)
        command = start.call_args.args[0]
        self.assertEqual(
            command[:4],
            [
                "/bin/xvfb-run",
                "--auto-servernum",
                "--server-args",
                "-screen 0 3000x2000x24 -nolisten tcp -noreset",
            ],
        )
        self.assertEqual(command[4:], ["python3", "fixture.py", "argument with spaces"])
        environment = start.call_args.kwargs["env"]
        for name in ("DISPLAY", "WAYLAND_DISPLAY", "WAYLAND_SOCKET", "XAUTHORITY"):
            self.assertNotIn(name, environment)
        self.assertEqual(environment["RODE_TEST_BINARY"], "/test/rode")
        self.assertEqual(environment["WINIT_UNIX_BACKEND"], "x11")
        self.assertTrue(start.call_args.kwargs["start_new_session"])
        self.assertEqual(kill.call_args_list[0].args, (12345, signal.SIGTERM))
        self.assertEqual(kill.call_args_list[1].args, (12345, signal.SIGKILL))

    def test_interruption_cleans_up_only_the_owned_group(self):
        child = Mock(pid=12345)
        child.wait.side_effect = [KeyboardInterrupt, subprocess.TimeoutExpired("fixture", 5), 0]
        with (
            patch("ui_display.shutil.which", side_effect=lambda name: "/bin/" + name),
            patch("ui_display.subprocess.Popen", return_value=child),
            patch("ui_display.os.killpg") as kill,
        ):
            self.assertEqual(ui_display.main(["--", "python3", "fixture.py"]), 130)
        self.assertEqual(
            [call.args for call in kill.call_args_list],
            [(12345, signal.SIGTERM), (12345, signal.SIGKILL)],
        )


if __name__ == "__main__":
    unittest.main()
