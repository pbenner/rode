#!/usr/bin/env python3
"""Real native Git/Ruff/venv/monitor/settings checks on an isolated X display."""

import json
import os
import re
import shlex
import shutil
import subprocess
import sys
import time
from pathlib import Path

from test_capabilities import skip

os.environ["RODE_UI_ARTIFACTS"] = str(
    Path(__file__).resolve().parent.parent / "target/workbench-ui"
)
from ui_harness import (
    ARTIFACTS,
    BINARY,
    NativeUI,
    close_window,
    decode_ron_string,
    fixture_directory,
    fixture_environment,
    focus_window,
    key,
    screenshot,
    text,
    wait_for_project,
    wait_for_window,
    write_python_launcher,
)


def wait_for(check, message, timeout=10):
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        if check():
            return
        time.sleep(0.06)
    raise AssertionError(message)


def run(remote=False, scenario="all"):
    label = ("ssh" if remote else "local") + "-" + scenario
    ruff = os.environ.get("RODE_TEST_RUFF") or shutil.which("ruff")
    if scenario in ("all", "tools"):
        assert ruff, "Set RODE_TEST_RUFF to a real Ruff executable"
    with fixture_directory(prefix="rode-workbench-ui-") as base:
        root = base / "project"
        root.mkdir()
        if scenario in ("all", "tools"):
            subprocess.run(
                [sys.executable, "-m", "venv", "--without-pip", str(root / ".venv")], check=True
            )
            shutil.copy2(ruff, root / ".venv/bin/ruff")
        source = root / "main.py"
        source.write_text(
            "import os\n\n\ndef greet():\n    value= [1,2,3]\n    print(value)\n\n\ngreet()\n"
        )
        (root / ".gitignore").write_text(".venv/\n*.ok\n.worktrees/\n")

        def git(*args):
            return subprocess.check_output(
                ["git", "-C", str(root), *args], text=True, stderr=subprocess.DEVNULL
            )

        git("init", "-b", "main")
        git("config", "user.name", "RODE UI Test")
        git("config", "user.email", "rode@example.invalid")
        git("add", ".")
        git("commit", "-m", "Initial project")
        if scenario == "git":
            source.write_text(source.read_text() + "\n# change to commit\n")
        state_path = ARTIFACTS / (label + "-state.json")
        environment = fixture_environment(
            base,
            XDG_CONFIG_HOME=str(base / "config"),
            XDG_DATA_HOME=str(base / "data-home"),
            RODE_UI_TEST_STATE=str(state_path),
        )
        if scenario in ("all", "tools"):
            # Exercise the default uvx route with a real, supplied Ruff, without
            # using the developer's home or fetching packages during a UI test.
            tools = base / "home/.local/bin"
            tools.mkdir(parents=True)
            write_python_launcher(
                tools / "uvx",
                "import os, sys\nassert sys.argv[1] == 'ruff', sys.argv\n"
                + f"os.execv({str(root / '.venv/bin/ruff')!r}, [{str(root / '.venv/bin/ruff')!r}, *sys.argv[2:]])\n",
            )
            environment["PATH"] = str(tools) + os.pathsep + environment["PATH"]
            # mise/uv Python may live outside /usr. Grant that exact runtime,
            # not the developer's home, so the private HOME also works in CI.
            host = ["rode-test", "", "", os.environ["RODE_TEST_SSH_CONFIG"], ""] if remote else None
            context = json.dumps([host, str(root)], separators=(",", ":"))
            settings = {
                context: {
                    "sandbox": {
                        "enabled": True,
                        "read_only": [str(Path(sys.base_prefix).resolve())],
                    }
                }
            }
            preferences = base / "data-home/rode/app.ron"
            preferences.parent.mkdir(parents=True)
            preferences.write_text(json.dumps({"project_preferences": json.dumps(settings)}))
        if remote:
            tools = base / "ssh-fixture"
            tools.mkdir()
            real_ssh = shutil.which("ssh")
            wrapper = tools / "ssh"
            wrapper.write_text(
                "#!/usr/bin/python3\nimport os,sys\nbinary="
                + repr(real_ssh)
                + "\nos.execv(binary,[binary]+['-T' if a=='-tt' else a for a in sys.argv[1:]])\n"
            )
            wrapper.chmod(0o755)
            environment["PATH"] = str(tools) + os.pathsep + os.environ["PATH"]
        args = [str(BINARY), "--sandbox", "--theme", "carbon"]
        if remote:
            args += [
                "--ssh",
                "rode-test",
                "--ssh-config",
                os.environ["RODE_TEST_SSH_CONFIG"],
                "--remote-path",
                str(root),
            ]
        else:
            args += [str(root)]
        log_path = ARTIFACTS / (label + ".log")
        with log_path.open("w") as log:
            process = subprocess.Popen(args, env=environment, stdout=log, stderr=log)
            window = None
            try:
                window = wait_for_window(process, log_path)
                wait_for_project(window, process, log_path, root)
                focus_window(window)
                ui = NativeUI(window, process, log_path, state_path, root)
                command = ui.command
                control = ui.click
                if scenario in ("all", "tools"):
                    ui.open_file("main.py")
                    ui.control("source-editor", focused=True)
                    key("End", ("Control_L",))
                    expected = source.read_text() + "\n# native workbench saved"
                    text("\n# native workbench saved")
                    ui.source(expected)
                    key("s", ("Control_L",))
                    wait_for(
                        lambda: "value = [1, 2, 3]" in source.read_text(),
                        "Default uvx Ruff did not format on save",
                    )
                    ui.control("source-editor-saved")
                    command("Code: Problems")
                    ui.control("workbench-panel:PROBLEMS")
                    screenshot(window, label + "-problems.png")
                    formatted = source.read_text()
                    assert formatted.startswith("import os\n\n\n")
                    fixed = formatted.removeprefix("import os\n\n\n")
                    command("Ruff: Apply safe fixes")
                    ui.click("workspace-code")
                    ui.click("source-editor")
                    ui.source(fixed)
                    key("s", ("Control_L",))
                    ui.control("source-editor-saved")
                    assert source.read_text() == fixed
                    command("Python: Select environment")
                    ui.control("workbench-panel:ENVIRONMENT")
                    screenshot(window, label + "-python.png")
                    # A command uses the actual selected virtual environment in the terminal.
                    command("Code: Run current file / project")
                    ui.terminal()
                    text('python -c \'import sys;open("venv.ok","w").write(sys.prefix)\'\n')
                    wait_for(
                        lambda: (
                            (root / "venv.ok").exists()
                            and (root / "venv.ok").read_text() == str(root / ".venv")
                        ),
                        "Project terminal did not execute Python",
                    )
                    assert (root / "venv.ok").read_text() == str(root / ".venv"), (
                        "Terminal uses the wrong Python environment"
                    )
                    command("System: Monitor")
                    ui.control("monitor-sample")
                    screenshot(window, label + "-monitor.png")
                if scenario in ("all", "git"):
                    command("Git: Source control")
                    ui.control("workbench-panel:SOURCE CONTROL")
                    screenshot(window, label + "-git.png")

                    control("git-stage-all")
                    wait_for(
                        lambda: bool(git("diff", "--cached", "--name-only").strip()),
                        "Native Stage all failed",
                    )
                    screenshot(window, label + "-git-staged.png")
                    control("git-commit-message")
                    text("Native project commit")
                    control("git-commit")
                    wait_for(
                        lambda: git("log", "-1", "--format=%s").strip() == "Native project commit",
                        "Native commit failed",
                    )
                if scenario in ("all", "settings"):
                    command("Project: Settings and sandbox access")
                    screenshot(window, label + "-settings.png")

                    def settings_click(name):
                        control(name)

                    settings_click("settings-format-on-save")
                    # Categories retain an edit until explicitly saved or discarded.
                    for category in (
                        "LaTeX",
                        "Presentations",
                        "Monitoring",
                        "Sandbox",
                        "Code tools",
                    ):
                        settings_click("settings-category-" + category)
                        screenshot(
                            window,
                            label + "-settings-" + category.replace(" ", "-").lower() + ".png",
                        )
                    # Navigation/actions remain reachable at the compact native window size.
                    ui.resize(850, 560)
                    for category in ("Presentations", "Sandbox", "Code tools"):
                        settings_click("settings-category-" + category)
                        screenshot(
                            window,
                            label
                            + "-settings-compact-"
                            + category.replace(" ", "-").lower()
                            + ".png",
                        )
                    settings_click("settings-discard")
                    ui.layout(
                        lambda state: any(
                            b["name"] == "settings-save" and not b["enabled"]
                            for b in state["buttons"]
                        ),
                        "Discard did not clear the pending code-tool edit",
                    )
                    settings_click("settings-format-on-save")
                    settings_click("settings-save")
                    ui.resize(1380, 900)
                    preferences = base / "data-home/rode/app.ron"
                    wait_for(
                        preferences.exists, "Project settings were not saved while the app was open"
                    )

                    def saved_settings():
                        match = re.search(
                            r'"project_preferences"\s*:\s*("(?:\\.|[^"\\])*")',
                            preferences.read_text(),
                        )
                        if not match:
                            return False
                        data = json.loads(decode_ron_string(match[1]))
                        return any(value.get("format_on_save") is False for value in data.values())

                    wait_for(saved_settings, "Changed UI settings were not flushed to the config")
                    close_window(window, process)
                    # Reopen with a saved per-project policy different from the
                    # last global defaults, exercising startup policy restoration.
                    match = re.search(
                        r'"project_preferences"\s*:\s*("(?:\\.|[^"\\])*")', preferences.read_text()
                    )
                    saved = json.loads(decode_ron_string(match[1]))
                    output = base / "shared-output"
                    output.mkdir()
                    for value in saved.values():
                        value["sandbox"].update(
                            enabled=True, project_read_only=True, read_write=[str(output)]
                        )
                    content = preferences.read_text()
                    content = (
                        content[: match.start(1)]
                        + json.dumps(json.dumps(saved))
                        + content[match.end(1) :]
                    )
                    preferences.write_text(content)
                    process = subprocess.Popen(args, env=environment, stdout=log, stderr=log)
                    window = None
                    window = wait_for_window(process, log_path)
                    wait_for_project(window, process, log_path, root)
                    focus_window(window)
                    ui = NativeUI(window, process, log_path, state_path, root)
                    state = subprocess.check_output(["xwininfo", "-id", hex(window)], text=True)
                    if "[Sandbox]" in state:
                        ui.terminal()
                        text(
                            "echo blocked > policy-blocked.ok; echo done > "
                            + shlex.quote(str(output / "done.ok"))
                            + "\n"
                        )
                        wait_for(
                            lambda: (output / "done.ok").exists(),
                            "Restored policy did not share the explicitly granted output folder",
                        )
                        assert not (root / "policy-blocked.ok").exists(), (
                            "Saved read-only project policy was not restored"
                        )
                    else:
                        skip(
                            "bwrap",
                            "Read-only policy enforcement unavailable in visible normal mode",
                            test=label,
                        )
                    screenshot(window, label + "-policy-restored.png")
                    close_window(window, process)
                else:
                    close_window(window, process)
                print(label + ": workbench assertions passed.")
            except Exception:
                if window is not None and process.poll() is None:
                    screenshot(window, label + "-failure.png")
                raise
            finally:
                if process.poll() is None:
                    process.terminate()
                    process.wait(timeout=5)


if __name__ == "__main__":
    case = os.environ.get("RODE_WORKBENCH_UI_CASE", "all")
    scenario = os.environ.get("RODE_WORKBENCH_UI_SCENARIO", "all")
    assert case in ("all", "local", "ssh"), f"Unknown workbench host case: {case}"
    assert scenario in ("all", "tools", "git", "settings"), (
        f"Unknown workbench scenario: {scenario}"
    )
    if case == "ssh":
        assert os.environ.get("RODE_TEST_SSH_CONFIG"), "SSH case requires scripts/test-ssh.sh"
    if case in ("all", "local"):
        run(scenario=scenario)
    if case in ("all", "ssh") and os.environ.get("RODE_TEST_SSH_CONFIG"):
        run(remote=True, scenario=scenario)
