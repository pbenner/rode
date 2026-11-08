#!/usr/bin/env python3
"""Native project/sandbox UI checks; optional isolated SSH daemon from test-ssh.sh."""

import ctypes
import json
import os
import shutil
import signal
import socket
import struct
import subprocess
import tempfile
import time
from pathlib import Path

os.environ["RODE_UI_ARTIFACTS"] = str(Path(__file__).resolve().parent.parent / "target/project-ui")
from ui_harness import (
    ARTIFACTS,
    BINARY,
    assert_running,
    click_button,
    close_window,
    display,
    focus_window,
    key,
    palette_command,
    screenshot,
    text,
    wait_for_button,
    wait_for_project,
    wait_for_ui_state,
    wait_for_window,
    x11,
)

x11.XResizeWindow.argtypes = [ctypes.c_void_p, ctypes.c_ulong, ctypes.c_uint, ctypes.c_uint]


def window_for(process, log_path):
    window = wait_for_window(process, log_path)
    focus_window(window)
    return window


class ProjectUI:
    """Drive the real controls, with acknowledgements before dependent input."""

    def __init__(self, window, process, log_path, environment):
        self.window = window
        self.process = process
        self.log_path = log_path
        self.environment = environment
        self.state_path = Path(environment["RODE_UI_TEST_STATE"])

    def wait(self, check, message):
        deadline = time.monotonic() + 8
        while time.monotonic() < deadline:
            assert_running(self.process, self.log_path)
            result = check()
            if result:
                return result
            time.sleep(0.05)
        raise AssertionError(message + ". Log: " + str(self.log_path))

    def layout(self, check=lambda state: True, message="No rendered layout"):
        return wait_for_ui_state(self.process, self.log_path, self.state_path, check, message)

    def control(self, name, **guards):
        return wait_for_button(
            self.window, self.process, self.log_path, self.state_path, name, **guards
        )

    def click(self, name):
        click_button(self.window, self.process, self.log_path, self.state_path, name)

    def absent(self, name):
        self.layout(
            lambda state: all(button["name"] != name for button in state["buttons"]),
            name + " did not close",
        )

    def command(self, value):
        palette_command(self.window, self.process, self.log_path, self.state_path, value)

    def title(self):
        return subprocess.check_output(
            ["xwininfo", "-id", hex(self.window)],
            text=True,
            env=dict(os.environ, LC_ALL="C.UTF-8"),
            timeout=3,
        )

    def project(self, project, remote=False):
        wait_for_project(self.window, self.process, self.log_path, project)
        assert ("[SSH" in self.title()) == remote, self.title()

    def clipboard(self):
        result = subprocess.run(
            ["xclip", "-selection", "clipboard", "-o"],
            capture_output=True,
            text=True,
            timeout=2,
        )
        return result.stdout if result.returncode == 0 else None

    def copy(self, name, expected):
        self.control(name, focused=True)
        before = self.layout()["pass"]
        key("a", ("Control_L",))
        self.control(name, focused=True, after_pass=before)
        key("c", ("Control_L",))
        self.wait(
            lambda: self.clipboard() == expected,
            f"{name} did not contain {expected!r}",
        )
        before = self.layout()["pass"]
        key("End", ("Control_L",))
        self.control(name, focused=True, after_pass=before)

    def path(self, name, value):
        self.click(name)
        self.control(name, focused=True)
        before = self.layout()["pass"]
        key("a", ("Control_L",))
        text(value)
        self.control(name, focused=True, changed_after=before)
        self.copy(name, value)

    def editor(self):
        state = self.layout(
            lambda state: any(
                button["name"] in ("source-editor", "workspace-code") and button["enabled"]
                for button in state["buttons"]
            ),
            "Neither the source editor nor its workspace tab became available",
        )
        if not any(
            button["name"] == "source-editor" and button["enabled"] for button in state["buttons"]
        ):
            self.click("workspace-code")
        self.click("source-editor")
        self.control("source-editor", focused=True)

    def append(self, contents, marker):
        self.editor()
        self.copy("source-editor", contents)
        before = self.layout()["pass"]
        text("\n// " + marker)
        self.control("source-editor", focused=True, changed_after=before)
        self.copy("source-editor", contents + "\n// " + marker)

    def saved(self, path, contents):
        self.wait(
            lambda: path.exists() and path.read_text() == contents,
            "File did not receive the complete expected contents: " + str(path),
        )

    def saved_edit(self, path, marker):
        # Rust format-on-save may change whitespace. Verify the complete buffer
        # before Save, then wait for the edit to reach the actual project file.
        self.wait(
            lambda: path.exists() and marker in path.read_text(),
            "The edit was not saved: " + str(path),
        )
        self.control("source-editor-saved")

    def edit_file(self, path, marker):
        self.command("Open file by path")
        self.control("file-dialog-path", focused=True)
        self.path("file-dialog-path", path.name)
        key("Return")
        self.absent("file-dialog-path")
        contents = path.read_text()
        self.append(contents, marker)
        key("s", ("Control_L",))
        self.saved_edit(path, marker)

    def picker(self, remote=False):
        key("o", ("Control_L", "Shift_L"))
        self.control("project-dialog-path", focused=True)
        self.control("project-dialog-host:" + ("ssh" if remote else "local"))
        # Let the initial listing finish before editing its path; its response
        # otherwise competes with the typed destination.
        self.control("project-dialog-ready")

    def open_project(self, project, remote=False, screenshot_name=None):
        self.picker(remote)
        self.path("project-dialog-path", str(project))
        self.click("project-dialog-browse")
        self.control("project-dialog-ready")
        self.click("project-dialog-path")
        self.copy("project-dialog-path", str(project))
        if screenshot_name:
            screenshot(self.window, screenshot_name)
        self.click("project-dialog-open")
        self.absent("project-dialog-path")
        self.project(project, remote)

    def resize(self, width, height):
        before = self.layout()["pass"]
        x11.XResizeWindow(display, self.window, width, height)
        x11.XFlush(display)
        self.layout(
            lambda state: (
                state["pass"] > before
                and all(
                    abs(logical * state["pixels_per_point"] - actual) <= 2
                    for logical, actual in zip(state["size"], (width, height))
                )
            ),
            f"Window did not render at {width}x{height}",
        )

    def terminal(self, project):
        """Check the displayed session, its project, live attachment and input focus."""
        state = self.layout(
            lambda state: any(
                button["name"].startswith("terminal-input:") and button["enabled"]
                for button in state["buttons"]
            ),
            "The selected project's terminal did not become ready",
        )
        name = next(
            button["name"]
            for button in state["buttons"]
            if button["name"].startswith("terminal-input:") and button["enabled"]
        )
        identity = name.removeprefix("terminal-input:")
        info = self.wait(
            lambda: next(
                (
                    info
                    for info in session_rpc(self.environment, "list")
                    if info["id"] == identity
                    and info["root"] == str(project)
                    and info["alive"]
                    and info["clients"] > 0
                ),
                None,
            ),
            "The displayed terminal is not attached to " + str(project),
        )
        self.click(name)
        self.control(name, focused=True)
        return info

    def disconnected(self):
        self.wait(lambda: '"RODE"' in self.title(), "Disconnect did not clear the host")

    def failure(self, label):
        if self.state_path.exists():
            shutil.copy2(self.state_path, ARTIFACTS / (label + "-ui-state.json"))
        if self.process.poll() is None:
            screenshot(self.window, label + "-failure.png")


def session_rpc(environment, action, **values):
    reply = json.loads(
        subprocess.check_output(
            [str(BINARY), "--session-service", "request"],
            input=json.dumps(dict(action=action, **values)).encode(),
            env=environment,
            timeout=20,
        )
    )
    assert reply["kind"] != "error", reply
    return reply.get("data")


def run(remote=False, fallback=False):
    label = "ssh" if remote else "fallback" if fallback else "sandbox"
    with tempfile.TemporaryDirectory(prefix="rode-project-ui-") as directory:
        base = Path(directory)
        first, second, local = (
            base / name for name in ("first-project", "second-project", "local-project")
        )
        for project, filename in [(first, "alpha.rs"), (second, "beta.rs"), (local, "local.rs")]:
            project.mkdir()
            (project / "src").mkdir()
            (project / filename).write_text(
                f'// {filename}\nfn main() {{\n    println!("remote host");\n}}\n' + "\n" * 5
            )
        environment = dict(
            os.environ,
            XDG_CONFIG_HOME=str(base / "config"),
            XDG_DATA_HOME=str(base / "data-home"),
            RODE_SESSION_DIR=str(base / "sessions"),
            RODE_UI_TEST_STATE=str(base / "ui-state.json"),
        )
        landlock = os.environ.get("RODE_TEST_SANDBOX_BACKEND") == "landlock" and not fallback
        if landlock:
            preferences = base / "data-home/rode"
            preferences.mkdir(parents=True)
            (preferences / "app.ron").write_text(
                json.dumps(
                    {"sandbox_defaults": json.dumps({"enabled": True, "backend": "landlock"})}
                )
            )
        if fallback:
            tools = base / "no-bwrap"
            tools.mkdir()
            # Missing system bwrap now selects the bundled implementation. Make
            # an installed backend fail to exercise visible normal-mode fallback.
            backend = tools / "bwrap"
            backend.write_text(
                "#!/bin/sh\nprintf '%s\\n' 'Sandbox unavailable in project UI fixture' >&2\nexit 1\n"
            )
            backend.chmod(0o700)
            environment["PATH"] = str(tools)
        if remote:
            # An unprivileged fixture sshd cannot create/login a remote TTY.
            # Keep the real local PTY and real SSH master/channels, requesting
            # only a non-TTY remote shell for this fixture (never in the app).
            real_ssh = shutil.which("ssh")
            tools = base / "ssh-fixture-tools"
            tools.mkdir()
            wrapper = tools / "ssh"
            wrapper.write_text(
                "#!/usr/bin/python3\nimport os, sys\n"
                + "binary = "
                + repr(real_ssh)
                + "\n"
                + "os.execv(binary, [binary] + ['-T' if arg == '-tt' else arg for arg in sys.argv[1:]])\n"
            )
            wrapper.chmod(0o755)
            environment["PATH"] = str(tools) + os.pathsep + os.environ["PATH"]
        args = [str(BINARY), "--sandbox"]
        launch_copy = None
        if not remote and not fallback:
            launch_copy = base / "rode-running"
            shutil.copy2(BINARY, launch_copy)
            args[0] = str(launch_copy)
        if remote:
            args += [
                "--ssh",
                "rode-test",
                "--ssh-config",
                os.environ["RODE_TEST_SSH_CONFIG"],
                "--remote-path",
                str(first),
            ]
        else:
            args += [str(first)]
        with (ARTIFACTS / (label + ".log")).open("w") as log:
            process = subprocess.Popen(args, env=environment, stdout=log, stderr=log)
            ui = None
            try:
                window = window_for(process, log.name)
                ui = ProjectUI(window, process, log.name, environment)
                ui.project(first, remote)
                if fallback:
                    assert "[Sandbox]" not in ui.title()
                ui.edit_file(first / "alpha.rs", label + "-first-saved")
                screenshot(window, label + "-project.png")
                if launch_copy is not None:
                    replacement = base / "rode-rebuilt"
                    shutil.copy2(BINARY, replacement)
                    os.replace(replacement, launch_copy)
                    ui.edit_file(first / "alpha.rs", "saved-after-executable-replacement")
                    ui.command("Terminal: New terminal")
                    ui.terminal(first)
                    text("pwd > after-update.cwd\n")
                    ui.saved(first / "after-update.cwd", str(first) + "\n")
                    if landlock:
                        infos = session_rpc(environment, "list")
                        assert infos and all(
                            info["mode"] == "SANDBOX · Landlock" for info in infos
                        ), infos
                ui.open_project(second, remote, label + "-folder-picker.png")
                ui.edit_file(second / "beta.rs", label + "-second-saved")
                screenshot(window, label + "-second-project.png")
                key("t", ("Control_L", "Shift_L"))
                selected = ui.terminal(second)
                text("pwd > cwd.txt\n")
                ui.saved(second / "cwd.txt", str(second) + "\n")
                if remote:
                    infos = session_rpc(environment, "list")
                    assert len(infos) == 1, infos
                    persistent = selected
                    ui.click("terminal-persist:off")
                    ui.wait(
                        lambda: any(
                            info["id"] == persistent["id"] and info["persistent"]
                            for info in session_rpc(environment, "list")
                        ),
                        "Could not enable remote terminal persistence",
                    )
                    ui.command("Terminal: New terminal")
                    ui.wait(
                        lambda: len(session_rpc(environment, "list")) == 2,
                        "No second remote terminal",
                    )
                    temporary = ui.terminal(second)
                    assert temporary["id"] != persistent["id"], temporary
                    assert not temporary["persistent"], temporary
                    # The complete dirty buffer must survive disconnect, while
                    # the nonpersistent terminal must not survive its last view.
                    contents = (second / "beta.rs").read_text()
                    ui.append(contents, "survives-disconnect")
                    ui.resize(850, 560)
                    screenshot(window, "ssh-disconnect-compact.png")
                    ui.command("Remote: Disconnect SSH")
                    ui.disconnected()
                    ui.wait(
                        lambda: (
                            [info["id"] for info in session_rpc(environment, "list")]
                            == [persistent["id"]]
                        ),
                        "The persistent terminal was not the only surviving session",
                    )
                    assert (second / "beta.rs").read_text() == contents
                    screenshot(window, "ssh-disconnected.png")
                    # Dirty disconnected remote tabs must not keep folder
                    # discovery bound to the lost host.
                    ui.picker()
                    screenshot(window, "ssh-disconnected-local-picker.png")
                    ui.click("project-dialog-cancel")
                    ui.absent("project-dialog-path")
                    ui.command("Remote: Reconnect SSH")
                    ui.project(second, remote=True)
                    ui.terminal(second)
                    screenshot(window, "ssh-reconnected-compact.png")
                    ui.resize(1380, 900)
                    ui.editor()
                    ui.copy("source-editor", contents + "\n// survives-disconnect")
                    key("s", ("Control_L",))
                    ui.saved_edit(second / "beta.rs", "survives-disconnect")
                    ui.command("Terminal: Sessions and Slurm allocations")
                    ui.layout(
                        lambda state: persistent["id"] in (state["session_ids"] or []),
                        "The reconnected session manager did not list the persistent terminal",
                    )
                    screenshot(window, "ssh-reconnected-sessions.png")
                    key("Escape")
                    ui.layout(
                        lambda state: state["session_ids"] is None,
                        "The session manager did not close",
                    )
                    ui.command("Remote: Disconnect SSH")
                    ui.disconnected()
                    ui.command("Remote: Reconnect SSH")
                    ui.project(second, remote=True)
                    ui.terminal(second)
                    # Close only this GUI's private master to exercise
                    # unexpected transport loss rather than a palette action.
                    sockets = list(Path(tempfile.gettempdir()).glob(f"rode-{process.pid}-*/ssh"))
                    assert len(sockets) == 1, sockets
                    with socket.socket(socket.AF_UNIX) as control:
                        control.connect(str(sockets[0]))
                        owner, uid, _ = struct.unpack(
                            "3i", control.getsockopt(socket.SOL_SOCKET, socket.SO_PEERCRED, 12)
                        )
                        assert uid == os.getuid()
                        arguments = Path(f"/proc/{owner}/cmdline").read_bytes().split(b"\0")
                        assert b"--ssh-client-master" in arguments, arguments
                        assert str(sockets[0]).encode() in arguments, arguments
                        os.kill(owner, signal.SIGTERM)
                    ui.disconnected()
                    screenshot(window, "ssh-lost-local-mode.png")
                    ui.open_project(local)
                    ui.edit_file(local / "local.rs", "saved-locally-after-ssh-loss")
                    screenshot(window, "ssh-lost-local-project.png")
                    session_rpc(
                        environment, "end", id=persistent["id"], policy=persistent["policy"]
                    )
                close_window(window, process)
                print(
                    label
                    + ": native folder browsing, project switch, edit/save, and host indicators passed."
                )
            except Exception:
                if ui is not None:
                    ui.failure(label)
                raise
            finally:
                if process.poll() is None:
                    process.terminate()
                    process.wait(timeout=5)
                if remote:
                    try:
                        for info in session_rpc(environment, "list"):
                            session_rpc(environment, "end", id=info["id"], policy=info["policy"])
                    except (subprocess.SubprocessError, OSError, ValueError):
                        pass


def failed_connection():
    """A real refused SSH login must leave copyable diagnostics, not a local shell."""
    with tempfile.TemporaryDirectory(prefix="rode-ssh-failure-ui-") as directory:
        base = Path(directory)
        # Reserve a loopback port without listening; no real host/account is contacted.
        with socket.socket() as reserved:
            reserved.bind(("127.0.0.1", 0))
            port = reserved.getsockname()[1]
            config = base / "ssh_config"
            config.write_text(
                f"Host rode-failure\n HostName 127.0.0.1\n Port {port}\n"
                " BatchMode yes\n ConnectTimeout 2\n ConnectionAttempts 1\n"
            )
            environment = dict(
                os.environ,
                XDG_CONFIG_HOME=str(base / "config"),
                XDG_DATA_HOME=str(base / "data"),
                RODE_SESSION_DIR=str(base / "sessions"),
                RODE_UI_TEST_STATE=str(base / "ui-state.json"),
            )
            with (ARTIFACTS / "ssh-login-failure.log").open("w") as log:
                process = subprocess.Popen(
                    [str(BINARY), "--ssh", "rode-failure", "--ssh-config", str(config)],
                    cwd=base,
                    env=environment,
                    stdout=log,
                    stderr=log,
                )
                ui = None
                try:
                    window = window_for(process, log.name)
                    ui = ProjectUI(window, process, log.name, environment)
                    ui.control("terminal-output-ended")
                    ui.disconnected()
                    screenshot(window, "ssh-login-error-output.png")
                    ui.click("terminal-output-ended")
                    ui.control("terminal-output-ended", focused=True)
                    text("echo should-not-run\n")
                    # Copy's mouse-hover acknowledgement processes pending
                    # terminal input without depending on an idle repaint or
                    # opening a palette that could consume the rejected Enter.
                    ui.click("terminal-copy-output")

                    def copied_error():
                        output = ui.clipboard()
                        return (
                            output
                            if output
                            and "Connection refused" in output
                            and f"port {port}:" in output
                            else None
                        )

                    output = ui.wait(
                        copied_error, "Terminal copy did not expose the refused SSH diagnostic"
                    )
                    assert "should-not-run" not in output, output
                    ui.command("New file")
                    ui.control("source-editor", focused=True)
                    before = ui.layout()["pass"]
                    key("v", ("Control_L",))
                    ui.control("source-editor", focused=True, changed_after=before)
                    ui.copy("source-editor", output)
                    key("s", ("Control_L",))
                    ui.control("file-dialog-path", focused=True)
                    copied = base / "copied-ssh-error.txt"
                    ui.path("file-dialog-path", str(copied))
                    key("Return")
                    ui.absent("file-dialog-path")
                    ui.saved(copied, output)
                    screenshot(window, "ssh-login-error-copied.png")
                    ui.picker()
                    screenshot(window, "ssh-login-error-local-picker.png")
                    ui.click("project-dialog-cancel")
                    ui.absent("project-dialog-path")
                    assert "[SSH" not in ui.title(), ui.title()
                    close_window(window, process)
                    print(
                        "SSH failure: retained read-only client error, Copy and local opening passed."
                    )
                except Exception:
                    if ui is not None:
                        ui.failure("ssh-login")
                    raise
                finally:
                    if process.poll() is None:
                        process.terminate()
                        process.wait(timeout=5)


case = os.environ.get("RODE_PROJECT_UI_CASE", "all")
if case in ("all", "failure"):
    failed_connection()
if case in ("all", "sandbox"):
    run()
if case in ("all", "fallback"):
    run(fallback=True)
if case in ("all", "ssh") and os.environ.get("RODE_TEST_SSH_CONFIG"):
    run(remote=True)
