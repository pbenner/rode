#!/usr/bin/env python3
"""Real native Git/Ruff/venv/monitor/settings checks on an isolated X display."""

import ctypes
import json
import os
import re
import shutil
import subprocess
import time
from pathlib import Path

os.environ["RODE_UI_ARTIFACTS"] = str(
    Path(__file__).resolve().parent.parent / "target/workbench-ui"
)
from ui_harness import (
    ARTIFACTS,
    BINARY,
    click,
    click_button,
    close_window,
    decode_ron_string,
    display,
    fixture_directory,
    focus_window,
    key,
    screenshot,
    text,
    wait_for_ui_state,
    x11,
)

x11.XResizeWindow.argtypes = [ctypes.c_void_p, ctypes.c_ulong, ctypes.c_uint, ctypes.c_uint]


def wait_for(check, message, timeout=10):
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        if check():
            return
        time.sleep(0.06)
    raise AssertionError(message)


def command(name):
    key("p", ("Control_L", "Shift_L"))
    text(name)
    key("Return")
    time.sleep(0.35)


def run(remote=False):
    label = "ssh" if remote else "local"
    ruff = os.environ.get("RODE_TEST_RUFF") or shutil.which("ruff")
    assert ruff, "Set RODE_TEST_RUFF to a real Ruff executable"
    with fixture_directory(prefix="rode-workbench-ui-") as base:
        root = base / "project"
        root.mkdir()
        subprocess.run(["python3", "-m", "venv", "--without-pip", str(root / ".venv")], check=True)
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
        state_path = base / "ui-state.json"
        environment = dict(
            os.environ,
            XDG_CONFIG_HOME=str(base / "config"),
            XDG_DATA_HOME=str(base / "data-home"),
            RODE_UI_TEST_STATE=str(state_path),
        )
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

                def control(name):
                    click_button(window, process, log_path, state_path, name)

                def mapped():
                    nonlocal window
                    assert process.poll() is None, "RODE exited unexpectedly"
                    tree = subprocess.check_output(["xwininfo", "-root", "-tree"], text=True)
                    found = re.search(r'(0x[0-9a-f]+) "RODE', tree)
                    if found:
                        window = int(found[1], 16)
                    return found

                wait_for(mapped, "No native window")
                time.sleep(1.0)
                focus_window(window)
                command("Open file by path")
                text("main.py")
                key("Return")
                time.sleep(0.6)
                click(550, 220)
                key("End", ("Control_L",))
                text("\n# native workbench saved")
                key("s", ("Control_L",))
                wait_for(
                    lambda: "value = [1, 2, 3]" in source.read_text(),
                    "Default uvx Ruff did not format on save",
                )
                command("Code: Problems")
                time.sleep(1.0)
                screenshot(window, label + "-problems.png")
                command("Ruff: Apply safe fixes")
                last_save = 0.0

                def fixes_saved():
                    nonlocal last_save
                    if "import os" not in source.read_text():
                        return True
                    # A remote fix can arrive after an earlier Save captured the old
                    # buffer. The palette targets the document regardless of panel focus.
                    if time.monotonic() - last_save >= 1.0:
                        command("Save file")
                        last_save = time.monotonic()
                    return False

                wait_for(
                    fixes_saved,
                    "Ruff safe fixes were not applied to the buffer",
                    # Formatting and checking each have their own bounded host job;
                    # a cold uvx cache or slow SSH channel can exceed ten seconds.
                    timeout=40,
                )
                command("Python: Select environment")
                screenshot(window, label + "-python.png")
                # A command uses the actual selected virtual environment in the terminal.
                command("Code: Run current file / project")
                time.sleep(0.4)
                click(500, 750)
                text('python -c \'import sys;open("venv.ok","w").write(sys.prefix)\'\n')
                wait_for(
                    lambda: (root / "venv.ok").exists(), "Project terminal did not execute Python"
                )
                assert (root / "venv.ok").read_text() == str(root / ".venv"), (
                    "Terminal uses the wrong Python environment"
                )
                command("System: Monitor")
                time.sleep(3.0)
                screenshot(window, label + "-monitor.png")
                command("Git: Source control")
                time.sleep(0.5)
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
                command("Project: Settings and sandbox access")
                screenshot(window, label + "-settings.png")

                def settings_click(name):
                    control(name)

                settings_click("settings-format-on-save")
                # Categories retain an edit until explicitly saved or discarded.
                for category in ("LaTeX", "Presentations", "Monitoring", "Sandbox", "Code tools"):
                    settings_click("settings-category-" + category)
                    screenshot(
                        window, label + "-settings-" + category.replace(" ", "-").lower() + ".png"
                    )
                # Navigation/actions remain reachable at the compact native window size.
                x11.XResizeWindow(display, window, 850, 560)
                x11.XFlush(display)
                for category in ("Presentations", "Sandbox", "Code tools"):
                    settings_click("settings-category-" + category)
                    screenshot(
                        window,
                        label + "-settings-compact-" + category.replace(" ", "-").lower() + ".png",
                    )
                settings_click("settings-discard")
                wait_for_ui_state(
                    process,
                    log_path,
                    state_path,
                    lambda state: any(
                        b["name"] == "settings-save" and not b["enabled"] for b in state["buttons"]
                    ),
                    "Discard did not clear the pending code-tool edit",
                )
                settings_click("settings-format-on-save")
                settings_click("settings-save")
                x11.XResizeWindow(display, window, 1380, 900)
                x11.XFlush(display)
                preferences = base / "data-home/rode/app.ron"
                wait_for(
                    preferences.exists, "Project settings were not saved while the app was open"
                )

                def saved_settings():
                    match = re.search(
                        r'"project_preferences"\s*:\s*("(?:\\.|[^"\\])*")', preferences.read_text()
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
                wait_for(mapped, "No native window on restart")
                time.sleep(1.0)
                focus_window(window)
                state = subprocess.check_output(["xwininfo", "-id", hex(window)], text=True)
                if "[Sandbox]" in state:
                    click(500, 750)
                    text(
                        "echo blocked > policy-blocked.ok; echo done > '"
                        + str(output / "done.ok")
                        + "'\n"
                    )
                    wait_for(
                        lambda: (output / "done.ok").exists(),
                        "Restored policy did not share the explicitly granted output folder",
                    )
                    assert not (root / "policy-blocked.ok").exists(), (
                        "Saved read-only project policy was not restored"
                    )
                else:
                    print(
                        label
                        + ": host cannot start bwrap; read-only enforcement check skipped in visible normal mode."
                    )
                screenshot(window, label + "-policy-restored.png")
                close_window(window, process)
                print(
                    label
                    + ": native Ruff formatting/fixes, venv terminal, monitor, Git staging/commit, and saved project settings passed."
                )
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
    if case in ("all", "local"):
        run()
    if case in ("all", "ssh") and os.environ.get("RODE_TEST_SSH_CONFIG"):
        run(remote=True)
