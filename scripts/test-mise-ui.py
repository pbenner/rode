#!/usr/bin/env python3
"""Native optional mise extension, host metadata, task execution and retained terminal state."""

import json
import os
import shutil
import subprocess
import tempfile
import time
from pathlib import Path

os.environ["RODE_UI_ARTIFACTS"] = str(Path(__file__).resolve().parent.parent / "target/mise-ui")
from ui_harness import (
    ARTIFACTS,
    BINARY,
    ROOT,
    assert_running,
    click_button,
    close_window,
    focus_window,
    key,
    palette_command,
    screenshot,
    text,
    wait_for_button,
    wait_for_project,
    wait_for_ui_state,
    wait_for_window,
)


def contents(path):
    try:
        return path.read_text()
    except FileNotFoundError:
        return None


with tempfile.TemporaryDirectory(prefix="rode-mise-ui-") as directory:
    base = Path(directory)
    project = base / "project"
    project.mkdir()
    program = project / "mise"
    shutil.copy2(ROOT / "tests/fixtures/mise.py", program)
    program.chmod(0o755)
    (project / "mise-bin").mkdir()
    (project / "mise-trusted").touch()
    (project / "mise.toml").write_text("[tasks.smoke]\nrun='printf smoke'\n")
    shell = base / "shell"
    shell.write_text("#!/bin/sh\nexec /bin/bash --noprofile --norc\n")
    shell.chmod(0o755)
    config = base / "data/rode"
    config.mkdir(parents=True)
    scope = json.dumps([None, str(project)], separators=(",", ":"))
    preferences = {
        "extension_preferences": json.dumps({"rode.mise": {"enabled": True}}),
        "project_preferences": json.dumps(
            {scope: {"environment": {"mise": True, "runtimes": {"mise": str(program)}}}}
        ),
    }
    (config / "app.ron").write_text(json.dumps(preferences))
    state_path = base / "ui-state.json"
    environment = dict(
        os.environ,
        XDG_CONFIG_HOME=str(base / "config"),
        XDG_DATA_HOME=str(base / "data"),
        RODE_SESSION_DIR=str(base / "runtime"),
        SHELL=str(shell),
        RODE_UI_TEST_STATE=str(state_path),
    )
    log_path = ARTIFACTS / "mise.log"
    with log_path.open("w") as log:
        process = subprocess.Popen(
            [str(BINARY), str(project)], env=environment, stdout=log, stderr=log
        )
        window = None
        try:

            def wait(check, message):
                deadline = time.monotonic() + 20
                while time.monotonic() < deadline:
                    assert_running(process, log_path)
                    if check():
                        return
                    time.sleep(0.05)
                raise AssertionError(message)

            def control(name, *, focused=False):
                return wait_for_button(window, process, log_path, state_path, name, focused=focused)

            def click_control(name):
                click_button(window, process, log_path, state_path, name)

            window = wait_for_window(process, log_path)
            wait_for_project(window, process, log_path, project)
            focus_window(window)
            key("t", ("Control_L", "Shift_L"))
            state = wait_for_ui_state(
                process,
                log_path,
                state_path,
                lambda state: any(
                    button["name"].startswith("terminal-input:")
                    and button["enabled"]
                    and button["focused"]
                    for button in state["buttons"]
                ),
                "Project terminal did not acquire input focus",
                timeout=20,
            )
            terminals = [
                button["name"]
                for button in state["buttons"]
                if button["name"].startswith("terminal-input:")
            ]
            assert len(terminals) == 1, terminals
            terminal = terminals[0]
            control(terminal, focused=True)
            text('printf "%s %s" "$RODE_MISE_VALUE" "$$" > before.tmp && mv before.tmp before\n')
            wait(lambda: (project / "before").exists(), "mise terminal did not accept input")
            before = (project / "before").read_text()
            value, pid = before.split()
            assert value == "from-mise" and int(pid) > 0, before
            palette_command(window, process, log_path, state_path, "Tasks: Run, build and test")
            control("Refresh mise tools and tasks")
            screenshot(window, "tasks-before-refresh.png")
            click_control("Refresh mise tools and tasks")
            # A logged request is not evidence that its reply reached the UI.
            control("mise-task:smoke")
            calls = [
                json.loads(line)
                for line in (project / "mise-calls.jsonl").read_text().splitlines(keepends=True)
                if line.endswith("\n")
            ]
            assert any(call["args"] == ["tasks", "ls", "--local", "--json"] for call in calls), (
                calls
            )
            screenshot(window, "tasks-discovered.png")
            click_control("mise-task:smoke")
            wait(
                lambda: contents(project / "mise-task-result") == "task passed",
                "Mise task did not run in the project terminal",
            )
            control(terminal, focused=True)
            # Keep this redundant tab click: it caught observation replaying
            # egui hit testing and surrendering the terminal's restored focus.
            click_control("workspace-tab:Terminal")
            control(terminal, focused=True)
            text('printf "%s %s" "$RODE_MISE_VALUE" "$$" > after.tmp && mv after.tmp after\n')
            wait(lambda: (project / "after").exists(), "Terminal input was not retained")
            assert before == (project / "after").read_text(), (
                "Task replaced the shell or environment"
            )
            screenshot(window, "terminal-retained.png")
            close_window(window, process)
            print("Optional mise tasks, terminal environment and retained shell passed.")
        except Exception:
            for path in [
                state_path,
                project / "mise-calls.jsonl",
                project / "before",
                project / "after",
                project / "mise-task-result",
            ]:
                try:
                    if path.exists():
                        shutil.copyfile(path, ARTIFACTS / ("failure-" + path.name))
                except OSError as error:
                    print(f"Could not retain {path.name}: {error}")
            if window is not None and process.poll() is None:
                try:
                    screenshot(window, "failure.png")
                except Exception as error:
                    print(f"Could not capture the failure window: {error}")
            raise
        finally:
            if process.poll() is None:
                process.terminate()
                process.wait(timeout=10)
