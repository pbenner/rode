#!/usr/bin/env python3
"""Encrypted-project UI fails closed and preserves dirty local/SSH buffers."""

import ctypes
import hashlib
import json
import os
import re
import shlex
import shutil
import subprocess
import tempfile
import time
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
os.environ["RODE_UI_ARTIFACTS"] = str(ROOT / "target/vaults-ui")
from ui_harness import (
    ARTIFACTS,
    BINARY,
    click,
    close_window,
    display,
    focus_window,
    key,
    request_close,
    screenshot,
    text,
    x11,
)

x11.XResizeWindow.argtypes = [ctypes.c_void_p, ctypes.c_ulong, ctypes.c_uint, ctypes.c_uint]


def wait_for(check, message):
    until = time.monotonic() + 15
    while time.monotonic() < until:
        if check():
            return
        time.sleep(0.05)
    raise AssertionError(message)


def command(value):
    key("p", ("Control_L", "Shift_L"))
    text(value)
    key("Return")
    time.sleep(0.3)


def cleanup_result(cipher, remote, environment):
    """Check the fixture's real host lock result; namespace restrictions must fail closed."""
    command = [str(BINARY), "--vault-helper"]
    if remote:
        command = [
            "ssh",
            "-F",
            os.environ["RODE_TEST_SSH_CONFIG"],
            "-T",
            "rode-test",
            shlex.join(["python3", "-u", "-c", (ROOT / "src/remote_vault.py").read_text()]),
        ]
    request = dict(
        settings=dict(cipherdir=str(cipher), program="/usr/bin/true", unmount_program=""),
        password="",
        create=False,
        lock_only=True,
    )
    result = subprocess.run(
        command,
        input=json.dumps(request) + "\n",
        text=True,
        capture_output=True,
        env=environment,
        timeout=15,
        check=True,
    )
    reply = json.loads(result.stdout)
    assert reply["kind"] == "closed", reply
    if not reply["locked"]:
        assert reply["message"].startswith("Cannot verify other mount namespaces"), reply
    return reply


def run(remote=False):
    label = "ssh" if remote else "local"
    with tempfile.TemporaryDirectory(prefix="rode-vault-ui-") as directory:
        base = Path(directory)
        project, cipher = base / "plain-project", base / "encrypted"
        project.mkdir()
        cipher.mkdir()
        (cipher / "gocryptfs.conf").write_text("fixture")
        source = project / "main.txt"
        source.write_text("keep the existing project\n" + "\n" * 8)
        environment = dict(
            os.environ,
            XDG_CONFIG_HOME=str(base / "config"),
            XDG_DATA_HOME=str(base / "data"),
            RODE_SESSION_DIR=str(base / "sessions"),
        )
        args = [str(BINARY), "--theme", "carbon"]
        if remote:
            tools = base / "tools"
            tools.mkdir()
            wrapper = tools / "ssh"
            wrapper.write_text(
                "#!/usr/bin/python3\nimport os,sys\n"
                + f"binary={shutil.which('ssh')!r}\n"
                + "os.execv(binary,[binary]+['-T' if arg=='-tt' else arg for arg in sys.argv[1:]])\n"
            )
            wrapper.chmod(0o755)
            environment["PATH"] = str(tools) + os.pathsep + os.environ["PATH"]
            args += [
                "--ssh",
                "rode-test",
                "--ssh-config",
                os.environ["RODE_TEST_SSH_CONFIG"],
                "--remote-path",
                str(project),
            ]
        else:
            args.append(str(project))
        with (ARTIFACTS / f"{label}.log").open("w") as log:
            process = subprocess.Popen(args, env=environment, stdout=log, stderr=log)
            window = None
            try:

                def ready():
                    nonlocal window
                    tree = subprocess.check_output(["xwininfo", "-root", "-tree"], text=True)
                    match = re.search(r'(0x[0-9a-f]+) "RODE.*plain-project[^\n]*', tree)
                    if match and "[Opening project]" not in match[0]:
                        window = int(match[1], 16)
                        return True
                    assert process.poll() is None
                    return False

                wait_for(ready, "The fixture project did not open")
                time.sleep(1)
                focus_window(window)
                command("Open file by path")
                text("main.txt")
                key("Return")
                time.sleep(0.8)
                click(600, 250)
                key("End", ("Control_L",))
                text("\nunsaved-vault-marker")
                command("Project: Open encrypted project")
                text(str(cipher))
                key("Tab")
                key("a", ("Control_L",))
                text("/usr/bin/true")
                key("Tab")  # Unmount executable
                key("Tab")  # Creation checkbox, left off
                key("Tab")  # Password
                text("RODEUiSecret4821")
                key("Tab")  # Sandbox checkbox
                key("Tab")  # Unlock button
                key("Return")
                time.sleep(1.3)
                screenshot(window, label + "-refused-plaintext.png")
                assert "unsaved-vault-marker" not in source.read_text()
                assert "plain-project" in subprocess.check_output(
                    ["xwininfo", "-id", hex(window)], text=True
                )
                x11.XResizeWindow(display, window, 850, 560)
                x11.XFlush(display)
                time.sleep(0.4)
                screenshot(window, label + "-compact.png")
                key("Escape")
                x11.XResizeWindow(display, window, 1380, 900)
                x11.XFlush(display)
                time.sleep(0.3)
                click(600, 250)
                key("s", ("Control_L",))
                wait_for(
                    lambda: "unsaved-vault-marker" in source.read_text(),
                    "Unlock failure lost the dirty source buffer",
                )
                # An ordinary failed mount still requires verified cleanup.
                # Hardened hosts can deny /proc namespace inspection; only that
                # explicit refusal permits expecting the exit guard here.
                cleanup = cleanup_result(cipher, remote, environment)
                (ARTIFACTS / f"{label}-cleanup.json").write_text(json.dumps(cleanup, indent=2))
                if cleanup["locked"]:
                    close_window(window, process)
                else:
                    request_close(window)
                    try:
                        process.wait(timeout=2)
                    except subprocess.TimeoutExpired:
                        pass
                    else:
                        raise AssertionError("RODE exited without verified encrypted cleanup")
                    screenshot(window, label + "-cleanup-blocked.png")
                    print(label + ": namespace inspection refused; close correctly stayed blocked.")
                for path in (base / "data").rglob("*"):
                    if path.is_file():
                        assert b"RODEUiSecret4821" not in path.read_bytes(), path
                print(
                    label
                    + ": rejected an ordinary directory, preserved dirty buffers, saved no password, and compact dialog passed."
                )
            except Exception:
                if window is not None and process.poll() is None:
                    screenshot(window, label + "-failure.png")
                raise
            finally:
                if process.poll() is None:
                    process.terminate()
                    process.wait(timeout=5)
                runtime = (
                    Path(f"/tmp/rode-vaults-{os.geteuid()}")
                    / hashlib.sha256(str(cipher).encode()).hexdigest()
                )
                point = runtime / "plain"
                if point.exists():
                    point.chmod(0o700)
                    point.rmdir()
                    for name in ("operation.lock", "views.lock"):
                        (runtime / name).unlink(missing_ok=True)
                    runtime.rmdir()


if os.environ.get("RODE_VAULT_UI_REMOTE_ONLY") != "1":
    run()
if os.environ.get("RODE_TEST_SSH_CONFIG"):
    run(remote=True)
