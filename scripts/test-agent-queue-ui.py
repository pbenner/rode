#!/usr/bin/env python3
"""Native queued follow-ups with a deterministic agent and no inference."""

import hashlib
import json
import os
import shutil
import subprocess
import sys
import time
from pathlib import Path

os.environ["RODE_UI_ARTIFACTS"] = str(
    Path(__file__).resolve().parent.parent / "target/agent-queue-ui"
)
from ui_harness import (
    ARTIFACTS,
    BINARY,
    ROOT,
    NativeUI,
    assert_running,
    close_window,
    display,
    fixture_directory,
    fixture_environment,
    focus_window,
    key,
    read_json_lines,
    screenshot,
    stop_fixture_agents,
    stop_process,
    wait_for_project,
    wait_for_window,
    x11,
    xtest,
)

with fixture_directory(prefix="rode-agent-queue-ui-") as base:
    environment = fixture_environment(base, RODE_SESSION_DIR=str(base / "runtime"))
    project = base / "project"
    project.mkdir()
    shutil.copy2(ROOT / "tests/fixtures/agent.py", project / "agent.py")
    (project / "agent.py").chmod(0o755)
    config = base / "data/rode"
    config.mkdir(parents=True)
    profile = json.dumps(["LOCAL", str(project), ""], separators=(",", ":"))
    (config / "app.ron").write_text(
        json.dumps({"agent_preferences": json.dumps({profile: {"codex": "./agent.py"}})})
    )
    log_path = ARTIFACTS / "queue.log"
    with log_path.open("w") as log:
        process = subprocess.Popen(
            [str(BINARY), str(project)], env=environment, stdout=log, stderr=log
        )
        window = None
        try:

            def wait(check, message):
                deadline = time.monotonic() + 15
                while time.monotonic() < deadline:
                    assert_running(process, log_path)
                    if check():
                        return
                    time.sleep(0.05)
                raise AssertionError(message)

            def prompts():
                calls = read_json_lines(project / "agent-rpc.jsonl")
                return [
                    call["params"]["input"][0]["text"]
                    for call in calls
                    if call.get("method") == "turn/start"
                ]

            def send(message):
                ui.click("agent-composer")
                ui.fill("agent-composer", message)
                ui.control("agent-send")
                before = ui.layout()["pass"]
                key("Return")
                ui.control("agent-composer-empty", after_pass=before)

            def queued(index, message):
                digest = hashlib.sha256(message.encode()).hexdigest()
                ui.control(f"agent-queue-{index}-value:" + digest)

            def panel_height(state=None):
                state = state or ui.layout()
                panel = next(
                    b["rect"] for b in state["buttons"] if b["name"] == "agent-queue-panel"
                )
                return panel[3] - panel[1]

            def aligned_rows(compact=False):
                buttons = {b["name"]: b["rect"] for b in ui.layout()["buttons"]}
                for index, message in enumerate(["first queued", "second queued", "delete this"]):
                    preview = buttons[
                        f"agent-queue-{index}-value:" + hashlib.sha256(message.encode()).hexdigest()
                    ]
                    row = buttons[f"agent-queue-row-{index}"]
                    actions = [
                        buttons[f"agent-queue-{index}-{action}"]
                        for action in ["earlier", "later", "edit", "delete"]
                    ]
                    assert all(
                        row[0] <= b[0] < b[2] <= row[2] and row[1] <= b[1] < b[3] <= row[3]
                        for b in actions
                    ), (row, actions)
                    assert all(a[2] <= b[0] for a, b in zip(actions, actions[1:])), actions
                    if compact:
                        assert preview[3] <= min(b[1] for b in actions), (preview, actions)
                    else:
                        assert preview[2] <= actions[0][0], (preview, actions)

            window = wait_for_window(process, log_path)
            wait_for_project(window, process, log_path, project)
            focus_window(window)
            ui = NativeUI(window, process, log_path, base / "ui-state.json", project)
            key("a", ("Control_L", "Shift_L"))
            ui.control("agent-composer")
            send("wait")
            wait(lambda: prompts() == ["wait"], "Initial turn was not started")
            send("first queued")
            queued(0, "first queued")
            send("second queued")
            queued(1, "second queued")
            send("delete this")
            queued(2, "delete this")
            assert prompts() == ["wait"], "A queued prompt interrupted the running turn"
            aligned_rows()
            screenshot(window, "queued.png")
            original_height = panel_height()
            # Start from the composer: the first key after a keyboard-only
            # focus handoff must resize instead of selecting another control.
            ui.hint("Queued prompts height")
            ui.press("agent-queue-resize", "Up")
            ui.layout(
                lambda state: panel_height(state) >= original_height + 18,
                "First keyboard resize lost focus",
            )
            ui.press("agent-queue-resize", "Home")
            ui.layout(
                lambda state: abs(panel_height(state) - original_height) <= 2,
                "Queue height did not reset",
            )
            x, y = ui.control("agent-queue-resize", hovered=True)
            xtest.XTestFakeMotionEvent(display, -1, x, y, 0)
            xtest.XTestFakeButtonEvent(display, 1, 1, 0)
            x11.XFlush(display)
            ui.control("agent-queue-resize-active")
            for distance in [20, 40, 60]:
                xtest.XTestFakeMotionEvent(display, -1, x, y - distance, 0)
                x11.XFlush(display)
                ui.layout(
                    lambda state: panel_height(state) >= original_height + distance - 2,
                    "Queue divider did not move",
                )
            xtest.XTestFakeButtonEvent(display, 1, 0, 0)
            x11.XFlush(display)
            ui.absent("agent-queue-resize-active")
            ui.hint("Queued prompts height")
            ui.press("agent-queue-resize", "Home")
            ui.layout(
                lambda state: abs(panel_height(state) - original_height) <= 2,
                "Queue height did not reset",
            )
            for step in range(1, 7):
                ui.press("agent-queue-resize", "Up")
                ui.layout(
                    lambda state: panel_height(state) >= original_height + step * 20 - 2,
                    "Keyboard queue resizing failed",
                )
            preferred_height = panel_height()
            ui.resize(850, 560)
            bounds = {b["name"]: b["rect"] for b in ui.layout()["buttons"]}
            assert bounds["agent-queue-panel"][3] <= bounds["agent-composer"][1], bounds
            ui.resize(1380, 900)
            ui.layout(
                lambda state: abs(panel_height(state) - preferred_height) <= 2,
                "Compact window discarded queue height",
            )
            ui.resize(850, 900)
            # Startup intentionally hides Explorer. A narrow native window is
            # not itself a narrow chat pane; explicitly create that layout.
            ui.navigate("f")
            ui.control("pane-Explorer")
            ui.layout(
                lambda state: (
                    next(
                        b["rect"][2] - b["rect"][0]
                        for b in state["buttons"]
                        if b["name"] == "agent-queue-panel"
                    )
                    < 600
                ),
                "Explorer did not provide a compact queue pane",
            )
            aligned_rows(compact=True)
            screenshot(window, "compact-queue.png")
            ui.resize(1380, 900)
            assert prompts() == ["wait"], "Resizing submitted queued work"
            ui.click("agent-queue-1-earlier")
            queued(0, "second queued")
            ui.click("agent-queue-0-edit")
            screenshot(window, "editing.png")
            ui.click("agent-queue-edit-input")
            long_edit = "queued edit line\n" * 24 + "edited second queued"
            ui.fill("agent-queue-edit-input", long_edit)
            ui.resize(850, 560)
            ui.resize(1380, 900)
            ui.control(
                "agent-queue-edit-input-value:" + hashlib.sha256(long_edit.encode()).hexdigest()
            )
            ui.fill("agent-queue-edit-input", "edited second queued")
            ui.click("agent-queue-save")
            ui.absent("agent-queue-edit-input")
            queued(0, "edited second queued")
            ui.click("agent-queue-2-delete")
            ui.absent("agent-queue-2-delete")
            ui.click("agent-composer")
            ui.fill("agent-composer", "untouched composer draft")
            ui.click("agent-stop")
            ui.control("agent-queue-resume")
            assert prompts() == ["wait"], "Stop started the next queued prompt"
            screenshot(window, "paused.png")
            ui.click("agent-queue-resume")
            wait(lambda: len(prompts()) == 3, "The queue did not resume and drain automatically")
            assert prompts() == ["wait", "edited second queued", "first queued"], prompts()
            ui.absent("agent-queue-resume")
            ui.control("agent-idle")
            ui.click("agent-composer")
            key("Return")
            wait(
                lambda: len(prompts()) == 4,
                "The original composer draft was lost during queue editing",
            )
            assert prompts()[-1] == "untouched composer draft"
            screenshot(window, "drained.png")

            send("wait")
            wait(lambda: prompts()[-1] == "wait", "Second held turn did not start")
            send("bad-protocol")
            queued(0, "bad-protocol")
            send("after failure")
            queued(1, "after failure")
            ui.click("agent-stop")
            ui.click("agent-queue-resume")
            wait(lambda: prompts()[-1] == "bad-protocol", "Failed-turn fixture was not dispatched")
            queued(0, "after failure")
            # A broken protocol pauses pending work and disables Resume until
            # the backend is usable again. New chat remains available.
            ui.control("agent-queue-resume", enabled=False)
            assert "after failure" not in prompts(), "Failure did not pause the queue"
            screenshot(window, "failed-paused.png")
            ui.click("agent-new-chat")
            ui.absent("agent-queue-resume")
            ui.control("agent-idle")
            send("new conversation")
            wait(lambda: prompts()[-1] == "new conversation", "New chat did not accept input")
            ui.control("agent-idle")
            assert "after failure" not in prompts(), (
                "A queued prompt leaked into a new conversation"
            )
            close_window(window, process)
            print(
                "Queued prompts: mouse/keyboard resizing, compact rows, long edits, FIFO, reordering, deletion, Stop/failure pause, draft preservation and conversation isolation passed."
            )
        except BaseException:
            if window and process.poll() is None:
                screenshot(window, "failure.png")
            raise
        finally:
            stop_process(process)
            if not stop_fixture_agents(environment, project) and sys.exc_info()[0] is None:
                raise AssertionError("The fixture's agent service did not clean up")
