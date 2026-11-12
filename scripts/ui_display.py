#!/usr/bin/env python3
"""Run native UI tests on an owned headless display, never the user's desktop."""

import argparse
import os
import re
import shutil
import signal
import subprocess
from pathlib import Path


def stop_group(process):
    """Clean up only the process group created for this invocation, including on interruption."""
    try:
        os.killpg(process.pid, signal.SIGTERM)
    except ProcessLookupError:
        pass
    try:
        process.wait(timeout=5)
    except subprocess.TimeoutExpired:
        pass
    try:
        os.killpg(process.pid, signal.SIGKILL)
    except ProcessLookupError:
        pass
    process.wait()


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--screen", default="1600x1100x24", help="Xvfb WIDTHxHEIGHTxDEPTH")
    parser.add_argument("command", nargs=argparse.REMAINDER)
    args = parser.parse_args(argv)
    command = args.command[1:] if args.command[:1] == ["--"] else args.command
    if not command:
        parser.error("provide a test command after --")
    if not re.fullmatch(r"[1-9][0-9]*x[1-9][0-9]*x[1-9][0-9]*", args.screen):
        parser.error("--screen must be WIDTHxHEIGHTxDEPTH")
    missing = [name for name in ("xvfb-run", "Xvfb", "xauth") if not shutil.which(name)]
    if missing:
        parser.error(
            "headless UI testing requires "
            + ", ".join(missing)
            + "; no test was started. Do not fall back to the desktop or a visible Xephyr window."
        )

    environment = dict(os.environ)
    for name in ("DISPLAY", "WAYLAND_DISPLAY", "WAYLAND_SOCKET", "XAUTHORITY"):
        environment.pop(name, None)
    environment["WINIT_UNIX_BACKEND"] = "x11"
    environment["LIBGL_ALWAYS_SOFTWARE"] = "1"
    environment["__GLX_VENDOR_LIBRARY_NAME"] = "mesa"
    mesa = Path("/usr/share/glvnd/egl_vendor.d/50_mesa.json")
    if mesa.exists():
        environment["__EGL_VENDOR_LIBRARY_FILENAMES"] = str(mesa)

    def terminate(signum, _frame):
        raise SystemExit(128 + signum)

    previous = signal.signal(signal.SIGTERM, terminate)
    process = None
    try:
        process = subprocess.Popen(
            [
                shutil.which("xvfb-run"),
                "--auto-servernum",
                "--server-args",
                # Short-lived probes must not reset display/authentication state
                # between cases. This invocation still owns server cleanup.
                f"-screen 0 {args.screen} -nolisten tcp -noreset",
                *command,
            ],
            env=environment,
            start_new_session=True,
        )
        return process.wait()
    except KeyboardInterrupt:
        return 130
    finally:
        if process is not None:
            stop_group(process)
        signal.signal(signal.SIGTERM, previous)


if __name__ == "__main__":
    raise SystemExit(main())
