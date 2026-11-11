#!/usr/bin/env python3
"""Native project environments: saved overrides, tasks and new/existing shells."""

import json
import os
import shutil
import subprocess
import time
from pathlib import Path

os.environ["RODE_UI_ARTIFACTS"] = str(
    Path(__file__).resolve().parent.parent / "target/environment-ui"
)
from ui_harness import (
    ARTIFACTS,
    BINARY,
    assert_running,
    click_button,
    close_window,
    fixture_directory,
    focus_window,
    key,
    palette_command,
    screenshot,
    text,
    wait_for_button,
    wait_for_project,
    wait_for_window,
)


def command(name):
    palette_command(window, process, log_path, state_path, name)


def click_control(name):
    click_button(window, process, log_path, state_path, name)


def replace_control(name, value):
    click_control(name)
    wait_for_button(window, process, log_path, state_path, name, focused=True)
    before = json.loads(state_path.read_text())["pass"]
    key("a", ("Control_L",))
    text(value)
    wait_for_button(window, process, log_path, state_path, name, changed_after=before)


with fixture_directory(prefix="rode-environment-ui-") as base:
    project = base / "project"
    project.mkdir()
    (project / "main.js").write_text("// fixture: the selected runtime records its invocation\n")
    runtime = project / "runtime-node"
    runtime.write_text(
        "#!/usr/bin/env python3\nimport json,os,sys\n"
        "from pathlib import Path\n"
        "if '--version' in sys.argv: print('fixture Node.js 1.0')\n"
        "else:\n"
        "    result = Path('task.json.tmp')\n"
        "    result.write_text(json.dumps(dict(mode=os.getenv('PROJECT_MODE'),argv=sys.argv[1:])))\n"
        "    result.replace('task.json')\n"
    )
    runtime.chmod(0o755)
    state_path = base / "ui-state.json"
    environment = dict(
        os.environ,
        XDG_CONFIG_HOME=str(base / "config"),
        XDG_DATA_HOME=str(base / "data"),
        RODE_UI_TEST_STATE=str(state_path),
    )
    log_path = ARTIFACTS / "environment.log"
    preferences = base / "data/rode/app.ron"
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

            def settings():
                if not preferences.exists():
                    return {}
                data = preferences.read_text().rstrip()
                data = json.loads(data[:-1].rstrip().removesuffix(",") + "}")
                projects = json.loads(data.get("project_preferences", "{}"))
                return next(
                    (
                        value.get("environment", {})
                        for scope, value in projects.items()
                        if str(project) in scope
                    ),
                    {},
                )

            def shell_record(filename):
                # Existence is the readiness signal, so publish only complete records.
                text(
                    f"printf '%s' \"$PROJECT_MODE\" > {filename}.tmp "
                    f"&& mv {filename}.tmp {filename}\n"
                )
                wait(lambda: (project / filename).exists(), "New terminal did not execute input")
                return (project / filename).read_text()

            def applied_status(applied):
                wait_for_button(
                    window,
                    process,
                    log_path,
                    base / "ui-state.json",
                    "environment-applied" if applied else "environment-unapplied",
                )

            window = wait_for_window(process, log_path)
            wait_for_project(window, process, log_path, project)
            focus_window(window)
            command("Open file by path")
            text("main.js")
            key("Return")
            time.sleep(0.4)
            command("Environment: Runtimes and project variables")
            screenshot(window, "environment.png")
            replace_control("environment-runtime-node", str(runtime))
            replace_control("environment-variables", "PROJECT_MODE=first")
            applied_status(False)
            click_control("environment-apply")
            wait(
                lambda: settings().get("variables", {}).get("PROJECT_MODE") == "first",
                "Environment settings were not saved",
            )
            applied_status(True)
            assert settings()["runtimes"]["node"] == str(runtime)
            command("Tasks: Run, build and test")
            screenshot(window, "tasks.png")
            click_control("task-run-current")
            wait(lambda: (project / "task.json").exists(), "Tasks did not use the selected runtime")
            result = json.loads((project / "task.json").read_text())
            assert result["mode"] == "first" and result["argv"] == [str(project / "main.js")], (
                result
            )
            assert shell_record("first.txt") == "first"
            command("Environment: Runtimes and project variables")
            replace_control("environment-variables", "PROJECT_MODE=second")
            applied_status(False)
            click_control("environment-apply")
            wait(
                lambda: settings().get("variables", {}).get("PROJECT_MODE") == "second",
                "Updated project variables were not saved",
            )
            applied_status(True)
            key("space", ("Control_L",))
            key("t")
            assert shell_record("existing.txt") == "first", (
                "Applying settings changed a running shell"
            )
            (project / "task.json").unlink()
            command("Tasks: Run, build and test")
            click_control("task-run-current")
            wait(lambda: (project / "task.json").exists(), "Updated task environment was not used")
            assert json.loads((project / "task.json").read_text())["mode"] == "second"
            assert shell_record("after-task.txt") == "first", "Task exports escaped their subshell"
            key("space", ("Control_L",))
            key("n")
            time.sleep(0.5)
            assert shell_record("second.txt") == "second"
            screenshot(window, "new-terminal.png")
            close_window(window, process)
            process = subprocess.Popen(
                [str(BINARY), str(project)], env=environment, stdout=log, stderr=log
            )
            window = wait_for_window(process, log_path)
            wait_for_project(window, process, log_path, project)
            focus_window(window)
            time.sleep(0.5)
            key("t", ("Control_L", "Shift_L"))
            time.sleep(0.6)
            assert shell_record("restored.txt") == "second", "Restart lost the project environment"
            close_window(window, process)
            print(
                "Environment overrides, task execution, new/existing shells and restart persistence passed."
            )
        except BaseException:
            for source, name in [
                (state_path, "failure-state.json"),
                (preferences, "failure-preferences.ron"),
            ]:
                if source.exists():
                    shutil.copy2(source, ARTIFACTS / name)
            if window and process.poll() is None:
                screenshot(window, "failure.png")
            raise
        finally:
            if process.poll() is None:
                process.terminate()
                process.wait(timeout=10)
