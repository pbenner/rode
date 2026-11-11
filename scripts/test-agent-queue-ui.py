#!/usr/bin/env python3
"""Native queued follow-ups with a deterministic agent and no inference."""

import hashlib
import json
import os
import shutil
import subprocess
import sys
import tempfile
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
    focus_window,
    key,
    read_json_lines,
    screenshot,
    stop_fixture_agents,
    stop_process,
    wait_for_project,
    wait_for_window,
)

with tempfile.TemporaryDirectory(prefix="rode-agent-queue-ui-") as directory:
    base = Path(directory)
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
    environment = dict(
        os.environ,
        XDG_CONFIG_HOME=str(base / "config"),
        XDG_DATA_HOME=str(base / "data"),
        RODE_UI_TEST_STATE=str(base / "ui-state.json"),
        RODE_SESSION_DIR=str(base / "runtime"),
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
            screenshot(window, "queued.png")
            # The bounded queue sits immediately above the unchanged composer.
            ui.click("agent-queue-1-earlier")
            queued(0, "second queued")
            ui.click("agent-queue-0-edit")
            screenshot(window, "editing.png")
            ui.click("agent-queue-edit-input")
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
                "Queued prompts: FIFO, editing, reordering, deletion, Stop/failure pause, draft preservation and conversation isolation passed."
            )
        except BaseException:
            if window and process.poll() is None:
                screenshot(window, "failure.png")
            raise
        finally:
            stop_process(process)
            if not stop_fixture_agents(environment, project) and sys.exc_info()[0] is None:
                raise AssertionError("The fixture's agent service did not clean up")
