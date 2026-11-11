#!/usr/bin/env python3
"""Answer and cancel real chat question forms using a deterministic app-server peer."""

import json
import os
import shutil
import subprocess
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
    fixture_directory,
    focus_window,
    key,
    read_json_lines,
    screenshot,
    text,
    wait_for_button,
    wait_for_project,
    wait_for_window,
)

with fixture_directory(prefix="rode-agent-questions-ui-") as base:
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
        RODE_SESSION_DIR=str(base / "runtime"),
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
                return read_json_lines(project / "agent-rpc.jsonl")

            def enabled(name):
                state = json.loads(state_path.read_text())
                return next(
                    (button["enabled"] for button in state["buttons"] if button["name"] == name),
                    None,
                )

            def ask(message="question"):
                click(*control("agent-composer"))
                control("agent-composer", focused=True)
                before = json.loads(state_path.read_text())["pass"]
                text(message)
                control("agent-composer", changed_after=before)
                click_button(window, process, log_path, state_path, "agent-send")
                control("agent-question-0-answer")
                if message == "question":
                    control("agent-question-0-header")
                    control("agent-question-0-description-0")

            def fill_async():
                click_button(window, process, log_path, state_path, "agent-question-0-option-1")
                click(*control("agent-question-1-answer"))
                control("agent-question-1-answer", focused=True)
                before = json.loads(state_path.read_text())["pass"]
                text("Keep tool commands offline")
                control("agent-question-1-answer", changed_after=before)

            def async_answered():
                path = project / "async-answers.json"
                try:
                    content = json.loads(path.read_text())
                    return any(
                        "Disabled" in item.get("text", "")
                        and "Keep tool commands offline" in item.get("text", "")
                        for item in content
                    )
                except (FileNotFoundError, json.JSONDecodeError):
                    return False

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
            text("Focused tests")
            control("agent-question-0-answer", changed_after=before)
            before = json.loads(state_path.read_text())["pass"]
            key("Return", ("Shift_L",))
            text("please")
            control("agent-question-0-answer", changed_after=before)
            screenshot(window, "answer-form.png")
            key("Return")
            expected = {"answers": {"test_mode": {"answers": ["Focused tests\nplease"]}}}

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
            ask("async-question")
            control("agent-question-working")
            wait(
                lambda: (project / "async-work-continued").exists(),
                "The async fixture did not continue independent work",
            )
            fill_async()
            screenshot(window, "async-active.png")
            click_button(window, process, log_path, state_path, "agent-question-send")
            wait(async_answered, "The asynchronous answer did not reach the active turn")
            control("agent-idle")
            assert any(call.get("method") == "turn/steer" for call in calls()), (
                "An active answer did not steer the turn"
            )

            (project / "async-answers.json").unlink()
            ask("async-question-finished")
            control("agent-question-idle")
            click_button(window, process, log_path, state_path, "agent-question-0-option-1")
            control("agent-question-send")
            screenshot(window, "async-after-completion.png")
            before_turns = sum(call.get("method") == "turn/start" for call in calls())
            click_button(window, process, log_path, state_path, "agent-question-send")

            def partial_answered():
                try:
                    content = (project / "async-answers.json").read_text()
                    return "Disabled" in content and "Which setting should I use?" not in content
                except FileNotFoundError:
                    return False

            wait(partial_answered, "A selected option could not be sent on its own")
            click(*control("agent-question-1-answer"))
            control("agent-question-1-answer", focused=True)
            before = json.loads(state_path.read_text())["pass"]
            text("Keep tool commands offline")
            control("agent-question-1-answer", changed_after=before)
            key("Return")
            wait(
                lambda: (
                    "Keep tool commands offline" in (project / "async-answers.json").read_text()
                ),
                "The remaining question could not be answered with Enter",
            )
            control("agent-idle")
            assert sum(call.get("method") == "turn/start" for call in calls()) == before_turns + 2

            ask("async-question-finished")
            control("agent-question-idle")
            before_calls = len(calls())
            click_button(window, process, log_path, state_path, "agent-question-stop")
            control("agent-idle")
            assert not any(
                call.get("method") in ("turn/start", "turn/steer", "turn/interrupt")
                or isinstance(call.get("id"), dict)
                for call in calls()[before_calls:]
            ), "Dismissing an idle question sent work or a fabricated RPC response"
            screenshot(window, "cancelled.png")
            close_window(window, process)
            print(
                "Blocking and asynchronous question forms, active/idle answer delivery, dismissal and composer restoration passed."
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
