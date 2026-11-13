#!/usr/bin/env python3
"""Keyboard-only pane workflow on an isolated X display; no pointer actions."""

import os
import shutil
import subprocess
from functools import partial
from pathlib import Path

os.environ["RODE_UI_ARTIFACTS"] = str(
    Path(__file__).resolve().parent.parent
    / (
        "target/navigation-ssh-ui"
        if os.environ.get("RODE_TEST_SSH_CONFIG")
        else "target/navigation-ui"
    )
)
from ui_fixture import project_ui
from ui_harness import (
    ARTIFACTS,
    BINARY,
    close_window,
    fixture_directory,
    fixture_environment,
    key,
    screenshot,
    text,
    wait_for_effect,
    write_python_launcher,
)

wait_for = partial(wait_for_effect, timeout=10)


def append(ui, path, value):
    ui.document(path)
    expected = path.read_text() + value
    before = ui.layout()["pass"]
    key("End", ("Control_L",))
    text(value)
    ui.source(expected, changed_after=before)
    key("s", ("Control_L",))
    ui.wait(lambda: path.read_text() == expected, f"Editor save failed: {value}")
    ui.control("source-editor-saved")


def run():
    with fixture_directory(prefix="rode-navigation-") as base:
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
        shell.write_text("#!/bin/sh\nexec bash --noprofile --norc\n")
        shell.chmod(0o755)
        env = fixture_environment(
            base,
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
            write_python_launcher(
                wrapper,
                "import os,sys\n"
                + "binary="
                + repr(real_ssh)
                + "\nos.execv(binary,[binary]+['-T' if a=='-tt' else a for a in sys.argv[1:]])\n",
            )
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

                ui = project_ui(process, env, root, log.name)
                window = ui.window
                command = partial(ui.command, keyboard=True)
                nav = ui.navigate
                # Enter a directory and open its child entirely with arrows.
                nav("f")
                ui.control("pane-Explorer", focused=True)
                key("Home")
                ui.control("explorer-selected:" + str(folder))
                key("Right")
                ui.control("explorer-row:" + str(nested))
                key("Right")
                ui.control("explorer-selected:" + str(nested))
                key("Return")
                append(ui, nested, "nested-edit\n")
                # Left from the editor selects the explorer; collapse to parent
                # then move to the next root file.
                nav("h")
                ui.control("pane-Explorer", focused=True)
                key("Left")
                key("Left")
                key("Down")
                ui.control("explorer-selected:" + str(first))
                key("Return")
                append(ui, first, "first-edit\n")
                # File suggestion selection, not just the first matching entry.
                nav("o")
                ui.control("file-dialog-path", focused=True)
                ui.fill("file-dialog-path", ".txt")
                key("Down")
                ui.value("file-picker-selected", str(first))
                key("Return")
                append(ui, first, "suggestion-edit\n")
                # A pane command must never insert its printable key in source.
                nav("f")
                ui.control("pane-Explorer", focused=True)
                key("c")
                ui.control("explorer-selected:" + str(second))
                key("Return")
                append(ui, second, "second-edit\n")
                nav("t")
                ui.terminal_input()
                # SSH shells come from the project host, independently of the GUI's SHELL.
                # Pin the readline fixture in this PTY and acknowledge it before shell-key checks.
                text("exec bash --noprofile --norc -i\n")
                text("printf '%s\\n' \"${BASH_VERSION:+bash-ready}\" > bash-ready.out\n")
                ui.wait(
                    lambda: (root / "bash-ready.out").read_text() == "bash-ready\n",
                    "Interactive Bash fixture did not become ready on the project host",
                )
                text("cat completion-")
                key("Tab")
                text(" > completed.out\n")
                wait_for(
                    lambda: (root / "completed.out").read_text() == "completed\n",
                    "Shell Tab/focus failed",
                )
                text("echo should-be-killed")
                key("a", ("Control_L",))
                key("k", ("Control_L",))
                text("echo shell-one > one.out\n")
                wait_for(
                    lambda: (root / "one.out").read_text() == "shell-one\n",
                    "Ctrl+K was intercepted",
                )
                # Persistence reveals a hidden terminal and retains input focus.
                nav("q")
                nav("s")
                ui.terminal_input()
                text("echo persistence > persistence.out\n")
                wait_for(
                    lambda: (root / "persistence.out").read_text() == "persistence\n",
                    "Persistence command left focus on a hidden pane",
                )
                nav("s")
                nav("n")
                ui.terminal_input()
                text("echo shell-two > two.out\n")
                wait_for(
                    lambda: (root / "two.out").read_text() == "shell-two\n",
                    "New terminal focus failed",
                )
                nav("bracketleft")
                ui.terminal_input()
                text("echo previous > previous.out\n")
                wait_for(
                    lambda: (root / "previous.out").read_text() == "previous\n",
                    "Previous terminal failed",
                )
                nav("bracketright")
                ui.terminal_input()
                text("echo next > next.out\n")
                wait_for(
                    lambda: (root / "next.out").read_text() == "next\n", "Next terminal failed"
                )
                nav("k", ("Shift_L",))
                screenshot(window, "taller-terminal.png")
                nav("m")
                ui.control("pane-maximized-Terminal")
                screenshot(window, "maximized-terminal.png")
                nav("m")
                ui.absent("pane-maximized-Terminal")
                # Closing a palette restores shell focus without a mouse click.
                command("Theme: Code Dark")
                ui.terminal_input()
                text("echo restored > restored.out\n")
                wait_for(
                    lambda: (root / "restored.out").read_text() == "restored\n",
                    "Palette did not restore shell focus",
                )
                # Select the second palette result with Down (UI theme list order).
                key("p", ("Control_L", "Shift_L"))
                ui.control("command-palette-input", focused=True)
                ui.fill("command-palette-input", "Navigation: Focus")
                key("Down")
                ui.control("command-selected:Navigation: Focus pane below")
                screenshot(window, "palette-selection.png")
                key("Escape")
                ui.absent("command-palette-input")
                ui.terminal_input()
                text("echo after-escape > escaped.out\n")
                wait_for(
                    lambda: (root / "escaped.out").read_text() == "after-escape\n",
                    "Escape did not restore shell focus",
                )
                nav("e")
                append(ui, second, "back-to-editor\n")
                # Select Explorer, the second explicit focus result. Spatial
                # "below" has no target when Code replaces the terminal surface.
                key("p", ("Control_L", "Shift_L"))
                ui.control("command-palette-input", focused=True)
                ui.fill("command-palette-input", "Navigation: Focus e")
                key("Down")
                ui.control("command-selected:Navigation: Focus explorer")
                key("Return")
                ui.control("pane-Explorer", focused=True)
                key("b")
                ui.control("explorer-selected:" + str(first))
                key("Return")
                append(ui, first, "palette-selection\n")
                nav("f")
                ui.control("pane-Explorer", focused=True)
                key("c")
                ui.control("explorer-selected:" + str(second))
                key("Return")
                nav("t")
                ui.terminal_input()
                text("echo selected > selected.out\n")
                wait_for(
                    lambda: (root / "selected.out").read_text() == "selected\n",
                    "Terminal focus failed after palette selection",
                )
                nav("p")
                nav("h", ("Shift_L",))
                nav("l", ("Shift_L",))
                nav("m")
                ui.control("pane-maximized-Tools")
                screenshot(window, "maximized-tools.png")
                nav("m")
                ui.absent("pane-maximized-Tools")
                nav("q")
                append(ui, second, "tools-return\n")
                nav("u")
                key("Return")
                screenshot(window, "keyboard-menu.png")
                key("Escape")
                nav("e")
                nav("slash")
                ui.control("navigation-leader-input", focused=True)
                # The leader field receives focus when bindings open.
                key("a", ("Control_L",))
                text("Ctrl+Shift+F12")
                ui.value("navigation-leader-value", "Ctrl+Shift+F12", focused=True)
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
                ui.resize(850, 560)
                screenshot(window, "bindings-compact.png")
                key("Escape")
                ui.absent("navigation-leader-input")
                nav("t", leader="F12", leader_modifiers=("Control_L", "Shift_L"))
                ui.terminal_input()
                text("echo remapped > remapped.out\n")
                wait_for(
                    lambda: (root / "remapped.out").read_text() == "remapped\n",
                    "Remapped leader failed",
                )
                nav("e", leader="F12", leader_modifiers=("Control_L", "Shift_L"))
                append(ui, second, "remapped-editor\n")
                assert nested.read_text() == "nested\nnested-edit\n"
                assert (
                    first.read_text() == "first\nfirst-edit\nsuggestion-edit\npalette-selection\n"
                )
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
