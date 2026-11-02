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
    click,
    close_window,
    focus_window,
    key,
    screenshot,
    text,
    wait_for_window,
)


def command(name):
    key("p", ("Control_L", "Shift_L"))
    text(name)
    key("Return")
    time.sleep(0.3)


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
    environment = dict(
        os.environ,
        XDG_CONFIG_HOME=str(base / "config"),
        XDG_DATA_HOME=str(base / "data"),
        RODE_SESSION_DIR=str(base / "runtime"),
        SHELL=str(shell),
    )
    log_path = ARTIFACTS / "mise.log"
    with log_path.open("w") as log:
        process = subprocess.Popen(
            [str(BINARY), str(project)], env=environment, stdout=log, stderr=log
        )
        try:

            def wait(check, message):
                deadline = time.monotonic() + 20
                while time.monotonic() < deadline:
                    assert_running(process, log_path)
                    if check():
                        return
                    time.sleep(0.05)
                raise AssertionError(message)

            window = wait_for_window(process, log_path)
            focus_window(window)
            key("t", ("Control_L", "Shift_L"))
            time.sleep(0.8)
            text('printf "%s %s" "$RODE_MISE_VALUE" "$$" > before.tmp && mv before.tmp before\n')
            wait(lambda: (project / "before").exists(), "mise terminal did not accept input")
            assert (project / "before").read_text().startswith("from-mise ")
            command("Tasks")
            screenshot(window, "tasks-before-refresh.png")
            click(450, 260)  # Refresh tools/tasks in the mise card.
            wait(
                lambda: any(
                    call["args"] == ["tasks", "ls", "--local", "--json"]
                    for call in [
                        json.loads(line)
                        for line in (project / "mise-calls.jsonl").read_text().splitlines()
                    ]
                ),
                "Mise metadata was not requested",
            )
            time.sleep(0.5)
            screenshot(window, "tasks-discovered.png")
            click(395, 350)  # The contributed smoke task below discovered metadata.
            wait(
                lambda: (project / "mise-task-result").exists(),
                "Mise task did not run in the project terminal",
            )
            click(400, 69)  # Task execution already selected Terminal; keep it selected.
            time.sleep(0.3)
            text('printf "%s %s" "$RODE_MISE_VALUE" "$$" > after.tmp && mv after.tmp after\n')
            wait(lambda: (project / "after").exists(), "Terminal input was not retained")
            assert (project / "before").read_text() == (project / "after").read_text()
            screenshot(window, "terminal-retained.png")
            close_window(window, process)
            print("Optional mise tasks, terminal environment and retained shell passed.")
        finally:
            if process.poll() is None:
                process.terminate()
                process.wait(timeout=10)
