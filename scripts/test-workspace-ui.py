#!/usr/bin/env python3
"""Native workspace replacement, tool-window docking and live-state preservation."""

import contextlib
import ctypes
import json
import os
import re
import shutil
import subprocess
import time
from pathlib import Path

os.environ["RODE_UI_ARTIFACTS"] = str(
    Path(__file__).resolve().parent.parent / "target/workspace-ui"
)
from ui_harness import (
    ARTIFACTS,
    BINARY,
    ROOT,
    NativeUI,
    assert_running,
    click,
    click_button,
    close_window,
    display,
    fixture_directory,
    fixture_environment,
    focus_window,
    key,
    palette_command,
    read_json_lines,
    request_close,
    screenshot,
    text,
    viewport_state_path,
    wait_for_button,
    wait_for_project,
    wait_for_window,
    x11,
)

x11.XRaiseWindow.argtypes = [ctypes.c_void_p, ctypes.c_ulong]
window_titles = {}


def focus(window):
    x11.XRaiseWindow(display, window)
    focus_window(window)


def tool_window(title):
    tree = subprocess.check_output(["xwininfo", "-root", "-tree"], text=True)
    match = re.search(r'(0x[0-9a-f]+) "RODE · ' + re.escape(title) + '"', tree)
    if not match:
        return None
    attributes = subprocess.run(["xwininfo", "-id", match[1]], capture_output=True, text=True)
    if "Map State: IsViewable" not in attributes.stdout:
        return None
    window = int(match[1], 16)
    window_titles[window] = "RODE · " + title
    return window


def tools_window():
    return tool_window("Project tools")


with fixture_directory(prefix="rode-workspace-ui-") as base:
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
    (base / "data").mkdir(mode=0o700)
    config = base / "data/rode"
    config.mkdir(parents=True)
    profile = json.dumps(["LOCAL", str(project), ""], separators=(",", ":"))
    (config / "app.ron").write_text(
        json.dumps({"agent_preferences": json.dumps({profile: {"codex": "./agent.py"}})})
    )
    state_path = base / "ui-state.json"
    environment = fixture_environment(
        base,
        XDG_CONFIG_HOME=str(base / "config"),
        XDG_DATA_HOME=str(base / "data"),
        SHELL=str(shell),
        RODE_SESSION_DIR=str(base / "runtime"),
        RODE_UI_TEST_STATE=str(state_path),
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
                return read_json_lines(project / "agent-rpc.jsonl")

            def state_for(target):
                return (
                    state_path
                    if target == window
                    else viewport_state_path(process, log_path, state_path, window_titles[target])
                )

            def driver(target=None):
                target = window if target is None else target
                return NativeUI(target, process, log_path, state_for(target), project)

            def control(name, target=None, **guards):
                target = window if target is None else target
                return wait_for_button(target, process, log_path, state_for(target), name, **guards)

            def activate(name, target=None):
                target = window if target is None else target
                click_button(target, process, log_path, state_for(target), name)

            def disabled_tab(name):
                focus(window)
                point = control("workspace-tab:" + name, enabled=False)
                before = json.loads(state_path.read_text())["pass"]
                click(*point)
                control("workspace-code-selected", after_pass=before)

            def composer(target):
                activate("agent-composer", target)
                control("agent-composer", target, focused=True)

            def send(message, target, *, idle=True):
                composer(target)
                text(message)
                driver(target).value("agent-composer-value", message, focused=True)
                key("Return")
                wait(
                    lambda: any(
                        call.get("method") == "turn/start"
                        and call["params"]["input"][0]["text"] == message
                        for call in calls()
                    ),
                    "Agent input failed after moving its window: " + message,
                )
                if idle:
                    control("agent-idle", target)

            window = wait_for_window(process, log_path)
            wait_for_project(window, process, log_path, project)
            focus(window)
            ui = driver()
            ui.open_file("main.txt")
            ui.document(source)
            key("End", ("Control_L",))
            text("\nunsaved workspace edit")
            ui.source("original source\n\nunsaved workspace edit")
            screenshot(window, "code.png")
            key("a", ("Control_L", "Shift_L"))
            control("agent-composer")
            send("before detach", window)
            screenshot(window, "chat-full-workspace.png")
            composer(window)
            text("draft across sidebar toggle")
            key("grave", ("Control_L",))
            screenshot(window, "sidebar-hidden.png")
            key("grave", ("Control_L", "Shift_L"))
            control("agent-composer", focused=True)
            key("Return")
            wait(
                lambda: any(
                    call.get("method") == "turn/start"
                    and call["params"]["input"][0]["text"] == "draft across sidebar toggle"
                    for call in calls()
                ),
                "Sidebar did not restore the chat and its draft",
            )
            control("agent-idle")
            activate("workspace-detach")
            wait(tools_window, "No native tools window was created")
            child = tools_window()
            send("while detached", child)
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
            palette_command(
                child,
                process,
                log_path,
                state_for(child),
                "Terminal: Sessions and Slurm allocations",
            )
            driver(child).layout(
                lambda state: state.get("session_ids") is not None, "Session dialog did not load"
            )
            screenshot(child, "tools-dialog.png")
            key("Escape")
            request_close(child)
            wait(lambda: tools_window() is None, "Closing tools did not dock them back")
            # Keep a real error notification over the docked chat. Its centre
            # can cover the composer even though the input remains usable.
            # The click harness must find exposed input, not dismiss the error.
            ui.open_file("missing-docking-notification-fixture.txt", expect_error=True)
            key("a", ("Control_L", "Shift_L"))
            send("after docking", window)
            ui.control("notice-dismiss")
            ui.dismiss_notice()
            assert sum(call.get("method") == "initialize" for call in calls()) == 1
            assert sum(call.get("method") == "turn/start" for call in calls()) == 4
            activate("workspace-detach")
            child = wait(tools_window, "Could not detach the queued chat")
            send("queue-gated", child, idle=False)
            wait(lambda: (project / "queue-started").exists(), "Held turn did not start")
            # The first queued turn evicts user messages from the transcript;
            # recall must still include the second accepted queued prompt.
            for message in ["history-burst", "detached queue second"]:
                composer(child)
                before = driver(child).layout()["pass"]
                text(message)
                driver(child).value("agent-composer-value", message, focused=True)
                key("Return")
                control("agent-composer-empty", child, after_pass=before)
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
            control("agent-idle", child)
            screenshot(child, "detached-queue-drained.png")
            focus(child)
            composer(child)
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
            activate("workspace-code")
            key("s", ("Control_L",))
            wait(lambda: "unsaved workspace edit" in source.read_text(), "Code edits were lost")
            screenshot(window, "code-restored.png")
            key("t", ("Control_L", "Shift_L"))
            ui.terminal_input()
            text("echo $$ > before.pid\n")
            wait(
                lambda: (project / "before.pid").exists(),
                "Integrated terminal did not accept input",
            )
            screenshot(window, "terminal-full-workspace.png")
            key("grave", ("Control_L",))
            key("grave", ("Control_L", "Shift_L"))
            ui.terminal_input()
            text("echo $$ > sidebar.pid\n")
            wait(
                lambda: (project / "sidebar.pid").exists(), "Sidebar did not restore terminal input"
            )
            assert (project / "before.pid").read_text() == (project / "sidebar.pid").read_text()
            activate("workspace-detach")
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
            activate("workspace-detach-tab:Terminal")
            terminal_window = wait(
                lambda: tool_window("Terminal"), "No independent Terminal window"
            )
            focus(terminal_window)
            text("echo $$ > tab.pid\n")
            wait(lambda: (project / "tab.pid").exists(), "Detached terminal tab lost shell input")
            assert (project / "before.pid").read_text() == (project / "tab.pid").read_text()
            palette_command(
                terminal_window,
                process,
                log_path,
                state_for(terminal_window),
                "Terminal: Sessions and Slurm allocations",
            )
            driver(terminal_window).layout(
                lambda state: state.get("session_ids") is not None, "Session dialog did not load"
            )
            screenshot(terminal_window, "individual-terminal-dialog.png")
            key("Escape")
            driver(terminal_window).terminal_input()
            text("echo $$ > tab-modal.pid\n")
            wait(
                lambda: (project / "tab-modal.pid").exists(),
                "Terminal tab did not restore input after its dialog",
            )
            assert (project / "before.pid").read_text() == (project / "tab-modal.pid").read_text()
            focus(window)
            disabled_tab("Terminal")
            screenshot(window, "disabled-terminal-tab.png")
            activate("workspace-tab:Chat")
            activate("workspace-detach-tab:Chat")
            chat_window = wait(lambda: tool_window("Chat"), "No independent Chat window")
            send("independent chat tab", chat_window)
            assert tool_window("Terminal") == terminal_window
            assert sum(call.get("method") == "initialize" for call in calls()) == 1
            screenshot(chat_window, "independent-chat-tab.png")
            focus(window)
            disabled_tab("Chat")
            activate("source-editor")
            control("source-editor", focused=True)
            key("End", ("Control_L",))
            text("\nedit with two detached tabs")
            key("s", ("Control_L",))
            screenshot(window, "two-tabs-detached.png")
            wait(
                lambda: "edit with two detached tabs" in source.read_text(),
                "Disabled tool tab replaced Code or swallowed editor input",
            )
            focus(chat_window)
            palette_command(
                chat_window,
                process,
                log_path,
                state_for(chat_window),
                "Terminal: Sessions and Slurm allocations",
            )
            driver(chat_window).layout(
                lambda state: state.get("session_ids") is not None, "Session dialog did not load"
            )
            screenshot(chat_window, "individual-tab-dialog.png")
            key("Escape")
            activate("workspace-reattach:Chat", chat_window)
            wait(lambda: tool_window("Chat") is None, "Reattach did not close the Chat tab window")
            send("after individual reattach", window)
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
            activate("workspace-detach")
            child = wait(tools_window, "Could not reopen tools before closing the main window")
            focus(child)
            activate("workspace-tab:Chat", child)
            activate("workspace-detach-tab:Chat", child)
            chat_window = wait(
                lambda: tool_window("Chat"), "Could not detach a tab from the sidebar window"
            )
            send("tab from detached sidebar", chat_window)
            assert tools_window() == child
            focus(chat_window)
            activate("workspace-reattach:Chat", chat_window)
            wait(
                lambda: tool_window("Chat") is None, "Tab did not reattach to its detached sidebar"
            )
            send("reattached to sidebar window", child)
            assert sum(call.get("method") == "initialize" for call in calls()) == 1
            activate("workspace-detach-tab:Chat", child)
            wait(lambda: tool_window("Chat"), "Could not reopen the independent Chat tab")
            close_window(window, process)
            assert tools_window() is None, "Closing the main window left an orphan tools window"
            assert tool_window("Chat") is None, "Closing the main window left an orphan tab window"
            print(
                "Workspace switching, sidebar and individual tab windows, disabled placeholders, retained chat/code and the same live shell passed."
            )
        except Exception:
            if process.poll() is None:
                with contextlib.suppress(Exception):
                    screenshot(window, "workspace-failure.png")
            for snapshot in base.glob(state_path.name + "*"):
                shutil.copy2(snapshot, ARTIFACTS / snapshot.name)
            rpc = project / "agent-rpc.jsonl"
            if rpc.exists():
                shutil.copy2(rpc, ARTIFACTS / "workspace-rpc.jsonl")
            raise
        finally:
            if process.poll() is None:
                process.terminate()
                process.wait(timeout=10)
