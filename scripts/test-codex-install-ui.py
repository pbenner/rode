#!/usr/bin/env python3
"""Exercise explicit rootless runtime setup through native controls, without external requests.

A loopback HTTPS proxy holds/rejects CONNECT before TLS, so the normal test cannot
reach GitHub. RODE_TEST_CODEX_BUNDLE optionally supplies the actual pinned runtime
to exercise verified reuse and successful publication too; no agent/model is started.
"""

import ctypes
import fcntl
import http.server
import json
import os
import platform
import re
import shutil
import subprocess
import tempfile
import threading
import time
from pathlib import Path

os.environ["RODE_UI_ARTIFACTS"] = str(
    Path(__file__).resolve().parent.parent / "target/codex-install-ui"
)
from ui_harness import (
    ARTIFACTS,
    BINARY,
    ROOT,
    click,
    close_window,
    display,
    focus_window,
    key,
    screenshot,
    x11,
)

x11.XResizeWindow.argtypes = [ctypes.c_void_p, ctypes.c_ulong, ctypes.c_uint, ctypes.c_uint]


def wait_for(check, message):
    deadline = time.monotonic() + 15
    while time.monotonic() < deadline:
        if check():
            return
        time.sleep(0.05)
    raise AssertionError(message)


class Proxy(http.server.BaseHTTPRequestHandler):
    requests = 0
    release = threading.Event()

    def do_CONNECT(self):
        Proxy.requests += 1
        Proxy.release.wait(10)
        try:
            self.send_error(503, "Controlled test proxy: no external connection")
        except (BrokenPipeError, ConnectionResetError):
            pass  # A cancelled request may close before the controlled rejection.

    def log_message(self, *_args):
        pass


def idle(base):
    """Observe completion via the shared lock, without changing installer state."""
    try:
        with (base / ".install.lock").open("rb") as lock:
            fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
            return True
    except (FileNotFoundError, BlockingIOError):
        return False


with tempfile.TemporaryDirectory(prefix="rode-codex-install-ui-") as directory:
    version = json.loads((ROOT / "packaging/codex.json").read_text())["version"]
    root = Path(directory)
    # A developer checkout may have a prepared runtime beside target/debug.
    # Exercise a fresh installation without discovering that unrelated bundle.
    binary = root / "bin/rode"
    binary.parent.mkdir()
    try:
        os.link(BINARY, binary)
    except OSError:
        shutil.copy2(BINARY, binary)
    project = root / "project"
    project.mkdir()
    auth = root / "codex-config"
    auth.mkdir()
    marker = auth / "config.toml"
    marker.write_text("# User configuration must stay unchanged\n")
    server = http.server.ThreadingHTTPServer(("127.0.0.1", 0), Proxy)
    server.daemon_threads = True
    threading.Thread(target=server.serve_forever, daemon=True).start()
    proxy = f"http://127.0.0.1:{server.server_port}"
    environment = dict(
        os.environ,
        XDG_CONFIG_HOME=str(root / "config"),
        XDG_DATA_HOME=str(root / "data"),
        CODEX_HOME=str(auth),
        HTTPS_PROXY=proxy,
        https_proxy=proxy,
        ALL_PROXY=proxy,
        all_proxy=proxy,
        NO_PROXY="",
        no_proxy="",
    )
    environment.pop("RODE_CODEX_BUNDLE", None)
    base = root / "data/rode/codex"
    base.parent.mkdir(parents=True)
    base.parent.chmod(0o775)  # An older preferences directory with a shared umask.
    preferences_marker = base.parent / "unrelated-settings"
    preferences_marker.write_text("preserve existing settings")
    window = None
    with (ARTIFACTS / "install.log").open("w") as log:
        process = subprocess.Popen(
            [str(binary), str(project)], env=environment, stdout=log, stderr=log
        )
        try:
            for _ in range(150):
                tree = subprocess.check_output(["xwininfo", "-root", "-tree"], text=True)
                match = re.search(r'(0x[0-9a-f]+) "RODE[^\n]*project', tree)
                if match and "[Opening project]" not in match[0]:
                    window = int(match[1], 16)
                    break
                assert process.poll() is None, "RODE exited before project preparation"
                time.sleep(0.05)
            assert window, "No prepared RODE window"
            focus_window(window)
            time.sleep(0.6)
            key("a", ("Control_L", "Shift_L"))
            time.sleep(0.3)
            screenshot(window, "before-install.png")
            assert Proxy.requests == 0 and not base.exists(), "Opening chat initiated setup"
            click(360, 211)
            wait_for(
                lambda: Proxy.requests == 1, "Install Codex did not start the explicit request"
            )
            screenshot(window, "downloading.png")
            click(440, 211)  # Cancel the held request.
            Proxy.release.set()
            wait_for(lambda: idle(base), "Cancelled installation did not release its lock")
            time.sleep(0.2)
            assert not (base / "manifest.json").exists(), "Cancelled install selected a runtime"
            screenshot(window, "cancelled.png")
            click(360, 211)
            wait_for(lambda: Proxy.requests == 2, "Failed/cancelled setup could not be retried")
            wait_for(lambda: idle(base), "Rejected download did not finish")
            screenshot(window, "download-error.png")

            if source := os.environ.get("RODE_TEST_CODEX_BUNDLE"):
                source = Path(source)
                manifest = json.loads((source / "manifest.json").read_text())
                destination = base / manifest["version"] / platform.machine()
                destination.mkdir(mode=0o700, parents=True, exist_ok=True)
                destination.chmod(0o700)
                executables = {"codex"} | set(
                    manifest["targets"][platform.machine()].get("helpers", {})
                )
                for name in (
                    list(executables) + ["manifest.json"] + [n["file"] for n in manifest["notices"]]
                ):
                    try:
                        os.link(source / name, destination / name)
                    except OSError:
                        shutil.copy2(source / name, destination / name)
                click(360, 211)
                wait_for(
                    lambda: (base / "manifest.json").exists(),
                    "Install action did not publish the verified cached runtime",
                )
                wait_for(lambda: idle(base), "Cached setup did not finish")
                assert (base / "manifest.json").read_bytes() == (
                    source / "manifest.json"
                ).read_bytes()
                assert Proxy.requests == 2, "Cached runtime triggered a download"
                screenshot(window, "installed.png")

            assert marker.read_text() == "# User configuration must stay unchanged\n"
            assert preferences_marker.read_text() == "preserve existing settings"
            assert base.parent.stat().st_mode & 0o022 == 0
            assert sorted(p.name for p in auth.iterdir()) == ["config.toml"]
            assert list(project.iterdir()) == [], "Setup touched project files or started an agent"
            x11.XResizeWindow(display, window, 850, 560)
            x11.XFlush(display)
            screenshot(window, "minimum-install.png")
            if not os.environ.get("RODE_TEST_CODEX_BUNDLE"):
                x11.XResizeWindow(display, window, 1380, 900)
                x11.XFlush(display)
                time.sleep(0.2)
                Proxy.release.clear()
                click(360, 211)
                wait_for(lambda: Proxy.requests == 3, "Close-during-install fixture did not start")
            close_window(window, process)
            window = None
            if not os.environ.get("RODE_TEST_CODEX_BUNDLE"):
                assert sorted(p.name for p in base.iterdir()) == [".install.lock", version]
                assert list((base / version).iterdir()) == []
            print(
                "Native Codex setup: explicit start, cancellation, retry, private paths and untouched credentials passed."
            )
        except BaseException:
            if window:
                screenshot(window, "failure.png")
            raise
        finally:
            Proxy.release.set()
            if process.poll() is None:
                process.terminate()
                process.wait(timeout=8)
            server.shutdown()
