#!/usr/bin/env python3
"""Real readline completion and terminal focus/shortcut regression checks."""

import json
import os
import subprocess
import tempfile
import time
from pathlib import Path

os.environ["RODE_UI_ARTIFACTS"] = str(
    Path(__file__).resolve().parent.parent / "target/terminal-keys-ui"
)
from ui_harness import (
    ARTIFACTS,
    BINARY,
    click,
    close_window,
    display,
    focus_window,
    key,
    screenshot,
    text,
    wait_for_window,
    x11,
    xtest,
)


def wait_for(check, message):
    deadline = time.monotonic() + 8
    while time.monotonic() < deadline:
        if check():
            return
        time.sleep(0.05)
    raise AssertionError(message)


def mouse_button(x, y, button, pressed):
    xtest.XTestFakeMotionEvent(display, -1, x, y, 0)
    xtest.XTestFakeButtonEvent(display, button, pressed, 0)
    x11.XFlush(display)
    time.sleep(0.08)


def clipboard():
    result = subprocess.run(
        ["xclip", "-selection", "clipboard", "-o"],
        capture_output=True,
        text=True,
        timeout=2,
    )
    return result.stdout if result.returncode == 0 else ""


def check_selection(window, root):
    """Exercise actual mouse selection and the native system clipboard."""
    text("python3 selection-input.py\n")
    wait_for(lambda: (root / "selection-ready").exists(), "Selection fixture not ready")
    # The isolated window uses the harness's fixed 1380x900 size and scale 1.
    # Row zero starts at x=320, with 13-point monospace cells about 7.8 px wide.
    mouse_button(320, 150, 1, True)
    xtest.XTestFakeMotionEvent(display, -1, 390, 150, 0)
    x11.XFlush(display)
    time.sleep(0.08)
    mouse_button(390, 150, 1, False)
    screenshot(window, "selected-text.png")

    def assert_copy(action):
        subprocess.run(
            ["xclip", "-selection", "clipboard"],
            input="clipboard sentinel",
            text=True,
            stdout=subprocess.DEVNULL,
            stderr=subprocess.DEVNULL,
            check=True,
            timeout=2,
        )
        action()
        wait_for(lambda: clipboard() == "SELECT_ME", "Copied text did not match selection")
        assert not (root / "selection-result").exists(), "Copy leaked input into the PTY"

    assert_copy(lambda: key("c", ("Control_L",)))
    assert_copy(lambda: key("c", ("Control_L", "Shift_L")))
    assert_copy(lambda: click(1096, 108))  # Header Copy.

    def palette_copy():
        key("p", ("Control_L", "Shift_L"))
        text("Terminal: Copy output")
        key("Return")

    assert_copy(palette_copy)

    def context_copy():
        mouse_button(420, 150, 3, True)
        mouse_button(420, 150, 3, False)
        time.sleep(0.2)
        key("x")  # Menu typing must not leak into the raw terminal fixture.
        click(470, 162)

    assert_copy(context_copy)
    click(600, 300)  # Clear selection and return focus after using the header/palette.
    key("c", ("Control_L",))
    wait_for(lambda: (root / "selection-result").exists(), "Interrupt did not reach the PTY")
    assert (root / "selection-result").read_text() == "03"


def run():
    with tempfile.TemporaryDirectory(prefix="rode-terminal-keys-") as directory:
        base = Path(directory)
        root = base / "project"
        root.mkdir()
        (root / "completion-target.txt").write_text("completed filename\n")
        source = root / "main.txt"
        source.write_text("editor source must stay unchanged\n")
        (root / "control-input.py").write_text(
            "import os,termios,tty\nfrom pathlib import Path\n"
            "settings=termios.tcgetattr(0)\n"
            "try:\n"
            " tty.setraw(0)\n"
            " Path('control-ready').touch()\n"
            " data=b''\n"
            " while len(data)<2: data+=os.read(0,2-len(data))\n"
            " Path('control-result').write_text(data.hex())\n"
            "finally: termios.tcsetattr(0,termios.TCSANOW,settings)\n"
        )
        (root / "selection-input.py").write_text(
            "import os,termios,tty\nfrom pathlib import Path\n"
            "settings=termios.tcgetattr(0)\n"
            "try:\n"
            " tty.setraw(0)\n"
            " os.write(1,b'\\x1b[2J\\x1b[HSELECT_ME suffix\\r\\nsecond line\\r\\n')\n"
            " Path('selection-ready').touch()\n"
            " Path('selection-result').write_text(os.read(0,1).hex())\n"
            "finally: termios.tcsetattr(0,termios.TCSANOW,settings)\n"
        )
        shell = base / "shell"
        shell.write_text("#!/bin/sh\nexec /bin/bash --noprofile --norc\n")
        shell.chmod(0o755)
        environment = dict(
            os.environ,
            XDG_CONFIG_HOME=str(base / "config"),
            XDG_DATA_HOME=str(base / "data"),
            SHELL=str(shell),
            WINIT_X11_SCALE_FACTOR="1",
        )
        with (ARTIFACTS / "native.log").open("w") as log:
            process = subprocess.Popen(
                [str(BINARY), "--theme", "carbon", str(root)],
                env=environment,
                stdout=log,
                stderr=log,
            )
            window = None
            try:
                window = wait_for_window(process, ARTIFACTS / "native.log")
                focus_window(window)
                time.sleep(0.7)
                key("p", ("Control_L",))
                text("main.txt")
                key("Return")
                time.sleep(0.4)
                key("t", ("Control_L", "Shift_L"))
                time.sleep(0.6)
                text("cat completion-")
                key("Tab")
                text(" > completed.out\n")
                wait_for(
                    lambda: (
                        (root / "completed.out").exists()
                        and (root / "completed.out").read_text() == "completed filename\n"
                    ),
                    "Tab moved focus instead of completing a filename",
                )
                assert (root / "completed.out").read_text() == "completed filename\n"
                screenshot(window, "filename-completion.png")
                # Tab must remain a shell key immediately after clicking back
                # into the pane, including after a period with no redraws.
                text("cat completion-")
                key("space", ("Control_L",))
                key("e")  # Code and terminal share a workspace; switch away and back.
                key("space", ("Control_L",))
                key("t")
                key("Tab")
                text(" > refocused.out\n")
                wait_for(
                    lambda: (
                        (root / "refocused.out").exists()
                        and (root / "refocused.out").read_text() == "completed filename\n"
                    ),
                    "First Tab after refocusing did not reach readline",
                )
                assert (root / "refocused.out").read_text() == "completed filename\n"
                text("echo history > history.out\n")
                wait_for(lambda: (root / "history.out").exists(), "Shell command failed")
                (root / "history.out").unlink()
                key("Up")
                key("Return")
                wait_for(
                    lambda: (root / "history.out").exists(), "Up did not navigate shell history"
                )
                # Native clipboard events must reach raw terminal applications
                # as Control+X (nano exit) and Control+C (interrupt/cancel).
                text("python3 control-input.py\n")
                wait_for(lambda: (root / "control-ready").exists(), "Raw input fixture not ready")
                key("x", ("Control_L",))
                key("c", ("Control_L",))
                wait_for(lambda: (root / "control-result").exists(), "Control keys were swallowed")
                assert (root / "control-result").read_text() == "1803"
                check_selection(window, root)
                # The editor escape route must still work with terminal focus.
                key("p", ("Control_L", "Shift_L"))
                text("Terminal: Sessions and Slurm allocations")
                key("Return")
                time.sleep(0.5)
                screenshot(window, "palette-from-terminal.png")
                key("Escape")
                click(550, 740)
                text("echo after > palette.out\n")
                wait_for(
                    lambda: (root / "palette.out").exists(),
                    "Could not return from command palette to shell",
                )
                assert source.read_text() == "editor source must stay unchanged\n"
                close_window(window, process)
                print(
                    "Native readline completion, focus, history, Control+X/Control+C, drag selection, keyboard/header/palette/context clipboard actions, and editor preservation passed."
                )
            except Exception:
                if window and process.poll() is None:
                    screenshot(window, "failure.png")
                raise
            finally:
                if process.poll() is None:
                    process.terminate()
                    process.wait(timeout=5)
                # Only this fixture's service lives in this private directory.
                for info in sessions(environment):
                    subprocess.run(
                        [str(BINARY), "--session-service", "request"],
                        input=json.dumps(
                            dict(action="end", id=info["id"], policy=info["policy"])
                        ).encode(),
                        env=environment,
                        check=True,
                        stdout=subprocess.DEVNULL,
                    )


def sessions(environment):
    output = subprocess.run(
        [str(BINARY), "--session-service", "request"],
        input=b'{"action":"list"}',
        env=environment,
        capture_output=True,
    )
    # A normal End closes the last service just before this cleanup query.
    return json.loads(output.stdout).get("data", []) if output.returncode == 0 else []


if __name__ == "__main__":
    run()
