#!/usr/bin/env python3
"""Two native windows sharing config/data: independent edits and safe shutdown."""

import ctypes
import fcntl
import json
import os
import subprocess
import threading
import time
from pathlib import Path

os.environ["RODE_UI_ARTIFACTS"] = str(
    Path(__file__).resolve().parent.parent / "target/multiprocess-ui"
)
from ui_harness import (
    ARTIFACTS,
    BINARY,
    NativeUI,
    click,
    click_button,
    close_window,
    display,
    fixture_directory,
    fixture_environment,
    focus_window,
    key,
    screenshot,
    text,
    theme,
    wait_for_project,
    wait_for_window,
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


def window_for(process, project):
    window = wait_for_window(process, log_path)
    wait_for_project(window, process, log_path, project)
    focus(window)
    return window


def preferences(path):
    if not path.exists():
        return {}
    # Native RON map of string keys/values differs from JSON by its final comma.
    contents = path.read_text().rstrip()
    return json.loads(contents[:-1].rstrip().removesuffix(",") + "}")


def agent(window, executable):
    focus(window)
    key("a", ("Control_L", "Shift_L"))
    control(window, "agent-settings")
    control(window, "agent-codex-executable")
    key("a", ("Control_L",))
    text(executable)
    control(window, "agent-settings")


def control(window, name):
    """Resolve this process's current layout, then activate it with native X11 input."""
    index = windows.index(window)
    click_button(window, processes[index], log_path, states[index], name)


def connection_form(window, name):
    focus(window)
    key("p", ("Control_L", "Shift_L"))
    text("Remote: Connect")
    key("Return")
    control(window, "ssh-field-Host / alias")
    key("a", ("Control_L",))
    text(name + ".invalid")
    control(window, "ssh-field-Connection name")
    key("a", ("Control_L",))
    text(name)


def save_profile(window):
    focus(window)
    control(window, "ssh-save-profile")


with fixture_directory(prefix="rode-multiprocess-ui-") as base:
    roots = [base / "project-one", base / "project-two"]
    for root in roots:
        root.mkdir()
        (root / "main.txt").write_text("original file\n")
    environment = fixture_environment(
        base, XDG_CONFIG_HOME=str(base / "config"), XDG_DATA_HOME=str(base / "data")
    )
    saved = base / "data/rode/app.ron"
    connections = base / "config/rode/connections.json"
    processes, windows, states = [], [], []
    log_path = ARTIFACTS / "windows.log"
    with log_path.open("w") as log:
        try:
            for root in roots:
                states.append(base / f"ui-state-{len(states)}.json")
                process = subprocess.Popen(
                    [str(BINARY), "--sandbox", str(root)],
                    env=environment | {"RODE_UI_TEST_STATE": str(states[-1])},
                    stdout=log,
                    stderr=log,
                )
                processes.append(process)
                windows.append(window_for(process, root))
            # Both windows have already loaded the old preference snapshot.
            focus(windows[0])
            theme("carbon", ui=NativeUI(windows[0], processes[0], log_path, states[0], roots[0]))
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
            # Renaming the loaded profile updates it in place. Closing while
            # that save is queued must wait for the lock and preserve its order.
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
            profiles = json.loads(connections.read_text())
            assert [(profile["name"], profile["host"]) for profile in profiles] == [
                ("computer-one", "computer-one.invalid"),
                ("computer-final", "computer-final.invalid"),
            ], f"Queued profile rename was lost or changed another connection: {profiles!r}"
            wait_for(lambda: preferences(saved).get("color_theme") == "carbon", "Stale theme won")
            assert preferences(saved)["font_size"] == expected_font
            focus(windows[0])
            key("Escape")
            key("p", ("Control_L", "Shift_L"))
            ui = NativeUI(windows[0], processes[0], log_path, states[0], roots[0])
            ui.control("command-palette-input", focused=True)
            ui.fill("command-palette-input", "Open file by path")
            ui.control("command-match:Open file by path")
            key("Return")
            ui.control("file-dialog-path", focused=True)
            ui.fill("file-dialog-path", "main.txt")
            key("Return")
            ui.document(roots[0] / "main.txt")
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
            if connections.exists():
                (ARTIFACTS / "failed-connections.json").write_bytes(connections.read_bytes())
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
