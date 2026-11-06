#!/usr/bin/env python3
"""Native Git identity recovery, real commits and history filtering/actions."""

import json
import os
import subprocess
import tempfile
import time
from pathlib import Path

remote = os.environ.get("RODE_GIT_IDENTITY_UI_REMOTE") == "1"
label = "ssh" if remote else "local"
os.environ["RODE_UI_ARTIFACTS"] = str(
    Path(__file__).resolve().parent.parent / "target/git-identity-ui" / label
)
from ui_harness import (
    ARTIFACTS,
    BINARY,
    assert_running,
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

with tempfile.TemporaryDirectory(prefix="rode-git-identity-ui-") as directory:
    base = Path(directory)
    root = base / "project"
    root.mkdir()
    home = base / "home"
    home.mkdir()
    global_config = (Path(os.environ["RODE_TEST_SSH_HOME"]) if remote else home) / ".gitconfig"
    environment = dict(
        os.environ,
        HOME=str(home),
        XDG_CONFIG_HOME=str(base / "config"),
        XDG_DATA_HOME=str(base / "data"),
        GIT_CONFIG_NOSYSTEM="1",
        GIT_CONFIG_GLOBAL=str(global_config),
        RODE_UI_TEST_STATE=str(base / "ui-state.json"),
    )
    for name in (
        "GIT_AUTHOR_NAME",
        "GIT_AUTHOR_EMAIL",
        "GIT_COMMITTER_NAME",
        "GIT_COMMITTER_EMAIL",
        "EMAIL",
    ):
        environment.pop(name, None)

    def git(*args, checked=True):
        result = subprocess.run(
            ["git", "-C", str(root), *args],
            env=environment,
            capture_output=True,
            text=True,
            check=checked,
        )
        return result.stdout.strip()

    git("init", "-b", "main")
    git("config", "--local", "user.useConfigOnly", "true")
    (root / "source.txt").write_text("staged source\n")
    git("add", "--", "source.txt")
    args = [str(BINARY), "--theme", "studio-neutral"]
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
        args.append(str(root))
    log_path = ARTIFACTS / "identity.log"
    state_path = base / "ui-state.json"
    with log_path.open("w") as log:
        process = subprocess.Popen(args, env=environment, stdout=log, stderr=log)
        window = None
        try:
            window = wait_for_window(process, log_path)
            wait_for_project(window, process, log_path, "project")
            focus_window(window)
            key("g", ("Control_L", "Shift_L"))

            def control(name):
                click_button(window, process, log_path, state_path, name)

            def wait(check, message):
                until = time.monotonic() + 20
                while time.monotonic() < until:
                    assert_running(process, log_path)
                    if check():
                        return
                    time.sleep(0.05)
                raise AssertionError(message)

            def replace(name, value):
                control(name)
                wait_for_button(window, process, log_path, state_path, name, focused=True)
                before = json.loads(state_path.read_text())["pass"]
                key("a", ("Control_L",))
                if value:
                    text(value)
                else:
                    key("BackSpace")
                wait_for_button(window, process, log_path, state_path, name, changed_after=before)

            control("git-tab-history")
            wait_for_button(window, process, log_path, state_path, "git-history-filter")
            screenshot(window, "empty-history.png")
            control("git-tab-changes")
            replace("git-commit-message", "Identity configured through UI")
            control("git-commit")
            # A failed commit must retain its draft and index while offering Settings.
            control("git-tab-history")
            control("git-configure-identity")
            replace("git-identity-name", "Repository Fixture")
            replace("git-identity-email", "repository@example.invalid")
            control("git-identity-save")
            wait(
                lambda: (
                    git("config", "--local", "--get", "user.email", checked=False)
                    == "repository@example.invalid"
                ),
                "Repository identity was not saved",
            )
            assert git("config", "--local", "--get", "user.name") == "Repository Fixture"
            assert git("diff", "--cached", "--name-only") == "source.txt"
            control("git-tab-changes")
            control("git-commit")
            wait(
                lambda: (
                    git("log", "-1", "--format=%s", checked=False)
                    == "Identity configured through UI"
                ),
                "Commit draft was lost or the configured identity was not used",
            )
            assert (
                git("log", "-1", "--format=%an <%ae>")
                == "Repository Fixture <repository@example.invalid>"
            )

            control("git-tab-settings")
            control("git-identity-global")
            replace("git-identity-name", "Global Fixture")
            replace("git-identity-email", "global@example.invalid")
            control("git-identity-save")
            wait(
                lambda: (
                    git("config", "--global", "--get", "user.email", checked=False)
                    == "global@example.invalid"
                ),
                "Global identity was not saved on the project host",
            )
            assert git("config", "--get", "user.email") == "repository@example.invalid"
            control("git-identity-repository")
            control("git-identity-inherit")
            wait(
                lambda: git("config", "--get", "user.email") == "global@example.invalid",
                "Removing repository identity did not restore inheritance",
            )
            wait_for_button(window, process, log_path, state_path, "git-identity-save")
            screenshot(window, "settings.png")
            git(
                "commit",
                "--allow-empty",
                "-m",
                "Improve history layout with a deliberately long subject that should truncate cleanly in a narrow pane",
            )
            git("commit", "--allow-empty", "-m", "Target commit for history filter")
            target = git("rev-parse", "HEAD")
            git("commit", "--allow-empty", "-m", "Latest history commit")
            control("git-tab-history")
            replace("git-history-filter", "Target commit")

            def filtered_target():
                state = json.loads(state_path.read_text())
                names = [
                    button["name"]
                    for button in state["buttons"]
                    if button["name"].startswith("git-history-details-")
                ]
                return names == ["git-history-details-" + target]

            wait(filtered_target, "History filter lost or displayed the wrong commit")
            control("git-history-more-" + target)
            control("git-history-copy-hash")
            wait(
                lambda: (
                    subprocess.run(
                        ["xclip", "-selection", "clipboard", "-o"],
                        capture_output=True,
                        text=True,
                        timeout=3,
                    ).stdout
                    == target
                ),
                "History copied the wrong commit hash",
            )
            replace("git-history-filter", "")
            screenshot(window, "history.png")
            control("git-history-details-" + target)
            wait_for_button(window, process, log_path, state_path, "git-output-view")
            screenshot(window, "history-details.png")
            close_window(window, process)
        except BaseException:
            if window is not None and process.poll() is None:
                screenshot(window, "failure.png")
            if state_path.exists():
                (ARTIFACTS / "failure-state.json").write_bytes(state_path.read_bytes())
            (ARTIFACTS / "failure-config.txt").write_text(
                git("config", "--local", "--list", checked=False)
            )
            raise
        finally:
            if process.poll() is None:
                process.terminate()
                try:
                    process.wait(timeout=5)
                except subprocess.TimeoutExpired:
                    process.kill()
                    process.wait()

print(
    f"Git ({label}): identity recovery, real commit, scope/inheritance and history filtering/copy/details passed."
)
