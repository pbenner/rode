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
    click_button,
    close_window,
    display,
    focus_window,
    key,
    palette_command,
    request_close,
    screenshot,
    text,
    wait_for_button,
    wait_for_project,
    wait_for_ui_state,
    x11,
    xtest,
)

x11.XRaiseWindow.argtypes = [ctypes.c_void_p, ctypes.c_ulong]
x11.XResizeWindow.argtypes = [ctypes.c_void_p, ctypes.c_ulong, ctypes.c_uint, ctypes.c_uint]


def wait_for(check, message):
    deadline = time.monotonic() + 12
    while time.monotonic() < deadline:
        result = check()
        if result:
            return result
        time.sleep(0.05)
    raise AssertionError(message)


def focus(window):
    x11.XRaiseWindow(display, window)
    focus_window(window)


def window_for(process, previous):
    for _ in range(150):
        tree = subprocess.check_output(["xwininfo", "-root", "-tree"], text=True)
        for match in re.finditer(r'(0x[0-9a-f]+) "RODE', tree):
            window = int(match[1], 16)
            if window not in previous:
                focus(window)
                return window
        assert process.poll() is None
        time.sleep(0.05)
    raise AssertionError("No native RODE window")


def wait_for_marker(path, expected, message):
    # Creation can precede the write. Only complete shell output acknowledges a
    # command; keyboard echo or an empty file is not evidence of execution.
    def complete():
        try:
            return path.read_text() == expected
        except FileNotFoundError:
            return False

    wait_for(complete, message)


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
            RODE_SESSION_DIR=str(base / "sessions"),
        )
        processes, windows = [], []
        state_paths = {}
        with (ARTIFACTS / "native.log").open("w") as log:

            def launch():
                state_path = base / f"ui-state-{len(processes)}.json"
                process = subprocess.Popen(
                    [str(BINARY), "--theme", "carbon", str(root)],
                    env=dict(environment, RODE_UI_TEST_STATE=str(state_path)),
                    stdout=log,
                    stderr=log,
                )
                processes.append(process)
                state_paths[process.pid] = state_path
                window = window_for(
                    process, [wid for proc, wid in zip(processes, windows) if proc.poll() is None]
                )
                windows.append(window)
                wait_for_project(window, process, ARTIFACTS / "native.log", root)
                return process, window

            def infos():
                return rpc(environment, "list") or []

            def control(name, proc=None, wid=None, *, focused=False):
                proc, wid = proc or process, wid or window
                return wait_for_button(
                    wid,
                    proc,
                    ARTIFACTS / "native.log",
                    state_paths[proc.pid],
                    name,
                    timeout=12,
                    focused=focused,
                )

            def click_control(name, proc=None, wid=None):
                proc, wid = proc or process, wid or window
                click_button(wid, proc, ARTIFACTS / "native.log", state_paths[proc.pid], name, 12)

            def command(value, proc=None, wid=None):
                proc, wid = proc or process, wid or window
                palette_command(wid, proc, ARTIFACTS / "native.log", state_paths[proc.pid], value)

            def ui_state(check, message):
                return wait_for_ui_state(
                    process,
                    ARTIFACTS / "native.log",
                    state_paths[process.pid],
                    check,
                    message,
                )

            def terminal_ready(info, clients=1, proc=None, wid=None, *, refocus=False):
                wait_for(
                    lambda: any(
                        item["id"] == info["id"] and item["alive"] and item["clients"] == clients
                        for item in infos()
                    ),
                    f"Terminal {info['id']} did not acquire {clients} attached view(s)",
                )
                name = "terminal-input:" + info["id"]
                if refocus:
                    click_control(name, proc, wid)
                control(name, proc, wid, focused=True)

            def shell_command(value, marker, expected):
                text(value + "\n")
                wait_for_marker(root / marker, expected, f"Shell did not acknowledge {marker}")

            def new_terminal(persistent=False):
                old = {info["id"] for info in infos()}
                command(
                    "Terminal: New persistent terminal" if persistent else "Terminal: New terminal"
                )
                wait_for(
                    lambda: any(info["id"] not in old for info in infos()),
                    "No new terminal",
                )
                created = [info for info in infos() if info["id"] not in old]
                assert len(created) == 1, f"Expected exactly one new terminal: {created}"
                terminal_ready(created[0])
                return created[0]

            try:
                process, window = launch()
                first = new_terminal()
                assert first["persistent"] is False, "New terminals must default to persistence off"
                shell_command("TOKEN=preserved; echo ready > first.ok", "first.ok", "ready\n")

                def persistence_button(
                    enabled, name, width=1380, height=900, accent=(220, 163, 37)
                ):
                    # The toggle must stay at the far right in both states.
                    # Move the pointer away before comparing the actual fill.
                    control("terminal-persist:" + ("on" if enabled else "off"))
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
                click_control("terminal-persist:off")
                wait_for(
                    lambda: any(
                        info["id"] == first["id"] and info["persistent"] for info in infos()
                    ),
                    "The terminal Persist button did not enable persistence",
                )
                persistence_button(True, "persist-on.png")
                click_control("terminal-persist:on")
                wait_for(
                    lambda: any(
                        info["id"] == first["id"] and not info["persistent"] for info in infos()
                    ),
                    "The terminal Persist button did not switch off again",
                )
                persistence_button(False, "persist-off-again.png")
                # The same shell must still accept input after both transitions.
                terminal_ready(first, refocus=True)
                shell_command("printf '%s' \"$TOKEN\" > toggle.ok", "toggle.ok", "preserved")
                click_control("terminal-persist:off")
                wait_for(
                    lambda: any(
                        info["id"] == first["id"] and info["persistent"] for info in infos()
                    ),
                    "Could not enable persistence a second time",
                )
                control("terminal-persist:on")
                command("Theme: Phosphor")
                persistence_button(True, "persist-phosphor.png", accent=(125, 235, 164))
                command("Theme: Carbon")
                x11.XResizeWindow(display, window, 850, 560)
                x11.XFlush(display)
                persistence_button(True, "persist-compact.png", width=850, height=560)
                click_control("terminal-persist:on")
                wait_for(
                    lambda: any(
                        info["id"] == first["id"] and not info["persistent"] for info in infos()
                    ),
                    "Compact Persist button could not switch off",
                )
                persistence_button(False, "persist-compact-off.png", width=850, height=560)
                click_control("terminal-persist:off")
                wait_for(
                    lambda: any(
                        info["id"] == first["id"] and info["persistent"] for info in infos()
                    ),
                    "Compact Persist button could not switch on",
                )
                x11.XResizeWindow(display, window, 1380, 900)
                x11.XFlush(display)
                terminal_ready(first, refocus=True)
                # Acknowledge the command before switching terminals, then keep
                # it blocked until its view and desktop process have exited.
                shell_command(
                    "echo ready > trained.ready; while test ! -e resume; do sleep 0.2; done; "
                    "echo done > trained.ok",
                    "trained.ready",
                    "ready\n",
                )
                second = new_terminal(persistent=True)
                shell_command("echo independent > second.ok", "second.ok", "independent\n")
                screenshot(window, "multiple-terminals.png")
                third = new_terminal()
                assert third["persistent"] is False
                exited = new_terminal(persistent=True)
                shell_command(
                    "echo ready > exited.ready; while test ! -e exit-now; do sleep 0.2; done; "
                    "echo done > exited.ok; exit",
                    "exited.ready",
                    "ready\n",
                )
                command("Terminal: Sessions and Slurm allocations")
                expected_ids = {info["id"] for info in (first, second, third, exited)}
                ui_state(
                    lambda state: set(state.get("session_ids") or []) == expected_ids,
                    "Manager did not display all four sessions before natural exit",
                )
                screenshot(window, "before-terminal-exit.png")
                assert not (root / "exited.ok").exists(), "Exit gate was bypassed"
                (root / "exit-now").touch()
                wait_for_marker(root / "exited.ok", "done\n", "Acknowledged shell did not exit")
                wait_for(
                    lambda: all(info["id"] != exited["id"] for info in infos()),
                    "Exited persistent terminal was retained in the session service",
                )
                # The open manager must update automatically without Refresh.
                ui_state(
                    lambda state: (
                        set(state.get("session_ids") or []) == expected_ids - {exited["id"]}
                    ),
                    "Open manager did not automatically remove the exited session",
                )
                screenshot(window, "manager-allocations.png")
                key("Escape")
                request_close(window)
                assert process.wait(timeout=15) == 0
                wait_for(
                    lambda: {info["id"] for info in infos()} == {first["id"], second["id"]},
                    "Closing the window retained the wrong sessions",
                )
                assert all(info["alive"] and info["persistent"] for info in infos())
                assert not (root / "trained.ok").exists()
                (root / "resume").touch()
                wait_for_marker(root / "trained.ok", "done\n", "Training stopped when RODE exited")
                process, window = launch()
                command("Terminal: Sessions and Slurm allocations")
                screenshot(window, "reattach-list.png")
                click_control("session-attach:" + first["id"])
                terminal_ready(first)
                screenshot(window, "reattached.png")
                shell_command(
                    "printf '%s' \"$TOKEN\" > reattached.ok", "reattached.ok", "preserved"
                )
                other, other_window = launch()
                command("Terminal: Sessions and Slurm allocations", other, other_window)
                click_control("session-attach:" + first["id"], other, other_window)
                terminal_ready(first, 2, other, other_window)
                shell_command("printf '%s' \"$TOKEN\" > shared.ok", "shared.ok", "preserved")
                screenshot(other_window, "shared-terminal.png")
                click_control("terminal-persist:on", other, other_window)
                wait_for(
                    lambda: any(
                        info["id"] == first["id"] and not info["persistent"] for info in infos()
                    ),
                    "Shared persistence change did not reach the host service",
                )
                close_window(other_window, other)
                assert any(info["id"] == first["id"] and info["alive"] for info in infos())
                focus(window)
                terminal_ready(first, refocus=True)
                shell_command(
                    "printf '%s' \"$TOKEN\" > still-shared.ok", "still-shared.ok", "preserved"
                )
                command("Terminal: Sessions and Slurm allocations")
                screenshot(window, "before-release.png")
                # Refresh and removed terminal rows can move the allocation
                # within the scroller. Click its current visible control.
                click_control("Release allocation")
                control("session-confirm")
                assert not released.exists(), "Release ran without confirmation"
                screenshot(window, "release-confirmation.png")
                key("Return", ("Alt_L",))  # Confirm the exact displayed allocation.
                wait_for_marker(released, "42", "Confirmed allocation was not released")
                screenshot(window, "allocation-released.png")
                key("Escape")
                ui_state(
                    lambda state: state.get("session_ids") is None, "Sessions manager did not close"
                )
                command("Terminal: New temporary terminal")
                control("temporary-terminal-input", focused=True)
                shell_command("echo temporary > temporary.ok", "temporary.ok", "temporary\n")
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
                # Preserve process-specific state before TemporaryDirectory is
                # removed. Diagnostic failures must not replace the first error.
                try:
                    for pid, path in state_paths.items():
                        if path.exists():
                            (ARTIFACTS / f"failure-ui-{pid}.json").write_bytes(path.read_bytes())
                    (ARTIFACTS / "failure-sessions.json").write_text(json.dumps(infos(), indent=2))
                    (ARTIFACTS / "failure-markers.json").write_text(
                        json.dumps(
                            {
                                path.name: path.read_text()
                                for path in root.iterdir()
                                if path.is_file()
                            },
                            indent=2,
                        )
                    )
                except Exception as error:
                    print(f"Could not retain session diagnostics: {error}")
                for process, window in zip(processes, windows):
                    if process.poll() is None:
                        try:
                            focus(window)
                            screenshot(window, f"failure-{process.pid}.png")
                        except Exception as error:
                            print(f"Could not capture window {process.pid}: {error}")
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
