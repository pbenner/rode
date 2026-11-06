#!/usr/bin/env python3
"""Answer and cancel real chat question forms using a deterministic app-server peer."""

import json
import os
import shutil
import subprocess
import tempfile
import time
from pathlib import Path

os.environ["RODE_UI_ARTIFACTS"] = str(
    Path(__file__).resolve().parent.parent / "target/agent-questions-ui"
)
from ui_harness import (
    ARTIFACTS,
    BINARY,
    ROOT,
    assert_running,
    click,
    click_button,
    close_window,
    focus_window,
    key,
    screenshot,
    text,
    wait_for_button,
    wait_for_project,
    wait_for_window,
)

with tempfile.TemporaryDirectory(prefix="rode-agent-questions-ui-") as directory:
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
    state_path = base / "ui-state.json"
    environment = dict(
        os.environ,
        XDG_CONFIG_HOME=str(base / "config"),
        XDG_DATA_HOME=str(base / "data"),
        RODE_UI_TEST_STATE=str(state_path),
    )
    log_path = ARTIFACTS / "questions.log"
    with log_path.open("w") as log:
        process = subprocess.Popen(
            [str(BINARY), "--theme", "studio-neutral", str(project)],
            env=environment,
            stdout=log,
            stderr=log,
        )
        window = None
        try:
            window = wait_for_window(process, log_path)
            wait_for_project(window, process, log_path, project)
            focus_window(window)
            key("a", ("Control_L", "Shift_L"))

            def control(name, **guards):
                return wait_for_button(window, process, log_path, state_path, name, **guards)

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

            def enabled(name):
                state = json.loads(state_path.read_text())
                return next(
                    (button["enabled"] for button in state["buttons"] if button["name"] == name),
                    None,
                )

            def ask():
                click(*control("agent-composer"))
                control("agent-composer", focused=True)
                before = json.loads(state_path.read_text())["pass"]
                text("question")
                control("agent-composer", changed_after=before)
                click_button(window, process, log_path, state_path, "agent-send")
                control("agent-question-0-answer")
                control("agent-question-0-header")
                control("agent-question-0-description-0")

            ask()
            wait(lambda: enabled("agent-question-send") is False, "An empty answer could be sent")
            assert not (project / "answers.json").exists()
            click_button(window, process, log_path, state_path, "agent-question-0-option-0")
            control("agent-question-send")
            assert not (project / "answers.json").exists(), "Selecting an option submitted it"
            click(*control("agent-question-0-answer"))
            control("agent-question-0-answer", focused=True)
            before = json.loads(state_path.read_text())["pass"]
            key("a", ("Control_L",))
            text("Focused tests please")
            control("agent-question-0-answer", changed_after=before)
            screenshot(window, "answer-form.png")
            click_button(window, process, log_path, state_path, "agent-question-send")
            expected = {"answers": {"test_mode": {"answers": ["Focused tests please"]}}}

            def answered():
                path = project / "answers.json"
                try:
                    return json.loads(path.read_text()) == expected
                except (FileNotFoundError, json.JSONDecodeError):
                    return False

            wait(answered, "The edited answer did not reach the agent")
            control("agent-idle")
            ask()
            click_button(window, process, log_path, state_path, "agent-question-stop")
            control("agent-idle")
            wait(
                lambda: any(call.get("method") == "turn/interrupt" for call in calls()),
                "Stop did not interrupt the question",
            )
            assert any(
                call.get("id") == 401 and call.get("result") == {"answers": {}} for call in calls()
            ), "Stop did not cancel the question"
            screenshot(window, "cancelled.png")
            close_window(window, process)
            print(
                "Question options, explicit edited answers, composer restoration and cancellation passed."
            )
        except BaseException:
            if window and process.poll() is None:
                screenshot(window, "failure.png")
            if state_path.exists():
                shutil.copy2(state_path, ARTIFACTS / "failure-state.json")
            if (project / "agent-rpc.jsonl").exists():
                shutil.copy2(project / "agent-rpc.jsonl", ARTIFACTS / "failure-rpc.jsonl")
            raise
        finally:
            if process.poll() is None:
                process.terminate()
                process.wait(timeout=10)
