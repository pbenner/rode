#!/usr/bin/env python3
"""Two native windows sharing config/data: independent edits and safe shutdown."""

import ctypes
import fcntl
import json
import os
import re
import subprocess
import tempfile
import threading
import time
from pathlib import Path

os.environ["RODE_UI_ARTIFACTS"] = str(
    Path(__file__).resolve().parent.parent / "target/multiprocess-ui"
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
    theme,
    x11,
)

x11.XRaiseWindow.argtypes = [ctypes.c_void_p, ctypes.c_ulong]


def wait_for(check, message):
    deadline = time.monotonic() + 10
    while time.monotonic() < deadline:
        if check():
            return
        time.sleep(0.05)
    raise AssertionError(message)


def focus(window):
    x11.XRaiseWindow(display, window)
    focus_window(window)
    time.sleep(0.25)


def window_for(process, previous):
    deadline = time.monotonic() + 20
    while time.monotonic() < deadline:
        tree = subprocess.check_output(["xwininfo", "-root", "-tree"], text=True)
        for match in re.finditer(r'(0x[0-9a-f]+) "RODE', tree):
            window = int(match[1], 16)
            if window not in previous:
                state = subprocess.run(
                    ["xwininfo", "-id", hex(window)], capture_output=True, text=True
                )
                if (
                    state.returncode
                    or "Map State: IsViewable" not in state.stdout
                    or "[Opening project]" in state.stdout
                ):
                    continue
                focus(window)
                return window
        assert process.poll() is None
        time.sleep(0.05)
    raise AssertionError("No native window")


def preferences(path):
    if not path.exists():
        return {}
    # Native RON map of string keys/values differs from JSON by its final comma.
    contents = path.read_text().rstrip()
    return json.loads(contents[:-1].rstrip().removesuffix(",") + "}")


def agent(window, executable):
    focus(window)
    key("a", ("Control_L", "Shift_L"))
    time.sleep(0.2)
    state = subprocess.check_output(["xwininfo", "-id", hex(window)], text=True)
    header = 157 if "[Sandbox]" in state else 187
    click(1213, header)
    click(700, header + 94)
    key("a", ("Control_L",))
    text(executable)
    click(1213, header)


def connection_form(window, name):
    focus(window)
    key("p", ("Control_L", "Shift_L"))
    text("Remote: Connect")
    key("Return")
    time.sleep(0.3)
    key("a", ("Control_L",))
    text(name + ".invalid")
    for _ in range(7):
        key("Tab")
    key("a", ("Control_L",))
    text(name)


def save_profile(window):
    focus(window)
    for _ in range(3):  # Name → sandbox toggle → Connect → Save profile.
        key("Tab")
    key("Return")


with tempfile.TemporaryDirectory(prefix="rode-multiprocess-ui-") as directory:
    base = Path(directory)
    roots = [base / "project-one", base / "project-two"]
    for root in roots:
        root.mkdir()
        (root / "main.txt").write_text("original file\n")
    environment = dict(
        os.environ, XDG_CONFIG_HOME=str(base / "config"), XDG_DATA_HOME=str(base / "data")
    )
    saved = base / "data/rode/app.ron"
    connections = base / "config/rode/connections.json"
    processes, windows = [], []
    with (ARTIFACTS / "windows.log").open("w") as log:
        try:
            for root in roots:
                process = subprocess.Popen(
                    [str(BINARY), "--sandbox", str(root)], env=environment, stdout=log, stderr=log
                )
                processes.append(process)
                windows.append(window_for(process, windows))
            # Both windows have already loaded the old preference snapshot.
            focus(windows[0])
            theme("carbon")
            focus(windows[1])
            click(195, 25)
            screenshot(windows[1], "font-menu.png")
            key("Escape")
            # Focus code first: the project terminal may open during startup.
            # Use the font shortcut instead of a menu row's changing coordinates.
            key("space", ("Control_L",))
            key("e")
            key("equal", ("Control_L",))
            wait_for(
                lambda: float(preferences(saved).get("font_size", "14")) > 14, "Font was not saved"
            )
            expected_font = preferences(saved)["font_size"]
            agent(windows[0], "agent-one")
            agent(windows[1], "agent-two")

            def both_agents():
                agents = json.loads(preferences(saved).get("agent_preferences", "{}"))
                return {v["codex"] for v in agents.values()} >= {"agent-one", "agent-two"}

            wait_for(both_agents, "One window overwrote the other project's agent settings")
            projects = json.loads(preferences(saved)["project_preferences"])
            assert all(any(str(root) in key for key in projects) for root in roots)
            connection_form(windows[0], "computer-one")
            connection_form(windows[1], "computer-two")
            save_profile(windows[0])
            wait_for(connections.exists, "First connection profile was not saved")
            save_profile(windows[1])
            wait_for(
                lambda: len(json.loads(connections.read_text())) == 2,
                "Second connection profile replaced the first",
            )
            key("Escape")
            connection_form(windows[1], "computer-final")
            # Closing while a save is queued must wait for it. Keep its lock
            # busy until after the close request, then release from the fixture.
            lock_file = open(connections.with_name("connections.json.lock"), "r+")
            fcntl.flock(lock_file, fcntl.LOCK_EX)
            save_profile(windows[1])

            def unlock():
                fcntl.flock(lock_file, fcntl.LOCK_UN)
                lock_file.close()

            timer = threading.Timer(0.5, unlock)
            timer.start()
            screenshot(windows[1], "second-window.png")
            close_window(windows[1], processes[1])
            timer.join()
            assert len(json.loads(connections.read_text())) == 3, (
                "Queued profile save was lost on exit"
            )
            wait_for(lambda: preferences(saved).get("color_theme") == "carbon", "Stale theme won")
            assert preferences(saved)["font_size"] == expected_font
            focus(windows[0])
            key("Escape")
            key("p", ("Control_L", "Shift_L"))
            text("Open file by path")
            key("Return")
            time.sleep(0.2)
            text("main.txt")
            key("Return")
            time.sleep(0.3)
            key("End", ("Control_L",))
            text("saved after the other window closed\n")
            key("s", ("Control_L",))
            wait_for(
                lambda: "other window closed" in (roots[0] / "main.txt").read_text(),
                "Closing another process broke the remaining window's file helper",
            )
            screenshot(windows[0], "first-window.png")
            close_window(windows[0], processes[0])
            final = preferences(saved)
            assert final["color_theme"] == "carbon" and final["font_size"] == expected_font
            assert both_agents()
            print(
                "Two native processes: themes/font, agent/project preferences, SSH profiles, and shutdown isolation passed."
            )
        except Exception:
            if saved.exists():
                (ARTIFACTS / "failed-preferences.ron").write_bytes(saved.read_bytes())
            for window, process in zip(windows, processes):
                if process.poll() is None:
                    focus(window)
                    screenshot(window, "failure-" + str(window) + ".png")
            raise
        finally:
            for process in processes:
                if process.poll() is None:
                    process.terminate()
                    process.wait(timeout=5)
