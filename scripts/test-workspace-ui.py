#!/usr/bin/env python3
"""Native workspace replacement, tool-window docking and live-state preservation."""

import ctypes
import json
import os
import re
import shutil
import subprocess
import tempfile
import time
from pathlib import Path

os.environ["RODE_UI_ARTIFACTS"] = str(
    Path(__file__).resolve().parent.parent / "target/workspace-ui"
)
from ui_harness import (
    ARTIFACTS,
    BINARY,
    ROOT,
    assert_running,
    click,
    close_window,
    display,
    key,
    request_close,
    screenshot,
    text,
    wait_for_window,
    x11,
)

x11.XRaiseWindow.argtypes = [ctypes.c_void_p, ctypes.c_ulong]


def focus(window):
    x11.XRaiseWindow(display, window)
    x11.XSetInputFocus(display, window, 1, 0)
    x11.XFlush(display)
    time.sleep(0.15)


def tools_window():
    tree = subprocess.check_output(["xwininfo", "-root", "-tree"], text=True)
    match = re.search(r'(0x[0-9a-f]+) "RODE · Project tools"', tree)
    if not match:
        return None
    attributes = subprocess.run(["xwininfo", "-id", match[1]], capture_output=True, text=True)
    return int(match[1], 16) if "Map State: IsViewable" in attributes.stdout else None


with tempfile.TemporaryDirectory(prefix="rode-workspace-ui-") as directory:
    base = Path(directory)
    project = base / "project"
    project.mkdir()
    source = project / "main.txt"
    source.write_text("original source\n")
    (base / "private.txt").write_text("fixture")
    shutil.copy2(ROOT / "tests/fixtures/agent.py", project / "agent.py")
    (project / "agent.py").chmod(0o755)
    shell = base / "shell"
    shell.write_text("#!/bin/sh\nexec /bin/bash --noprofile --norc\n")
    shell.chmod(0o755)
    config = base / "data/rode"
    config.mkdir(parents=True)
    profile = json.dumps(["LOCAL", str(project), ""], separators=(",", ":"))
    (config / "app.ron").write_text(
        json.dumps({"agent_preferences": json.dumps({profile: {"codex": "./agent.py"}})})
    )
    environment = dict(
        os.environ,
        XDG_CONFIG_HOME=str(base / "config"),
        XDG_DATA_HOME=str(base / "data"),
        SHELL=str(shell),
    )
    log_path = ARTIFACTS / "workspace.log"
    with log_path.open("w") as log:
        process = subprocess.Popen(
            [str(BINARY), str(project)], env=environment, stdout=log, stderr=log
        )
        try:

            def wait(check, message):
                deadline = time.monotonic() + 15
                while time.monotonic() < deadline:
                    assert_running(process, log_path)
                    if check():
                        return
                    time.sleep(0.05)
                raise AssertionError(message)

            def calls():
                path = project / "agent-rpc.jsonl"
                return (
                    [json.loads(line) for line in path.read_text().splitlines()]
                    if path.exists()
                    else []
                )

            def send(message, window, point):
                focus(window)
                click(*point)
                text(message)
                key("Return")
                wait(
                    lambda: any(
                        call.get("method") == "turn/start"
                        and call["params"]["input"][0]["text"] == message
                        for call in calls()
                    ),
                    "Agent input failed after moving its window: " + message,
                )
                time.sleep(0.3)

            window = wait_for_window(process, log_path)
            focus(window)
            key("p", ("Control_L",))
            text("main.txt")
            key("Return")
            time.sleep(0.4)
            key("End", ("Control_L",))
            text("\nunsaved workspace edit")
            screenshot(window, "code.png")
            key("a", ("Control_L", "Shift_L"))
            time.sleep(0.3)
            send("before detach", window, (700, 790))
            screenshot(window, "chat-full-workspace.png")
            click(980, 69)  # Detach in the workspace toolbar, not the chat-session controls.
            wait(tools_window, "No native tools window was created")
            child = tools_window()
            send("while detached", child, (450, 680))
            screenshot(child, "chat-floating.png")
            key("p", ("Control_L", "Shift_L"))
            text("Terminal: Sessions and Slurm allocations")
            key("Return")
            time.sleep(0.3)
            screenshot(child, "tools-dialog.png")
            key("Escape")
            request_close(child)
            wait(lambda: tools_window() is None, "Closing tools did not dock them back")
            send("after docking", window, (700, 790))
            assert sum(call.get("method") == "initialize" for call in calls()) == 1
            assert sum(call.get("method") == "turn/start" for call in calls()) == 3
            click(339, 69)  # Code replaces tools again, preserving the unsaved buffer.
            key("s", ("Control_L",))
            wait(lambda: "unsaved workspace edit" in source.read_text(), "Code edits were lost")
            screenshot(window, "code-restored.png")
            key("grave", ("Control_L",))
            time.sleep(0.8)
            text("echo $$ > before.pid\n")
            wait(
                lambda: (project / "before.pid").exists(),
                "Integrated terminal did not accept input",
            )
            screenshot(window, "terminal-full-workspace.png")
            click(980, 69)
            wait(tools_window, "Terminal could not open in the tools window")
            child = tools_window()
            focus(child)
            screenshot(child, "terminal-floating.png")
            request_close(child)
            wait(lambda: tools_window() is None, "Terminal window did not dock back")
            focus(window)
            text("echo $$ > after.pid\n")
            wait(lambda: (project / "after.pid").exists(), "Docking lost terminal input focus")
            assert (project / "before.pid").read_text() == (project / "after.pid").read_text()
            click(980, 69)
            wait(tools_window, "Could not reopen tools before closing the main window")
            close_window(window, process)
            assert tools_window() is None, "Closing the main window left an orphan tools window"
            print(
                "Workspace switching, native tools docking, retained chat/code and the same live shell passed."
            )
        finally:
            if process.poll() is None:
                process.terminate()
                process.wait(timeout=10)
