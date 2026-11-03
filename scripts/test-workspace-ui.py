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
    focus_window,
    key,
    request_close,
    screenshot,
    text,
    wait_for_project,
    wait_for_window,
    x11,
)

x11.XRaiseWindow.argtypes = [ctypes.c_void_p, ctypes.c_ulong]


def focus(window):
    x11.XRaiseWindow(display, window)
    focus_window(window)
    time.sleep(0.15)


def tool_window(title):
    tree = subprocess.check_output(["xwininfo", "-root", "-tree"], text=True)
    match = re.search(r'(0x[0-9a-f]+) "RODE · ' + re.escape(title) + '"', tree)
    if not match:
        return None
    attributes = subprocess.run(["xwininfo", "-id", match[1]], capture_output=True, text=True)
    return int(match[1], 16) if "Map State: IsViewable" in attributes.stdout else None


def tools_window():
    return tool_window("Project tools")


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
                    result = check()
                    if result:
                        return result
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
            wait_for_project(window, process, log_path, project)
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
            click(700, 790)
            text("draft across sidebar toggle")
            key("grave", ("Control_L",))
            screenshot(window, "sidebar-hidden.png")
            key("grave", ("Control_L", "Shift_L"))
            time.sleep(0.3)
            key("Return")
            wait(
                lambda: any(
                    call.get("method") == "turn/start"
                    and call["params"]["input"][0]["text"] == "draft across sidebar toggle"
                    for call in calls()
                ),
                "Sidebar did not restore the chat and its draft",
            )
            time.sleep(0.3)
            click(1073, 69)  # Detach in the workspace toolbar, not the chat-session controls.
            wait(tools_window, "No native tools window was created")
            child = tools_window()
            send("while detached", child, (450, 680))
            screenshot(child, "chat-floating.png")
            focus(window)
            screenshot(window, "code-with-chat-detached.png")
            focus(child)
            key("grave", ("Control_L",))
            wait(lambda: tools_window() is None, "Sidebar shortcut did not hide detached tools")
            focus(window)
            key("grave", ("Control_L", "Shift_L"))
            wait(tools_window, "Sidebar shortcut did not restore detached tools")
            assert tools_window() == child, "Hiding the sidebar recreated its native window"
            focus(child)
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
            assert sum(call.get("method") == "turn/start" for call in calls()) == 4
            click(1073, 69)
            child = wait(tools_window, "Could not detach the queued chat")
            send("queue-gated", child, (450, 680))
            wait(lambda: (project / "queue-started").exists(), "Held turn did not start")
            # The first queued turn evicts user messages from the transcript;
            # recall must still include the second accepted queued prompt.
            for message in ["history-burst", "detached queue second"]:
                click(450, 680)
                text(message)
                key("Return")
                time.sleep(0.3)
            focus(window)
            (project / "queue-continue").touch()
            wait(
                lambda: sum(call.get("method") == "turn/start" for call in calls()) == 7,
                "The detached chat did not process every queued prompt",
            )
            prompts = [
                call["params"]["input"][0]["text"]
                for call in calls()
                if call.get("method") == "turn/start"
            ]
            assert prompts[-3:] == [
                "queue-gated",
                "history-burst",
                "detached queue second",
            ], prompts
            screenshot(child, "detached-queue-drained.png")
            focus(child)
            click(450, 680)
            key("Up")
            key("Return")
            wait(
                lambda: sum(call.get("method") == "turn/start" for call in calls()) == 8,
                "The detached queue was missing from prompt recall",
            )
            assert [
                call["params"]["input"][0]["text"]
                for call in calls()
                if call.get("method") == "turn/start"
            ][-1] == "detached queue second"
            request_close(child)
            wait(lambda: tools_window() is None, "Queued chat did not dock back")
            focus(window)
            click(339, 69)  # Code replaces tools again, preserving the unsaved buffer.
            key("s", ("Control_L",))
            wait(lambda: "unsaved workspace edit" in source.read_text(), "Code edits were lost")
            screenshot(window, "code-restored.png")
            key("t", ("Control_L", "Shift_L"))
            time.sleep(0.8)
            text("echo $$ > before.pid\n")
            wait(
                lambda: (project / "before.pid").exists(),
                "Integrated terminal did not accept input",
            )
            screenshot(window, "terminal-full-workspace.png")
            key("grave", ("Control_L",))
            key("grave", ("Control_L", "Shift_L"))
            time.sleep(0.3)
            text("echo $$ > sidebar.pid\n")
            wait(
                lambda: (project / "sidebar.pid").exists(), "Sidebar did not restore terminal input"
            )
            assert (project / "before.pid").read_text() == (project / "sidebar.pid").read_text()
            click(1073, 69)
            wait(tools_window, "Terminal could not open in the tools window")
            child = tools_window()
            focus(child)
            screenshot(child, "terminal-floating.png")
            focus(window)
            screenshot(window, "code-with-terminal-detached.png")
            focus(child)
            request_close(child)
            wait(lambda: tools_window() is None, "Terminal window did not dock back")
            focus(window)
            text("echo $$ > after.pid\n")
            wait(lambda: (project / "after.pid").exists(), "Docking lost terminal input focus")
            assert (project / "before.pid").read_text() == (project / "after.pid").read_text()

            # Independent tab windows share the existing shell and agent controller.
            click(1155, 69)  # Detach tab, beside the full-sidebar Detach action.
            terminal_window = wait(
                lambda: tool_window("Terminal"), "No independent Terminal window"
            )
            focus(terminal_window)
            text("echo $$ > tab.pid\n")
            wait(lambda: (project / "tab.pid").exists(), "Detached terminal tab lost shell input")
            assert (project / "before.pid").read_text() == (project / "tab.pid").read_text()
            key("p", ("Control_L", "Shift_L"))
            text("Terminal: Sessions and Slurm allocations")
            key("Return")
            time.sleep(0.3)
            screenshot(terminal_window, "individual-terminal-dialog.png")
            key("Escape")
            time.sleep(0.3)
            text("echo $$ > tab-modal.pid\n")
            wait(
                lambda: (project / "tab-modal.pid").exists(),
                "Terminal tab did not restore input after its dialog",
            )
            assert (project / "before.pid").read_text() == (project / "tab-modal.pid").read_text()
            focus(window)
            click(400, 69)  # Disabled Terminal tab must not take over the docked surface.
            screenshot(window, "disabled-terminal-tab.png")
            key("a", ("Control_L", "Shift_L"))
            time.sleep(0.3)
            click(1155, 69)
            chat_window = wait(lambda: tool_window("Chat"), "No independent Chat window")
            send("independent chat tab", chat_window, (450, 680))
            assert tool_window("Terminal") == terminal_window
            assert sum(call.get("method") == "initialize" for call in calls()) == 1
            screenshot(chat_window, "independent-chat-tab.png")
            focus(window)
            click(460, 69)  # Chat stays visible but disabled; Code remains selected.
            click(700, 180)  # Code must still be available after the disabled tab click.
            key("End", ("Control_L",))
            text("\nedit with two detached tabs")
            key("s", ("Control_L",))
            screenshot(window, "two-tabs-detached.png")
            wait(
                lambda: "edit with two detached tabs" in source.read_text(),
                "Disabled tool tab replaced Code or swallowed editor input",
            )
            focus(chat_window)
            key("p", ("Control_L", "Shift_L"))
            text("Terminal: Sessions and Slurm allocations")
            key("Return")
            time.sleep(0.3)
            screenshot(chat_window, "individual-tab-dialog.png")
            key("Escape")
            click(845, 14)  # Reattach in the individual window's header.
            wait(lambda: tool_window("Chat") is None, "Reattach did not close the Chat tab window")
            send("after individual reattach", window, (700, 790))
            assert tool_window("Terminal") == terminal_window
            focus(terminal_window)
            request_close(terminal_window)
            wait(
                lambda: tool_window("Terminal") is None,
                "Closing an independent tab did not reattach it",
            )
            focus(window)
            text("echo $$ > tab-docked.pid\n")
            wait(lambda: (project / "tab-docked.pid").exists(), "Tab reattach lost terminal input")
            assert (project / "before.pid").read_text() == (project / "tab-docked.pid").read_text()
            click(1073, 69)
            child = wait(tools_window, "Could not reopen tools before closing the main window")
            focus(child)
            click(170, 22)  # Chat in the detached whole-sidebar window.
            click(55, 55)  # Detach tab wraps below the tab row at this window width.
            chat_window = wait(
                lambda: tool_window("Chat"), "Could not detach a tab from the sidebar window"
            )
            send("tab from detached sidebar", chat_window, (450, 680))
            assert tools_window() == child
            focus(chat_window)
            click(845, 14)
            wait(
                lambda: tool_window("Chat") is None, "Tab did not reattach to its detached sidebar"
            )
            send("reattached to sidebar window", child, (450, 680))
            assert sum(call.get("method") == "initialize" for call in calls()) == 1
            click(55, 55)
            wait(lambda: tool_window("Chat"), "Could not reopen the independent Chat tab")
            close_window(window, process)
            assert tools_window() is None, "Closing the main window left an orphan tools window"
            assert tool_window("Chat") is None, "Closing the main window left an orphan tab window"
            print(
                "Workspace switching, sidebar and individual tab windows, disabled placeholders, retained chat/code and the same live shell passed."
            )
        finally:
            if process.poll() is None:
                process.terminate()
                process.wait(timeout=10)
