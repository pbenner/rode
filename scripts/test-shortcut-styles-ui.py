#!/usr/bin/env python3
"""Native Emacs/Vim editing, safe saves, shell keys, and preset persistence."""

import os
import re
import shutil
import subprocess
import tempfile
import time
from pathlib import Path

remote_config = os.environ.get("RODE_TEST_SSH_CONFIG")
os.environ["RODE_UI_ARTIFACTS"] = str(
    Path(__file__).resolve().parent.parent
    / ("target/shortcut-styles-ssh-ui" if remote_config else "target/shortcut-styles-ui")
)
from ui_harness import ARTIFACTS, BINARY, close_window, focus_window, key, screenshot, text


def wait_for(check, message):
    deadline = time.monotonic() + 12
    while time.monotonic() < deadline:
        if check():
            return
        time.sleep(0.05)
    raise AssertionError(message)


def command(value):
    key("p", ("Control_L", "Shift_L"))
    text(value)
    key("Return")
    time.sleep(0.25)


def prefix(name, ctrl=False, leader="x"):
    key(leader, ("Control_L",))
    key(name, ("Control_L",) if ctrl else ())
    time.sleep(0.12)


def run():
    with tempfile.TemporaryDirectory(prefix="rode-shortcuts-") as directory:
        base = Path(directory)
        root = base / "project"
        root.mkdir()
        source = root / "main.txt"
        source.write_text("alpha beta\ngamma delta\n")
        other = root / "other.txt"
        other.write_text("second\n")
        env = dict(
            os.environ, XDG_CONFIG_HOME=str(base / "config"), XDG_DATA_HOME=str(base / "data")
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
            wrapper.write_text(
                "#!/usr/bin/python3\nimport os,sys\n"
                + "binary="
                + repr(binary)
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
                process = subprocess.Popen(args, env=env, stdout=log, stderr=log)

                def mapped():
                    nonlocal window
                    assert process.poll() is None
                    tree = subprocess.check_output(["xwininfo", "-root", "-tree"], text=True)
                    match = re.search(r'(0x[0-9a-f]+) "RODE', tree)
                    if match:
                        window = int(match[1], 16)
                    return match

                wait_for(mapped, "No native window")
                time.sleep(1.0)
                focus_window(window)
                command("Open file by path")
                text("main.txt")
                key("Return")
                time.sleep(0.4)
                command("Keyboard style: Emacs")
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
                wait_for(
                    lambda: source.read_text() == "Alpha beta\ngamma delta\n",
                    "Emacs editing / Ctrl+X Ctrl+S failed",
                )
                screenshot(window, "emacs-source.png")
                prefix("f", ctrl=True)
                text("other.txt")
                key("Return")
                time.sleep(0.3)
                key("e", ("Control_L",))
                text("X")
                prefix("s", ctrl=True)
                wait_for(lambda: other.read_text() == "secondX\n", "Emacs file-open prefix failed")
                prefix("t")
                time.sleep(0.7)
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
                text("main.txt")
                key("Return")
                time.sleep(0.3)
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
                    lambda: (
                        preferences.exists()
                        and '"keyboard.style": "vim"' in preferences.read_text()
                    ),
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
                    "Native Emacs/Vim editing, clipboard prefix, operators, undo/redo, pane navigation, shell keys, safe saves, and style persistence passed."
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
