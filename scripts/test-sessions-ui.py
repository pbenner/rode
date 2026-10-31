#!/usr/bin/env python3
"""Native persistent-terminal/shared-view/Slurm UI checks without a scheduler."""

import json
import os
import subprocess
import tempfile
import time
from pathlib import Path

os.environ["RODE_UI_ARTIFACTS"] = str(Path(__file__).resolve().parent.parent / "target/sessions-ui")
# Target each native window by its newly mapped XID.
import ctypes
import re

from ui_harness import (
    ARTIFACTS,
    BINARY,
    click,
    close_window,
    display,
    focus_window,
    key,
    request_close,
    screenshot,
    text,
    theme,
    x11,
    xtest,
)

x11.XRaiseWindow.argtypes = [ctypes.c_void_p, ctypes.c_ulong]
x11.XResizeWindow.argtypes = [ctypes.c_void_p, ctypes.c_ulong, ctypes.c_uint, ctypes.c_uint]


def wait_for(check, message):
    deadline = time.monotonic() + 12
    while time.monotonic() < deadline:
        if check():
            return
        time.sleep(0.05)
    raise AssertionError(message)


def focus(window):
    x11.XRaiseWindow(display, window)
    focus_window(window)
    time.sleep(0.2)


def window_for(process, previous):
    for _ in range(150):
        tree = subprocess.check_output(["xwininfo", "-root", "-tree"], text=True)
        for match in re.finditer(r'(0x[0-9a-f]+) "RODE', tree):
            window = int(match[1], 16)
            if window not in previous:
                time.sleep(0.8)
                focus(window)
                return window
        assert process.poll() is None
        time.sleep(0.05)
    raise AssertionError("No native RODE window")


def command(value):
    key("p", ("Control_L", "Shift_L"))
    text(value)
    key("Return")
    time.sleep(1)


def rpc(environment, action, **values):
    output = subprocess.check_output(
        [str(BINARY), "--session-service", "request"],
        input=json.dumps(dict(action=action, **values)).encode(),
        env=environment,
    )
    reply = json.loads(output)
    assert reply["kind"] != "error", reply
    return reply.get("data")


def run():
    with tempfile.TemporaryDirectory(prefix="rode-sessions-ui-") as directory:
        base = Path(directory)
        root = base / "project"
        root.mkdir()
        tools = root / "tools"
        tools.mkdir()
        released = root / "released"
        queue = tools / "squeue"
        queue.write_text(
            "#!/usr/bin/python3\nfrom pathlib import Path\n"
            f"if not Path({str(released)!r}).exists():\n"
            " print('42|RUNNING|2:30|1:00:00|gpu01|cpu=16,gres/gpu=4,gres/gpu:a100=4|training')\n"
            "print('43|PENDING|0:00|2:00:00|(Resources)|gres/gpu=8|waiting')\n"
        )
        cancel = tools / "scancel"
        cancel.write_text(
            "#!/usr/bin/python3\nimport os,sys\nfrom pathlib import Path\n"
            "assert sys.argv[1:]==['--user',str(os.geteuid()),'--','42']\n"
            f'Path({str(released)!r}).write_text("42")\n'
        )
        shell = tools / "shell"
        shell.write_text("#!/bin/sh\nexec /bin/bash --noprofile --norc\n")
        for path in [queue, cancel, shell]:
            path.chmod(0o755)
        environment = dict(
            os.environ,
            XDG_CONFIG_HOME=str(base / "config"),
            XDG_DATA_HOME=str(base / "data"),
            PATH=str(tools) + os.pathsep + os.environ["PATH"],
            SHELL=str(shell),
        )
        processes, windows = [], []
        with (ARTIFACTS / "native.log").open("w") as log:

            def launch():
                process = subprocess.Popen(
                    [str(BINARY), "--theme", "carbon", str(root)],
                    env=environment,
                    stdout=log,
                    stderr=log,
                )
                processes.append(process)
                window = window_for(
                    process, [wid for proc, wid in zip(processes, windows) if proc.poll() is None]
                )
                windows.append(window)
                return process, window

            def infos():
                return rpc(environment, "list") or []

            def new_terminal(persistent=False):
                old = {info["id"] for info in infos()}
                command(
                    "Terminal: New persistent terminal" if persistent else "Terminal: New terminal"
                )
                wait_for(
                    lambda: any(info["id"] not in old for info in infos()),
                    "No new terminal",
                )
                time.sleep(0.4)
                return next(info for info in infos() if info["id"] not in old)

            try:
                process, window = launch()
                first = new_terminal()
                assert first["persistent"] is False, "New terminals must default to persistence off"
                click(600, 755)
                text("TOKEN=preserved\n")

                def persistence_button(
                    enabled, name, width=1380, height=900, accent=(220, 163, 37)
                ):
                    # The toggle must stay at the far right in both states.
                    # Move the pointer away before comparing the actual fill.
                    xtest.XTestFakeMotionEvent(display, -1, 600, 200, 0)
                    x11.XFlush(display)
                    screenshot(window, name)
                    raw = subprocess.check_output(
                        ["convert", str(ARTIFACTS / name), "-depth", "8", "rgb:-"]
                    )
                    assert len(raw) == width * height * 3
                    right = range(width - 92, width - 14)
                    header = range(98, 122) if width >= 1000 else range(124, 148)
                    filled = sum(
                        raw[(y * width + x) * 3 : (y * width + x) * 3 + 3] == bytes(accent)
                        for y in header
                        for x in right
                    )
                    assert (filled > 500) if enabled else (filled < 100), (name, filled)

                persistence_button(False, "persist-off.png")
                click(1326, 108)
                wait_for(
                    lambda: any(
                        info["id"] == first["id"] and info["persistent"] for info in infos()
                    ),
                    "The terminal Persist button did not enable persistence",
                )
                persistence_button(True, "persist-on.png")
                click(1326, 108)
                wait_for(
                    lambda: any(
                        info["id"] == first["id"] and not info["persistent"] for info in infos()
                    ),
                    "The terminal Persist button did not switch off again",
                )
                persistence_button(False, "persist-off-again.png")
                # The same shell must still accept input after both transitions.
                click(600, 755)
                text("printf '%s' \"$TOKEN\" > toggle.ok\n")
                wait_for(lambda: (root / "toggle.ok").exists(), "Switching off restarted the shell")
                assert (root / "toggle.ok").read_text() == "preserved", "Toggle replaced the shell"
                click(1326, 108)
                wait_for(
                    lambda: any(
                        info["id"] == first["id"] and info["persistent"] for info in infos()
                    ),
                    "Could not enable persistence a second time",
                )
                theme("phosphor")
                persistence_button(True, "persist-phosphor.png", accent=(125, 235, 164))
                theme("carbon")
                x11.XResizeWindow(display, window, 850, 560)
                x11.XFlush(display)
                time.sleep(0.5)
                persistence_button(True, "persist-compact.png", width=850, height=560)
                click(796, 134)
                wait_for(
                    lambda: any(
                        info["id"] == first["id"] and not info["persistent"] for info in infos()
                    ),
                    "Compact Persist button could not switch off",
                )
                persistence_button(False, "persist-compact-off.png", width=850, height=560)
                click(796, 134)
                wait_for(
                    lambda: any(
                        info["id"] == first["id"] and info["persistent"] for info in infos()
                    ),
                    "Compact Persist button could not switch on",
                )
                x11.XResizeWindow(display, window, 1380, 900)
                x11.XFlush(display)
                time.sleep(0.5)
                click(600, 755)
                # A file marker proves the shell ran the command after the view
                # and desktop process exit; split output avoids matching echo.
                text("while test ! -e resume; do sleep 0.2; done; echo done > trained.ok\n")
                time.sleep(0.2)
                second = new_terminal(persistent=True)
                text("echo independent > second.ok\n")
                wait_for(lambda: (root / "second.ok").exists(), "Second terminal did not run")
                screenshot(window, "multiple-terminals.png")
                third = new_terminal()
                assert third["persistent"] is False
                exited = new_terminal(persistent=True)
                text("sleep 8; echo done > exited.ok; exit\n")
                command("Terminal: Sessions and Slurm allocations")
                screenshot(window, "before-terminal-exit.png")
                wait_for(lambda: (root / "exited.ok").exists(), "Terminal did not exit naturally")
                wait_for(
                    lambda: all(info["id"] != exited["id"] for info in infos()),
                    "Exited persistent terminal was retained in the session service",
                )
                # The open manager must update automatically without Refresh.
                time.sleep(2.5)
                screenshot(window, "manager-allocations.png")
                key("Escape")
                request_close(window)
                assert process.wait(timeout=15) == 0
                wait_for(lambda: len(infos()) == 2, "Persistence-off terminal survived exit")
                assert all(info["alive"] and info["persistent"] for info in infos())
                assert not (root / "trained.ok").exists()
                (root / "resume").touch()
                wait_for(
                    lambda: (root / "trained.ok").exists(), "Training stopped when RODE exited"
                )
                process, window = launch()
                command("Terminal: Sessions and Slurm allocations")
                screenshot(window, "reattach-list.png")
                # First session row's Attach button (two fixed-height rows).
                for _ in range(5):
                    key("Tab")
                key("Return")
                time.sleep(0.4)
                screenshot(window, "reattached.png")
                text("echo reattached > reattached.ok\n")
                wait_for(
                    lambda: (root / "reattached.ok").exists(),
                    "Reattached terminal did not accept input",
                )
                other, other_window = launch()
                command("Terminal: Sessions and Slurm allocations")
                for _ in range(5):
                    key("Tab")
                key("Return")
                time.sleep(0.4)
                wait_for(
                    lambda: any(
                        info["id"] == first["id"] and info["clients"] == 2 for info in infos()
                    ),
                    "Two windows did not share the terminal",
                )
                text("echo shared > shared.ok\n")
                wait_for(lambda: (root / "shared.ok").exists(), "Shared view could not send input")
                screenshot(other_window, "shared-terminal.png")
                click(1326, 108)  # Turn it off in this view; both windows must agree.
                wait_for(
                    lambda: any(
                        info["id"] == first["id"] and not info["persistent"] for info in infos()
                    ),
                    "Shared persistence change did not reach the host service",
                )
                close_window(other_window, other)
                assert any(info["id"] == first["id"] and info["alive"] for info in infos())
                focus(window)
                click(600, 755)
                text("echo still-shared > still-shared.ok\n")
                wait_for(
                    lambda: (root / "still-shared.ok").exists(),
                    "Closing one view ended a shared shell",
                )
                command("Terminal: Sessions and Slurm allocations")
                screenshot(window, "before-release.png")
                # The fixture has two terminal rows, followed by job 42. Focus
                # order also includes live refresh controls; do not count Tabs.
                click(650, 538)  # Release allocation 42; confirmation comes first.
                time.sleep(0.3)
                assert not released.exists(), "Release ran without confirmation"
                screenshot(window, "release-confirmation.png")
                key("Return", ("Alt_L",))  # Confirm the exact displayed allocation.
                wait_for(released.exists, "Confirmed allocation was not released")
                screenshot(window, "allocation-released.png")
                key("Escape")
                command("Terminal: New temporary terminal")
                text("echo temporary > temporary.ok\n")
                wait_for(lambda: (root / "temporary.ok").exists(), "Temporary shell did not run")
                request_close(window)
                assert process.wait(timeout=15) == 0
                wait_for(
                    lambda: all(info["id"] != first["id"] for info in infos()),
                    "Persistence-off native terminal was not ended on final-view exit",
                )
                assert any(info["id"] == second["id"] and info["alive"] for info in infos())
                print(
                    "Native persistence toggle/default, exited-session cleanup, prompt-free mixed-session exit, reattachment, shared last-view lifetime, temporary shells and confirmed mock Slurm release passed."
                )
            except Exception:
                for process, window in zip(processes, windows):
                    if process.poll() is None:
                        focus(window)
                        screenshot(window, "failure.png")
                raise
            finally:
                for process in processes:
                    if process.poll() is None:
                        process.terminate()
                        process.wait(timeout=5)
                for info in infos():
                    rpc(environment, "end", id=info["id"], policy=info["policy"])


if __name__ == "__main__":
    run()
