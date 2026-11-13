#!/usr/bin/env python3
"""Native Standard/Emacs/Vim editing, safe saves, shell keys, and preset persistence."""

import os
import shutil
import subprocess
from functools import partial
from pathlib import Path

remote_config = os.environ.get("RODE_TEST_SSH_CONFIG")
os.environ["RODE_UI_ARTIFACTS"] = str(
    Path(__file__).resolve().parent.parent
    / ("target/shortcut-styles-ssh-ui" if remote_config else "target/shortcut-styles-ui")
)
from ui_fixture import project_ui
from ui_harness import (
    ARTIFACTS,
    BINARY,
    close_window,
    delayed_input,
    fixture_directory,
    fixture_environment,
    key,
    read_preferences,
    screenshot,
    stop_process,
    text,
    wait_for_effect,
    write_python_launcher,
)

wait_for = partial(wait_for_effect, timeout=12)


def run():
    with fixture_directory(prefix="rode-shortcuts-") as base:
        root = base / "project"
        root.mkdir()
        source = root / "main.txt"
        source.write_text("alpha beta\ngamma delta\n")
        other = root / "other.txt"
        other.write_text("second\n")
        comments = root / "settings.toml"
        comments.write_text("first = 1\nsecond = 2\n")
        env = fixture_environment(
            base, XDG_CONFIG_HOME=str(base / "config"), XDG_DATA_HOME=str(base / "data")
        )
        shell = base / "shell"
        shell.write_text("#!/bin/sh\nexec /bin/bash --noprofile --norc\n")
        shell.chmod(0o755)
        env["SHELL"] = str(shell)
        args = [str(BINARY), "--theme", "carbon"]
        if remote_config:
            binary = shutil.which("ssh")
            tools = base / "ssh-tools"
            tools.mkdir()
            wrapper = tools / "ssh"
            write_python_launcher(
                wrapper,
                "import os,sys\n"
                + "binary="
                + repr(binary)
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
                process = subprocess.Popen(args, env=env, stdout=log, stderr=log)

                ui = project_ui(process, env, root, log.name)
                window = ui.window
                command = partial(ui.command, keyboard=True)

                def prefix(name, ctrl=False, leader="x"):
                    ui.navigate(name, ("Control_L",) if ctrl else (), leader=leader)

                ui.open_file("main.txt")

                def saved(expected, message):
                    ui.source(expected)
                    key("s", ("Control_L",))
                    ui.control("source-editor-saved")
                    wait_for(lambda: source.read_text() == expected, message)

                original = "alpha beta\ngamma delta\n"
                key("a", ("Control_L",))
                key("bracketright", ("Control_L",))
                saved("    alpha beta\n    gamma delta\n", "Ctrl+] did not indent the selection")
                key("Tab")
                saved("        alpha beta\n        gamma delta\n", "Tab replaced selected source")
                key("Tab", ("Shift_L",))
                key("bracketleft", ("Control_L",))
                saved(original, "Shift+Tab / Ctrl+[ did not restore indentation")
                key("z", ("Control_L",))
                saved("    alpha beta\n    gamma delta\n", "Indentation was not one undo step")
                key("y", ("Control_L",))
                saved(original, "Ctrl+Y did not redo indentation")
                key("Home", ("Control_L",))
                key("l", ("Control_L",))
                key("d", ("Control_L", "Shift_L"))
                saved(
                    "alpha beta\nalpha beta\ngamma delta\n", "Line selection / duplication failed"
                )
                key("k", ("Control_L", "Shift_L"))
                saved(original, "Delete selected lines failed")
                key("Home", ("Control_L",))
                key("Return", ("Control_L",))
                saved("alpha beta\n\ngamma delta\n", "Insert line below split the current line")
                key("k", ("Control_L", "Shift_L"))
                key("Home", ("Control_L",))
                key("Return", ("Control_L", "Shift_L"))
                saved("\nalpha beta\ngamma delta\n", "Insert line above failed")
                key("k", ("Control_L", "Shift_L"))
                saved(original, "Deleting an inserted line changed other source")
                screenshot(window, "standard-source.png")

                ui.open_file("settings.toml")
                key("a", ("Control_L",))
                key("slash", ("Control_L",))
                key("s", ("Control_L",))
                wait_for(
                    lambda: comments.read_text() == "# first = 1\n# second = 2\n",
                    "Ctrl+/ did not comment the selection",
                )
                key("slash", ("Control_L",))
                key("s", ("Control_L",))
                wait_for(
                    lambda: comments.read_text() == "first = 1\nsecond = 2\n",
                    "Ctrl+/ did not uncomment the selection",
                )
                key("w", ("Control_L",))
                ui.document(source)

                command("Keyboard style: Emacs")
                ui.document(source)
                # Stress runs deliver the complete editing sequence together.
                # The following prefix must remain behind these pending edits.
                with delayed_input(process):
                    key("a", ("Control_L",))
                    key("space", ("Control_L",))
                    key("f", ("Control_L",))
                    key("f", ("Control_L",))
                    key("w", ("Control_L",))
                    key("y", ("Control_L",))
                    key("a", ("Control_L",))
                    key("k", ("Control_L",))
                    key("y", ("Control_L",))
                    key("a", ("Control_L",))
                    key("d", ("Control_L",))
                    text("A")
                # Actual X11 sends Cut for Ctrl+X, rather than Key::X.
                prefix("s", ctrl=True)
                ui.source("Alpha beta\ngamma delta\n")
                ui.control("source-editor-saved")
                wait_for(
                    lambda: source.read_text() == "Alpha beta\ngamma delta\n",
                    "Emacs editing / Ctrl+X Ctrl+S failed",
                )
                screenshot(window, "emacs-source.png")
                prefix("f", ctrl=True)
                ui.control("file-dialog-path", focused=True)
                ui.fill("file-dialog-path", "other.txt")
                key("Return")
                ui.document(other)
                key("e", ("Control_L",))
                text("X")
                prefix("s", ctrl=True)
                wait_for(lambda: other.read_text() == "secondX\n", "Emacs file-open prefix failed")
                prefix("t")
                ui.terminal_input()
                text("echo discard")
                key("w", ("Control_L",))
                text("emacs > emacs-shell.out\n")
                wait_for(
                    lambda: (root / "emacs-shell.out").exists(), "Emacs style stole shell keys"
                )
                assert (root / "emacs-shell.out").read_text() == "emacs\n"
                prefix("e", leader="space")
                command("Keyboard style: Vim")
                prefix("f", ctrl=True, leader="w")
                ui.control("file-dialog-path", focused=True)
                ui.fill("file-dialog-path", "main.txt")
                key("Return")
                ui.document(source)
                text("gg0wde")
                text("aBETA")
                key("Escape")
                text(":w")
                key("Return")
                wait_for(
                    lambda: source.read_text() == "Alpha BETA\ngamma delta\n",
                    "Vim insert/operators/:w failed",
                )
                text("u")
                key("r", ("Control_L",))
                text(":w")
                key("Return")
                wait_for(
                    lambda: source.read_text() == "Alpha BETA\ngamma delta\n",
                    "Vim undo/redo failed",
                )
                text("gg0vld")
                text(":w")
                key("Return")
                wait_for(
                    lambda: source.read_text() == "pha BETA\ngamma delta\n",
                    "Vim visual deletion failed",
                )
                text("u:w")
                key("Return")
                wait_for(
                    lambda: source.read_text() == "Alpha BETA\ngamma delta\n",
                    "Vim operator undo failed",
                )
                screenshot(window, "vim-normal.png")
                prefix("h", leader="w")
                key("Escape")  # Explorer returns to source without inserting.
                prefix("t", leader="w")
                text("echo discard")
                key("w", ("Control_L",))
                text("vim > vim-shell.out\n")
                wait_for(
                    lambda: (root / "vim-shell.out").exists(), "Vim style stole readline Ctrl+W"
                )
                assert (root / "vim-shell.out").read_text() == "vim\n"
                prefix("e", leader="space")
                command("Keyboard shortcuts")
                screenshot(window, "vim-bindings.png")
                key("Escape")
                preferences = base / "data/rode/app.ron"
                wait_for(
                    lambda: read_preferences(preferences).get("keyboard.style") == "vim",
                    "Style was not saved immediately",
                )
                command("Keyboard style: Standard")
                key("End", ("Control_L",))
                text("standard\n")
                key("s", ("Control_L",))
                wait_for(
                    lambda: source.read_text().endswith("standard\n"),
                    "Standard editing did not resume",
                )
                assert other.read_text() == "secondX\n"
                close_window(window, process)
                print(
                    "Native Standard/Emacs/Vim editing, indentation, line actions, clipboard prefix, operators, undo/redo, pane navigation, shell keys, safe saves, and style persistence passed."
                )
        except Exception:
            try:
                for path in [source, other, comments, Path(env["RODE_UI_TEST_STATE"])]:
                    if path.exists():
                        shutil.copyfile(path, ARTIFACTS / ("failure-" + path.name))
                if window and process and process.poll() is None:
                    screenshot(window, "failure.png")
            except Exception as error:
                print(f"Could not retain shortcut failure diagnostics: {error}")
            raise
        finally:
            if process:
                stop_process(process)


if __name__ == "__main__":
    run()
