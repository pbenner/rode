#!/usr/bin/env python3
"""Native project/sandbox UI checks; optional isolated SSH daemon from test-ssh.sh."""

import ctypes
import json
import os
import shutil
import socket
import subprocess
import tempfile
import time
from pathlib import Path

os.environ["RODE_UI_ARTIFACTS"] = str(Path(__file__).resolve().parent.parent / "target/project-ui")
from ui_harness import (
    ARTIFACTS,
    BINARY,
    click,
    close_window,
    display,
    key,
    screenshot,
    text,
    wait_for_window,
    x11,
)

x11.XResizeWindow.argtypes = [ctypes.c_void_p, ctypes.c_ulong, ctypes.c_uint, ctypes.c_uint]


def window_for(process, log_path):
    window = wait_for_window(process, log_path)
    x11.XSetInputFocus(display, window, 1, 0)
    x11.XFlush(display)
    return window


def wait_for(check, message):
    deadline = time.monotonic() + 8
    while time.monotonic() < deadline:
        if check():
            return
        time.sleep(0.05)
    raise AssertionError(message)


def edit_file(path, marker):
    # This remains available while the remote login/project terminal has focus;
    # Ctrl+P by itself belongs to the shell in that case.
    key("p", ("Control_L", "Shift_L"))
    text("Open file by path")
    key("Return")
    time.sleep(0.15)
    text(path.name)
    key("Return")
    time.sleep(0.5)
    click(600, 250)
    key("End", ("Control_L",))
    text("\n// " + marker)
    key("s", ("Control_L",))
    wait_for(lambda: marker in path.read_text(), "File was not saved: " + str(path))


def title(window):
    return subprocess.check_output(["xwininfo", "-id", hex(window)], text=True)


def workspace_y(window, y):
    # A prepared project adds a host bar and, in fallback mode, a reason strip.
    return y + (44 if "[Sandbox]" in title(window) else 74)


def session_rpc(environment, action, **values):
    reply = json.loads(
        subprocess.check_output(
            [str(BINARY), "--session-service", "request"],
            input=json.dumps(dict(action=action, **values)).encode(),
            env=environment,
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
                '// project file\nfn main() {\n    println!("remote host");\n}\n' + "\n" * 5
            )
        environment = dict(
            os.environ,
            XDG_CONFIG_HOME=str(base / "config"),
            XDG_DATA_HOME=str(base / "data-home"),
            RODE_SESSION_DIR=str(base / "sessions"),
        )
        if fallback:
            tools = base / "no-bwrap"
            tools.mkdir()
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
            window = None
            try:
                window = window_for(process, log.name)
                wait_for(
                    lambda: (
                        "first-project" in title(window)
                        and "[Opening project]" not in title(window)
                    ),
                    "Initial project did not open",
                )
                if remote:
                    assert "SSH" in title(window)
                if fallback:
                    assert "[Sandbox]" not in title(window)
                edit_file(first / "alpha.rs", label + "-first-saved")
                screenshot(window, label + "-project.png")
                if launch_copy is not None:
                    replacement = base / "rode-rebuilt"
                    shutil.copy2(BINARY, replacement)
                    os.replace(replacement, launch_copy)
                    edit_file(first / "alpha.rs", "saved-after-executable-replacement")
                    key("p", ("Control_L", "Shift_L"))
                    text("Terminal: New terminal")
                    key("Return")
                    time.sleep(0.5)
                    text("pwd > after-update.cwd\n")
                    wait_for(
                        lambda: (first / "after-update.cwd").exists(),
                        "A rebuilt editor could not start a new native terminal",
                    )
                    assert (first / "after-update.cwd").read_text().strip() == str(first)
                key("o", ("Control_L", "Shift_L"))
                time.sleep(0.3)
                key("a", ("Control_L",))
                text(str(second))
                key("Return")  # Browse the selected host, without changing project yet.
                time.sleep(0.5)
                screenshot(window, label + "-folder-picker.png")
                key("Return", ("Control_L",))
                wait_for(
                    lambda: (
                        "second-project" in title(window)
                        and "[Opening project]" not in title(window)
                    ),
                    "Project switch did not complete on " + label,
                )
                time.sleep(0.6)
                if remote:
                    assert "SSH" in title(window), (
                        "Opening a remote folder must not switch to local mode"
                    )
                edit_file(second / "beta.rs", label + "-second-saved")
                screenshot(window, label + "-second-project.png")
                key("t", ("Control_L", "Shift_L"))
                time.sleep(0.3)
                click(600, 740)
                text("pwd > cwd.txt\n")
                wait_for(
                    lambda: (second / "cwd.txt").exists(),
                    "Shell did not start in the selected project",
                )
                assert (second / "cwd.txt").read_text().strip() == str(second)
                if remote:
                    infos = session_rpc(environment, "list")
                    assert len(infos) == 1, infos
                    persistent = infos[0]
                    # Enable the real per-terminal toggle and verify that this
                    # session, rather than the new nonpersistent one, survives.
                    click(1326, workspace_y(window, 108))
                    wait_for(
                        lambda: any(
                            info["id"] == persistent["id"] and info["persistent"]
                            for info in session_rpc(environment, "list")
                        ),
                        "Could not enable remote terminal persistence",
                    )
                    key("p", ("Control_L", "Shift_L"))
                    text("Terminal: New terminal")
                    key("Return")
                    wait_for(
                        lambda: len(session_rpc(environment, "list")) == 2,
                        "No second remote terminal",
                    )
                    time.sleep(0.4)
                    # Dirty the remote source before disconnecting. The source
                    # must remain in its remote tab and save after reconnect.
                    click(339, workspace_y(window, 69))  # Code replaces the terminal.
                    click(600, 250)
                    key("End", ("Control_L",))
                    text("\n// survives-disconnect")
                    x11.XResizeWindow(display, window, 850, 560)
                    x11.XFlush(display)
                    time.sleep(0.4)
                    screenshot(window, "ssh-disconnect-compact.png")
                    key("p", ("Control_L", "Shift_L"))
                    text("Remote: Disconnect SSH")
                    key("Return")
                    wait_for(lambda: '"RODE"' in title(window), "Disconnect did not clear the host")
                    wait_for(
                        lambda: len(session_rpc(environment, "list")) == 1,
                        "A nonpersistent remote terminal survived its last view",
                    )
                    assert session_rpc(environment, "list")[0]["id"] == persistent["id"]
                    assert "survives-disconnect" not in (second / "beta.rs").read_text()
                    screenshot(window, "ssh-disconnected.png")
                    # Folder opening now defaults to this machine immediately,
                    # even while a dirty disconnected remote tab is retained.
                    key("o", ("Control_L", "Shift_L"))
                    time.sleep(0.4)
                    screenshot(window, "ssh-disconnected-local-picker.png")
                    key("Escape")
                    key("p", ("Control_L", "Shift_L"))
                    text("Remote: Reconnect SSH")
                    key("Return")
                    wait_for(
                        lambda: (
                            "SSH" in title(window)
                            and "second-project" in title(window)
                            and "[Opening project]" not in title(window)
                        ),
                        "Reconnect did not restore the project",
                    )
                    time.sleep(0.5)
                    screenshot(window, "ssh-reconnected-compact.png")
                    x11.XResizeWindow(display, window, 1380, 900)
                    x11.XFlush(display)
                    time.sleep(0.4)
                    click(339, workspace_y(window, 69))
                    click(600, 250)
                    key("s", ("Control_L",))
                    wait_for(
                        lambda: "survives-disconnect" in (second / "beta.rs").read_text(),
                        "Reconnect lost the dirty remote buffer",
                    )
                    key("p", ("Control_L", "Shift_L"))
                    text("Terminal: Sessions and Slurm allocations")
                    key("Return")
                    time.sleep(0.6)
                    screenshot(window, "ssh-reconnected-sessions.png")
                    key("Escape")
                    # Disconnect once more after restoring the dirty buffer.
                    key("p", ("Control_L", "Shift_L"))
                    text("Remote: Disconnect SSH")
                    key("Return")
                    wait_for(lambda: '"RODE"' in title(window), "Palette disconnect failed")
                    # Reconnect once more, then close only this GUI's private
                    # master from outside the UI to exercise unexpected loss.
                    key("p", ("Control_L", "Shift_L"))
                    text("Remote: Reconnect SSH")
                    key("Return")
                    wait_for(
                        lambda: "SSH" in title(window) and "[Opening project]" not in title(window),
                        "Could not reconnect before connection-loss check",
                    )
                    time.sleep(0.5)
                    sockets = list(Path(tempfile.gettempdir()).glob(f"rode-{process.pid}-*/ssh"))
                    assert len(sockets) == 1, sockets
                    subprocess.run(
                        [
                            "ssh",
                            "-F",
                            os.environ["RODE_TEST_SSH_CONFIG"],
                            "-S",
                            str(sockets[0]),
                            "-O",
                            "exit",
                            "rode-test",
                        ],
                        check=True,
                        capture_output=True,
                    )
                    wait_for(lambda: '"RODE"' in title(window), "Lost SSH still selected a host")
                    screenshot(window, "ssh-lost-local-mode.png")
                    # A local project can open/save without any SSH connection.
                    key("o", ("Control_L", "Shift_L"))
                    time.sleep(0.3)
                    key("a", ("Control_L",))
                    text(str(local))
                    key("Return")
                    time.sleep(0.4)
                    key("Return", ("Control_L",))
                    wait_for(
                        lambda: (
                            "local-project" in title(window)
                            and "[Opening project]" not in title(window)
                        ),
                        "Could not open a local folder after connection loss",
                    )
                    assert "SSH" not in title(window), title(window)
                    time.sleep(0.5)
                    edit_file(local / "local.rs", "saved-locally-after-ssh-loss")
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
                if window is not None and process.poll() is None:
                    screenshot(window, label + "-failure.png")
                raise
            finally:
                if process.poll() is None:
                    process.terminate()
                    process.wait(timeout=5)
                if remote:
                    try:
                        for info in session_rpc(environment, "list"):
                            session_rpc(environment, "end", id=info["id"], policy=info["policy"])
                    except (subprocess.CalledProcessError, OSError, ValueError):
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
            )
            with (ARTIFACTS / "ssh-login-failure.log").open("w") as log:
                process = subprocess.Popen(
                    [str(BINARY), "--ssh", "rode-failure", "--ssh-config", str(config)],
                    cwd=base,
                    env=environment,
                    stdout=log,
                    stderr=log,
                )
                window = None
                try:
                    window = window_for(process, log.name)
                    wait_for(
                        lambda: '"RODE"' in title(window), "Failed SSH retained the host title"
                    )
                    screenshot(window, "ssh-login-error-output.png")
                    click(600, 740)
                    text("echo should-not-run\n")  # The failed-login view accepts no input.
                    # Use the same copy action as the header without depending on its layout.
                    key("p", ("Control_L", "Shift_L"))
                    text("Terminal: Copy output")
                    key("Return")

                    def copied_error():
                        result = subprocess.run(
                            ["xclip", "-selection", "clipboard", "-o"],
                            capture_output=True,
                            text=True,
                            timeout=2,
                        )
                        return result.returncode == 0 and "Connection refused" in result.stdout

                    wait_for(
                        copied_error, "Terminal copy did not expose the refused SSH diagnostic"
                    )
                    key("p", ("Control_L", "Shift_L"))
                    text("New file")
                    key("Return")
                    time.sleep(0.2)
                    key("v", ("Control_L",))
                    key("s", ("Control_L",))
                    time.sleep(0.25)
                    key("a", ("Control_L",))
                    copied = base / "copied-ssh-error.txt"
                    text(str(copied))
                    key("Return")
                    wait_for(copied.exists, "Could not save copied login output locally")
                    output = copied.read_text()
                    assert "Connection refused" in output, f"Copied SSH diagnostic: {output!r}"
                    assert "should-not-run" not in output, output
                    screenshot(window, "ssh-login-error-copied.png")
                    key("o", ("Control_L", "Shift_L"))
                    time.sleep(0.3)
                    screenshot(window, "ssh-login-error-local-picker.png")
                    key("Escape")
                    assert "SSH" not in title(window), title(window)
                    close_window(window, process)
                    print(
                        "SSH failure: retained read-only OpenSSH error, Copy and local opening passed."
                    )
                except Exception:
                    if window is not None and process.poll() is None:
                        screenshot(window, "ssh-login-failure.png")
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
