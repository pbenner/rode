#!/usr/bin/env python3
"""Select file-only access through native controls; exercise its host-owned tools."""

import ctypes
import json
import os
import re
import shutil
import subprocess
import tempfile
import time
from pathlib import Path

os.environ["RODE_UI_ARTIFACTS"] = str(
    Path(__file__).resolve().parent.parent / "target/agent-file-only-ui"
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
    text,
    wait_for_project,
    wait_for_window,
    x11,
)

x11.XResizeWindow.argtypes = [ctypes.c_void_p, ctypes.c_ulong, ctypes.c_uint, ctypes.c_uint]


def wait(check, message):
    until = time.monotonic() + 15
    while time.monotonic() < until:
        if check():
            return
        time.sleep(0.05)
    raise AssertionError(message)


def preferences(path):
    if not path.exists():
        return {}
    match = re.search(r'"agent_preferences"\s*:\s*("(?:\\.|[^"\\])*")', path.read_text())
    if not match:
        return {}
    saved = json.loads(json.loads(match[1]))
    return next(iter(saved.values()), {})


with tempfile.TemporaryDirectory(prefix="rode-agent-files-ui-") as directory:
    base = Path(directory)
    root = base / "project"
    root.mkdir()
    shutil.copy2(ROOT / "tests/fixtures/file_only_agent.py", root / "agent.py")
    (root / "agent.py").chmod(0o755)
    (root / "file-tool-steps.json").write_text(
        json.dumps(
            [
                {
                    "params": {
                        "tool": "rode_write_file",
                        "arguments": {
                            "path": "ui-created.txt",
                            "text": "Native file-only edit.\n",
                            "revision": None,
                        },
                    }
                },
                {"params": {"tool": "exec_command", "arguments": {"cmd": "touch forbidden"}}},
            ]
        )
    )
    environment = dict(
        os.environ,
        XDG_CONFIG_HOME=str(base / "config"),
        XDG_DATA_HOME=str(base / "data"),
        RODE_SESSION_DIR=str(base / "runtime"),
    )
    saved = base / "data/rode/app.ron"
    log_path = ARTIFACTS / "file-only.log"
    with log_path.open("w") as log:
        process = subprocess.Popen(
            [str(BINARY), "--sandbox", "--theme", "carbon", str(root)],
            env=environment,
            stdout=log,
            stderr=log,
        )
        window = None
        try:
            window = wait_for_window(process, log_path)
            wait_for_project(window, process, log_path, root)
            focus_window(window)
            key("a", ("Control_L", "Shift_L"))
            state = subprocess.check_output(["xwininfo", "-id", hex(window)], text=True)
            header = 157 if "[Sandbox]" in state else 187
            click(1213, header)
            click(1110, header + 94)
            key("a", ("Control_L",))
            text("./agent.py")
            wait(
                lambda: preferences(saved).get("codex") == "./agent.py",
                "Agent executable was not saved",
            )
            click(1213, header)
            click(1280, 752)
            click(1090, 505)  # File-only row in the ordinary access menu.
            wait(
                lambda: preferences(saved).get("access") == "files_only",
                "File-only mode was not saved",
            )
            click(1280, 752)
            screenshot(window, "file-only-permissions.png")
            key("Escape")
            assert not (root / "file-agent-rpc.jsonl").exists(), "Selecting access started an agent"
            click(1280, 785)
            text("Edit project files without commands.")
            key("Return")
            wait(
                (root / "ui-created.txt").exists, "Native file tool did not create the project file"
            )

            def declined():
                path = root / "file-agent-rpc.jsonl"
                if not path.exists():
                    return False
                return any(
                    call.get("id") == 101 and call.get("result", {}).get("success") is False
                    for call in (
                        json.loads(line)
                        for line in path.read_text().splitlines(keepends=True)
                        if line.endswith("\n")
                    )
                )

            wait(declined, "The execution tool request was not declined")
            assert not (root / "forbidden").exists()
            assert (root / "ui-created.txt").read_text() == "Native file-only edit.\n"
            screenshot(window, "file-only-complete.png")
            x11.XResizeWindow(display, window, 760, 900)
            x11.XFlush(display)
            time.sleep(0.4)
            screenshot(window, "file-only-compact.png")
            close_window(window, process)
        except Exception:
            if window and process.poll() is None:
                screenshot(window, "failure.png")
            raise
        finally:
            if process.poll() is None:
                process.terminate()
                process.wait(timeout=5)
print("Native file-only selection, saved access, project edits and command denial passed.")
