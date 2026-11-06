#!/usr/bin/env python3
"""Open, navigate, edit, save and undo a binary larger than the old file/pixel budgets."""

import json
import os
import subprocess
import tempfile
import time
from pathlib import Path

os.environ["RODE_UI_ARTIFACTS"] = str(
    Path(__file__).resolve().parent.parent
    / (
        "target/large-files-ssh-ui"
        if os.environ.get("RODE_LARGE_FILES_UI_REMOTE") == "1"
        else "target/large-files-ui"
    )
)
from ui_harness import (
    ARTIFACTS,
    BINARY,
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


def wait(check, message):
    until = time.monotonic() + 30
    while time.monotonic() < until:
        assert process.poll() is None, "RODE exited: " + log.name
        if check():
            return
        time.sleep(0.05)
    raise AssertionError(message)


def tail(path):
    with path.open("rb") as source:
        source.seek(-1, os.SEEK_END)
        return source.read(1)


with tempfile.TemporaryDirectory(prefix="rode-large-files-ui-") as directory:
    base = Path(directory)
    root = base / "project"
    root.mkdir()
    path = root / "large.bin"
    length = 49 * 1024 * 1024
    with path.open("wb") as source:
        source.truncate(length)
        source.write(b"\x11")
        source.seek(-1, os.SEEK_END)
        source.write(b"\xcc")
    state = base / "ui-state.json"
    environment = dict(
        os.environ,
        XDG_CONFIG_HOME=str(base / "config"),
        XDG_DATA_HOME=str(base / "data"),
        RODE_UI_TEST_STATE=str(state),
    )
    with (ARTIFACTS / "large-files.log").open("w") as log:
        remote = os.environ.get("RODE_LARGE_FILES_UI_REMOTE") == "1"
        arguments = [str(BINARY), str(root), "--theme", "studio-neutral"]
        if remote:
            arguments = [
                str(BINARY),
                "--ssh",
                "rode-test",
                "--ssh-config",
                os.environ["RODE_TEST_SSH_CONFIG"],
                "--remote-path",
                str(root),
                "--theme",
                "studio-neutral",
            ]
        process = subprocess.Popen(
            arguments,
            env=environment,
            stdout=log,
            stderr=log,
        )
        window = None
        try:
            window = wait_for_window(process, log.name)
            wait_for_project(window, process, log.name, root)
            focus_window(window)

            def point(name, **guards):
                return wait_for_button(window, process, log.name, state, name, **guards)

            def button(name):
                click_button(window, process, log.name, state, name)

            def dirty():
                return not any(
                    item["name"] == "hex-saved" for item in json.loads(state.read_text())["buttons"]
                )

            # SSH login initially owns terminal focus. Open through the real
            # explorer row rather than sending Ctrl+P to that shell.
            button("explorer-row:" + str(path))
            point("hex-ready", timeout=60)
            point("hex-next-page")
            button("hex-offset")
            point("hex-offset", focused=True)
            key("a", ("Control_L",))
            text(f"{length - 1:X}")
            button("hex-go")
            selected = f"hex-byte:{length - 1}"
            point(selected)
            screenshot(window, "large-binary-tail.png")
            button("hex-edit")
            button("hex-patch")
            point("hex-patch", focused=True)
            key("a", ("Control_L",))
            before = json.loads(state.read_text())["pass"]
            text("AB")
            point("hex-patch", focused=True, changed_after=before)
            button("hex-overwrite")
            wait(dirty, "Large-file overwrite did not mark the buffer dirty")
            key("s", ("Control_L",))
            wait(lambda: tail(path) == b"\xab", "Large binary save did not preserve the tail edit")
            point("hex-saved", timeout=60)
            assert path.stat().st_size == length
            with path.open("rb") as source:
                assert source.read(1) == b"\x11"
            button(selected)
            point("hex-grid", focused=True)
            key("z", ("Control_L",))
            wait(dirty, "Large-file undo did not mark the saved buffer dirty")
            key("s", ("Control_L",))
            wait(lambda: tail(path) == b"\xcc", "Undo/save did not restore the original tail byte")
            point("hex-saved", timeout=60)
            screenshot(window, "large-binary-saved.png")
            close_window(window, process)
            print(
                "49 MiB binary: precise tail navigation, overwrite, save, undo and resave passed."
            )
        except Exception:
            if state.exists():
                (ARTIFACTS / "failure-state.json").write_bytes(state.read_bytes())
            if window is not None and process.poll() is None:
                screenshot(window, "failure.png")
            raise
        finally:
            if process.poll() is None:
                process.terminate()
                process.wait(timeout=5)
