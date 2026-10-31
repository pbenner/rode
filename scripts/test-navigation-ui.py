#!/usr/bin/env python3
"""Keyboard-only pane workflow on an isolated X display; no pointer actions."""

import ctypes
import os
import re
import shutil
import subprocess
import tempfile
import time
from pathlib import Path

os.environ["RODE_UI_ARTIFACTS"] = str(
    Path(__file__).resolve().parent.parent
    / (
        "target/navigation-ssh-ui"
        if os.environ.get("RODE_TEST_SSH_CONFIG")
        else "target/navigation-ui"
    )
)
from ui_harness import (
    ARTIFACTS,
    BINARY,
    close_window,
    display,
    focus_window,
    key,
    screenshot,
    text,
    x11,
)

x11.XResizeWindow.argtypes = [ctypes.c_void_p, ctypes.c_ulong, ctypes.c_uint, ctypes.c_uint]


def wait_for(check, message):
    deadline = time.monotonic() + 10
    while time.monotonic() < deadline:
        if check():
            return
        time.sleep(0.05)
    raise AssertionError(message)


def nav(name, modifiers=(), leader="space", leader_modifiers=("Control_L",)):
    key(leader, leader_modifiers)
    key(name, modifiers)
    time.sleep(0.15)


def command(value):
    key("p", ("Control_L", "Shift_L"))
    text(value)
    key("Return")
    time.sleep(0.3)


def append(path, value):
    key("End", ("Control_L",))
    text(value)
    key("s", ("Control_L",))
    wait_for(lambda: value in path.read_text(), f"Editor focus failed: {value}")


def run():
    with tempfile.TemporaryDirectory(prefix="rode-navigation-") as directory:
        base = Path(directory)
        root = base / "project"
        root.mkdir()
        folder = root / "a-folder"
        folder.mkdir()
        nested = folder / "inside.txt"
        nested.write_text("nested\n")
        first = root / "b-first.txt"
        first.write_text("first\n")
        second = root / "c-second.txt"
        second.write_text("second\n")
        (root / "completion-target.txt").write_text("completed\n")
        shell = base / "shell"
        shell.write_text("#!/bin/sh\nexec /bin/bash --noprofile --norc\n")
        shell.chmod(0o755)
        env = dict(
            os.environ,
            XDG_CONFIG_HOME=str(base / "config"),
            XDG_DATA_HOME=str(base / "data"),
            SHELL=str(shell),
        )
        remote_config = os.environ.get("RODE_TEST_SSH_CONFIG")
        args = [str(BINARY), "--theme", "carbon"]
        if remote_config:
            real_ssh = shutil.which("ssh")
            tools = base / "ssh-fixture-tools"
            tools.mkdir()
            wrapper = tools / "ssh"
            # The unprivileged fixture sshd cannot allocate a login TTY.
            # This affects the fixture login channel only, not native PTYs.
            wrapper.write_text(
                "#!/usr/bin/python3\nimport os,sys\n"
                + "binary="
                + repr(real_ssh)
                + "\nos.execv(binary,[binary]+['-T' if a=='-tt' else a for a in sys.argv[1:]])\n"
            )
            wrapper.chmod(0o755)
            env["PATH"] = str(tools) + os.pathsep + os.environ["PATH"]
            args += [
                "--ssh",
                "rode-test",
                "--ssh-config",
                remote_config,
                "--remote-path",
                str(root),
            ]
        else:
            args.append(str(root))
        process = None
        window = None
        try:
            with (ARTIFACTS / "native.log").open("w") as log:
                process = subprocess.Popen(
                    args,
                    env=env,
                    stdout=log,
                    stderr=log,
                )

                def mapped():
                    nonlocal window
                    assert process.poll() is None
                    output = subprocess.check_output(["xwininfo", "-root", "-tree"], text=True)
                    match = re.search(r'(0x[0-9a-f]+) "RODE', output)
                    if match:
                        window = int(match[1], 16)
                    return match

                wait_for(mapped, "No native window")
                wait_for(
                    lambda: (
                        "project"
                        in subprocess.check_output(["xwininfo", "-id", hex(window)], text=True)
                        and "[Opening project]"
                        not in subprocess.check_output(["xwininfo", "-id", hex(window)], text=True)
                    ),
                    "Project did not become ready",
                )
                time.sleep(0.9)
                focus_window(window)
                # Enter a directory and open its child entirely with arrows.
                nav("f")
                key("Home")
                key("Right")
                time.sleep(0.3)
                key("Right")
                key("Return")
                time.sleep(0.35)
                append(nested, "nested-edit\n")
                # Left from the editor selects the explorer; collapse to parent
                # then move to the next root file.
                nav("h")
                key("Left")
                key("Left")
                key("Down")
                key("Return")
                time.sleep(0.3)
                append(first, "first-edit\n")
                # File suggestion selection, not just the first matching entry.
                nav("o")
                text(".txt")
                key("Down")
                key("Return")
                time.sleep(0.3)
                append(first, "suggestion-edit\n")
                # A pane command must never insert its printable key in source.
                nav("f")
                key("c")
                key("Return")
                time.sleep(0.3)
                append(second, "second-edit\n")
                nav("t")
                time.sleep(0.8)
                text("cat completion-")
                key("Tab")
                text(" > completed.out\n")
                wait_for(lambda: (root / "completed.out").exists(), "Shell Tab/focus failed")
                assert (root / "completed.out").read_text() == "completed\n"
                text("echo should-be-killed")
                key("a", ("Control_L",))
                key("k", ("Control_L",))
                text("echo shell-one > one.out\n")
                wait_for(lambda: (root / "one.out").exists(), "Ctrl+K was intercepted")
                # Persistence reveals a hidden terminal and retains input focus.
                nav("q")
                nav("s")
                time.sleep(0.7)
                text("echo persistence > persistence.out\n")
                wait_for(
                    lambda: (root / "persistence.out").exists(),
                    "Persistence command left focus on a hidden pane",
                )
                nav("s")
                time.sleep(0.7)
                nav("n")
                time.sleep(0.7)
                text("echo shell-two > two.out\n")
                wait_for(lambda: (root / "two.out").exists(), "New terminal focus failed")
                nav("bracketleft")
                text("echo previous > previous.out\n")
                wait_for(lambda: (root / "previous.out").exists(), "Previous terminal failed")
                nav("bracketright")
                text("echo next > next.out\n")
                wait_for(lambda: (root / "next.out").exists(), "Next terminal failed")
                nav("k", ("Shift_L",))
                screenshot(window, "taller-terminal.png")
                nav("m")
                screenshot(window, "maximized-terminal.png")
                nav("m")
                # Closing a palette restores shell focus without a mouse click.
                command("Theme: Code Dark")
                text("echo restored > restored.out\n")
                wait_for(
                    lambda: (root / "restored.out").exists(), "Palette did not restore shell focus"
                )
                # Select the second palette result with Down (UI theme list order).
                key("p", ("Control_L", "Shift_L"))
                text("Navigation: Focus")
                key("Down")
                screenshot(window, "palette-selection.png")
                key("Escape")
                text("echo after-escape > escaped.out\n")
                wait_for(
                    lambda: (root / "escaped.out").exists(), "Escape did not restore shell focus"
                )
                nav("e")
                append(second, "back-to-editor\n")
                # Actually execute the second matching palette item: below.
                key("p", ("Control_L", "Shift_L"))
                text("Navigation: Focus pane")
                key("Down")
                key("Return")
                time.sleep(0.2)
                text("echo selected > selected.out\n")
                wait_for(
                    lambda: (root / "selected.out").exists(),
                    "Palette did not execute its second result",
                )
                nav("p")
                nav("h", ("Shift_L",))
                nav("l", ("Shift_L",))
                nav("m")
                screenshot(window, "maximized-tools.png")
                nav("m")
                nav("q")
                append(second, "tools-return\n")
                nav("u")
                key("Return")
                screenshot(window, "keyboard-menu.png")
                key("Escape")
                nav("e")
                nav("slash")
                # The leader field receives focus when bindings open.
                key("a", ("Control_L",))
                text("Ctrl+Shift+F12")
                key("Return", ("Control_L",))
                preferences = base / "data/rode/app.ron"
                wait_for(
                    lambda: (
                        preferences.exists()
                        and '"keyboard.leader": "Ctrl+Shift+F12"' in preferences.read_text()
                    ),
                    "Bindings were not saved immediately",
                )
                screenshot(window, "bindings.png")
                x11.XResizeWindow(display, window, 850, 560)
                x11.XFlush(display)
                screenshot(window, "bindings-compact.png")
                key("Escape")
                nav("t", leader="F12", leader_modifiers=("Control_L", "Shift_L"))
                text("echo remapped > remapped.out\n")
                wait_for(lambda: (root / "remapped.out").exists(), "Remapped leader failed")
                nav("e", leader="F12", leader_modifiers=("Control_L", "Shift_L"))
                append(second, "remapped-editor\n")
                assert nested.read_text() == "nested\nnested-edit\n"
                assert first.read_text() == "first\nfirst-edit\nsuggestion-edit\n"
                assert (
                    second.read_text()
                    == "second\nsecond-edit\nback-to-editor\ntools-return\nremapped-editor\n"
                )
                close_window(window, process)
                print(
                    "Keyboard-only explorer, suggestions, panes, terminals, shell keys, palette return, resizing/maximize, and persisted remapping passed."
                )
        except Exception:
            if window and process and process.poll() is None:
                screenshot(window, "failure.png")
            raise
        finally:
            if process and process.poll() is None:
                process.kill()
                process.wait()


if __name__ == "__main__":
    run()
