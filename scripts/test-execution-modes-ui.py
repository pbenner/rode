#!/usr/bin/env python3
"""Real Git and PTY access checks for independent user execution modes."""

import json
import os
import re
import shlex
import shutil
import subprocess
import tempfile
import time
from pathlib import Path

from ssh_fixture import shell_probe

remote = os.environ.get("RODE_EXECUTION_UI_REMOTE") == "1"
expect_fallback = remote and os.environ.get("RODE_SSH_TEST_NO_BWRAP") == "1"
label = "ssh-fallback" if expect_fallback else "ssh" if remote else "local"
backend = os.environ.get("RODE_TEST_SANDBOX_BACKEND", "landlock")
os.environ["RODE_UI_ARTIFACTS"] = str(
    Path(__file__).resolve().parent.parent / "target/execution-modes-ui" / label
)
from ui_harness import (
    ARTIFACTS,
    BINARY,
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


with tempfile.TemporaryDirectory(prefix="rode-execution-ui-") as directory:
    base = Path(directory)
    root = base / "project"
    root.mkdir()
    secret = base / "outside-project"
    secret.write_text("host-access")
    shell = root / "shell"
    shell.write_text("#!/bin/sh\nexec /bin/bash --noprofile --norc\n")
    shell.chmod(0o755)
    probe = root / "probe.sh"
    probe.write_text(
        f"if cat {shlex.quote(str(secret))} >/dev/null 2>&1; then value=host; "
        'else value=blocked; fi\nprintf "%s" "$value" > run.tmp && mv run.tmp run.result\n'
    )
    (root / "check-access").write_text(
        f"if cat {shlex.quote(str(secret))} >/dev/null 2>&1; then v=host; else v=blocked; fi\n"
        'printf "%s:%s" "$v" "$$" > "$1.tmp" && mv "$1.tmp" "$1"\n'
    )
    config = base / "data/rode"
    config.mkdir(parents=True)
    (config / "app.ron").write_text(
        json.dumps({"sandbox_defaults": json.dumps({"enabled": True, "backend": backend})})
    )
    state_path = base / "ui-state.json"
    environment = dict(
        os.environ,
        XDG_CONFIG_HOME=str(base / "config"),
        XDG_DATA_HOME=str(base / "data"),
        RODE_SESSION_DIR=str(base / "sessions"),
        SHELL=str(shell),
        RODE_UI_TEST_STATE=str(state_path),
        GIT_CONFIG_NOSYSTEM="1",
        GIT_CONFIG_GLOBAL="/dev/null",
    )

    def git(*args, check=True):
        return subprocess.run(
            ["git", "-C", str(root), *args],
            env=environment,
            text=True,
            capture_output=True,
            check=check,
            timeout=20,
        ).stdout.strip()

    git("init", "-b", "main")
    git("config", "user.name", "Execution fixture")
    git("config", "user.email", "fixture@example.invalid")
    git("add", "--", "probe.sh")
    (root / ".git/info/exclude").write_text("*\n")
    hook = root / ".git/hooks/pre-commit"
    hook.write_text(f"#!/bin/sh\ncat {shlex.quote(str(secret))} >/dev/null\n")
    hook.chmod(0o755)
    args = [str(BINARY), "--sandbox", "--theme", "carbon"]
    if remote:
        # The fixture sshd cannot allocate the account's initial login TTY.
        # Native project shells still use real host PTYs through the service.
        tools = base / "tools"
        tools.mkdir()
        ssh = shutil.which("ssh")
        wrapper = tools / "ssh"
        wrapper.write_text(
            "#!/usr/bin/python3\nimport os, sys\n"
            f"binary = {ssh!r}\n"
            "os.execv(binary, [binary] + ['-T' if a == '-tt' else a for a in sys.argv[1:]])\n"
        )
        wrapper.chmod(0o755)
        environment["PATH"] = str(tools) + os.pathsep + environment["PATH"]
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
    log_path = ARTIFACTS / "execution.log"
    with log_path.open("w") as log:
        process = subprocess.Popen(args, env=environment, stdout=log, stderr=log)
        window = None
        try:
            window = wait_for_window(process, log_path)
            wait_for_project(window, process, log_path, root)
            focus_window(window)

            def wait(check, message):
                until = time.monotonic() + 20
                while time.monotonic() < until:
                    assert_running(process, log_path)
                    value = check()
                    if value:
                        return value
                    time.sleep(0.05)
                raise AssertionError(message)

            def control(name, focused=False):
                return wait_for_button(window, process, log_path, state_path, name, focused=focused)

            def click(name):
                click_button(window, process, log_path, state_path, name)

            def command(value):
                palette_command(window, process, log_path, state_path, value)

            def infos():
                # Both fixture hosts share this private directory. Observe the
                # real daemon; never create or manipulate sessions through RPC.
                reply = json.loads(
                    subprocess.check_output(
                        [str(BINARY), "--session-service", "request"],
                        input=b'{"action":"list"}',
                        env=environment,
                        timeout=20,
                    )
                )
                assert reply["kind"] == "sessions", reply
                return reply["data"]

            def created(old):
                items = wait(lambda: [i for i in infos() if i["id"] not in old], "No new shell")
                assert len(items) == 1, items
                info = items[0]
                control("terminal-input:" + info["id"], focused=True)
                return info

            def shell_check(info, name, expected):
                click("terminal-input:" + info["id"])
                control("terminal-input:" + info["id"], focused=True)
                text(shell_probe(name))
                result = wait(lambda: contents(root / name), f"Shell did not acknowledge {name}")
                mode, pid = result.split(":")
                assert mode == expected and int(pid) > 0, result
                return pid

            def replace(name, value):
                click(name)
                control(name, focused=True)
                before = json.loads(state_path.read_text())["pass"]
                key("a", ("Control_L",))
                text(value)
                wait_for_button(window, process, log_path, state_path, name, changed_after=before)

            if remote:
                # Remote startup intentionally foregrounds chat and hides the
                # explorer. Select the surfaces this execution-mode test uses.
                click("workspace-tab:Terminal")
                command("Toggle explorer")
            first = created(set())
            active = first["mode"].startswith("SANDBOX")
            if expect_fallback:
                assert first["mode"] == "NORMAL MODE · sandbox unavailable", first
            if not active:
                assert os.environ.get("RODE_TEST_REQUIRE_LANDLOCK") != "1", first
                assert os.environ.get("RODE_TEST_REQUIRE_SANDBOX") != "1", first
                print(
                    f"Project sandbox unavailable; enforcement checks skipped: {first['mode']}",
                    flush=True,
                )
            first_pid = shell_check(first, "first", "blocked" if active else "host")
            click("terminal-execution-mode")
            click("terminal-new-host")
            host = created({first["id"]})
            assert host["mode"] == "NORMAL MODE", host
            host_pid = shell_check(host, "host", "host")
            assert first_pid != host_pid
            assert any(i["id"] == first["id"] and i["alive"] and i["clients"] == 1 for i in infos())

            # Run must create a restricted shell instead of using the selected
            # unrestricted shell, and leave both existing shells alive.
            click("explorer-row:" + str(probe))
            control("source-editor", focused=True)
            command("Code: Run current file / project")
            wait(
                lambda: contents(root / "run.result") == ("blocked" if active else "host"),
                "Run escaped the project policy or did not finish",
            )
            if active:
                runner = created({first["id"], host["id"]})
                assert runner["policy"] == first["policy"] != host["policy"]
                assert len([i for i in infos() if i["alive"]]) == 3
                click("terminal-execution-mode")
                click("terminal-new-sandbox")
                sandboxed = created({i["id"] for i in [first, host, runner]})
                shell_check(sandboxed, "chosen-sandbox", "blocked")
                # Leave the remembered new-terminal mode outside the sandbox.
                click("terminal-execution-mode")
                click("terminal-new-host")
                host2 = created({i["id"] for i in [first, host, runner, sandboxed]})
                shell_check(host2, "chosen-host", "host")
            for info, expected, pid in [
                (first, "blocked" if active else "host", first_pid),
                (host, "host", host_pid),
            ]:
                command("Terminal: Sessions and Slurm allocations")
                click("session-attach:" + info["id"])
                wait_for_ui_state(
                    process,
                    log_path,
                    state_path,
                    lambda state: (
                        state.get("session_ids") is None
                        and any(
                            button["name"] == "terminal-input:" + info["id"]
                            and button["enabled"]
                            and button["focused"]
                            for button in state["buttons"]
                        )
                    ),
                    "Sessions did not close and focus the selected terminal " + info["id"],
                )
                assert shell_check(info, "retained-" + info["id"], expected) == pid
            screenshot(window, "terminal-modes.png")

            key("g", ("Control_L", "Shift_L"))
            replace("git-commit-message", "Default host Git")
            click("git-commit")
            wait(
                lambda: git("log", "-1", "--format=%s", check=False) == "Default host Git",
                "Host Git failed",
            )
            initial_head = git("rev-parse", "HEAD")
            probe.write_text(probe.read_text() + "# second commit\n")
            git("add", "--", "probe.sh")
            if active:
                click("git-execution-mode")
                click("git-use-sandbox")
                replace("git-commit-message", "Retained Git draft")
                click("git-commit")
                control("git-output-view")
                assert git("rev-parse", "HEAD") == initial_head
                assert git("diff", "--cached", "--name-only") == "probe.sh"
                screenshot(window, "git-sandbox-denied.png")
                click("git-output-close")
                click("git-execution-mode")
                click("git-use-host")
                click("git-commit")
                wait(
                    lambda: git("log", "-1", "--format=%s") == "Retained Git draft",
                    "Host Git retry lost the commit draft or retained sandbox restrictions",
                )
            screenshot(window, "git-host.png")
            close_window(window, process)
            stored = re.search(
                r'"project_preferences"\s*:\s*("(?:\\.|[^"\\])*")',
                (config / "app.ron").read_text(),
            )
            assert stored, "Project preferences were not saved"
            saved = json.loads(json.loads(stored[1]))
            preferences = next(v for k, v in saved.items() if str(root) in k)
            # Persistence records changed fields, not every effective default.
            # In fallback mode Git was never switched, so its default false may
            # be absent. The terminal choice changed from true and must be saved.
            assert preferences.get("terminal_sandbox", True) is False, preferences
            assert preferences.get("git_sandbox", False) is False, preferences
            assert preferences["sandbox"]["enabled"] is True
            print(f"Independent Git/terminal modes and project-command policy passed ({label}).")
        except Exception:
            if state_path.exists():
                shutil.copy2(state_path, ARTIFACTS / "failed-state.json")
            if window is not None and process.poll() is None:
                screenshot(window, "failure.png")
            raise
        finally:
            if process.poll() is None:
                process.terminate()
                process.wait(timeout=10)
