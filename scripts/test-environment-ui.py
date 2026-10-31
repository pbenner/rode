#!/usr/bin/env python3
"""Native project environments: saved overrides, tasks and new/existing shells."""

import json
import os
import subprocess
import tempfile
import time
from pathlib import Path

os.environ["RODE_UI_ARTIFACTS"] = str(
    Path(__file__).resolve().parent.parent / "target/environment-ui"
)
from ui_harness import (
    ARTIFACTS,
    BINARY,
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


with tempfile.TemporaryDirectory(prefix="rode-environment-ui-") as directory:
    base = Path(directory)
    project = base / "project"
    project.mkdir()
    (project / "main.js").write_text("// fixture: the selected runtime records its invocation\n")
    runtime = project / "runtime-node"
    runtime.write_text(
        "#!/usr/bin/env python3\nimport json,os,sys\n"
        "from pathlib import Path\n"
        "if '--version' in sys.argv: print('fixture Node.js 1.0')\n"
        "else: Path('task.json').write_text(json.dumps(dict(mode=os.getenv('PROJECT_MODE'),argv=sys.argv[1:])))\n"
    )
    runtime.chmod(0o755)
    environment = dict(
        os.environ, XDG_CONFIG_HOME=str(base / "config"), XDG_DATA_HOME=str(base / "data")
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
                text(f"printf '%s' \"$PROJECT_MODE\" > {filename}\n")
                wait(lambda: (project / filename).exists(), "New terminal did not execute input")
                return (project / filename).read_text()

            window = wait_for_window(process, log_path)
            focus_window(window)
            command("Open file by path")
            text("main.js")
            key("Return")
            time.sleep(0.4)
            command("Environment: Runtimes and project variables")
            screenshot(window, "environment.png")
            click(480, 300)  # Node.js executable override.
            key("a", ("Control_L",))
            text(str(runtime))
            click(700, 620)  # Literal project variables.
            text("PROJECT_MODE=first")
            click(390, 696)  # Apply environment.
            wait(
                lambda: settings().get("variables", {}).get("PROJECT_MODE") == "first",
                "Environment settings were not saved",
            )
            assert settings()["runtimes"]["node"] == str(runtime)
            command("Tasks: Run, build and test")
            screenshot(window, "tasks.png")
            click(430, 185)  # Run the active JavaScript file with the selected runtime.
            wait(lambda: (project / "task.json").exists(), "Tasks did not use the selected runtime")
            result = json.loads((project / "task.json").read_text())
            assert result["mode"] == "first" and result["argv"] == [str(project / "main.js")], (
                result
            )
            assert shell_record("first.txt") == "first"
            command("Environment: Runtimes and project variables")
            click(700, 620)
            key("a", ("Control_L",))
            text("PROJECT_MODE=second")
            click(390, 696)
            wait(
                lambda: settings().get("variables", {}).get("PROJECT_MODE") == "second",
                "Updated project variables were not saved",
            )
            key("space", ("Control_L",))
            key("t")
            assert shell_record("existing.txt") == "first", (
                "Applying settings changed a running shell"
            )
            (project / "task.json").unlink()
            command("Tasks: Run, build and test")
            click(430, 185)
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
            if window and process.poll() is None:
                screenshot(window, "failure.png")
            raise
        finally:
            if process.poll() is None:
                process.terminate()
                process.wait(timeout=10)
