#!/usr/bin/env python3
"""Delete saved chats through native controls using only isolated backend fixtures."""

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
    Path(__file__).resolve().parent.parent / "target/agent-history-ui"
)
from ui_harness import (
    ARTIFACTS,
    BINARY,
    ROOT,
    assert_running,
    click,
    close_window,
    display,
    focus_window,
    key,
    screenshot,
    text,
    x11,
)

x11.XResizeWindow.argtypes = [ctypes.c_void_p, ctypes.c_ulong, ctypes.c_uint, ctypes.c_uint]


def resize(window, width, height):
    x11.XResizeWindow(display, window, width, height)
    x11.XFlush(display)
    time.sleep(0.5)


def wait(check, message):
    deadline = time.monotonic() + 15
    while time.monotonic() < deadline:
        if result := check():
            return result
        time.sleep(0.05)
    raise AssertionError(message)


with tempfile.TemporaryDirectory(prefix="rode-agent-history-ui-") as directory:
    base = Path(directory)
    root = base / "project"
    root.mkdir()
    shutil.copy2(ROOT / "tests/fixtures/agent.py", root / "agent.py")
    (root / "agent.py").chmod(0o755)
    history = dict(
        archived=True,
        count=4,
        active=["saved-3"],
        titles={
            "saved-0": "Can we discuss the implemented Slurm support in rode?",
            "saved-1": "I get this message on another machine, although I have clicked on install codex button\n\nI couldn’t access the repository: the command runner fails because this file is missing.",
            "saved-2": "Notebook kernels and debugger reattachment",
            "saved-3": "Training run on the GPU cluster",
        },
        previews={
            "saved-0": "Persistent notebook kernels, compute targets and debugger reattachment.",
            "saved-1": "I get this message on another machine, although I have clicked on install codex button\n\nI couldn’t access the repository: the command runner fails because this file is missing:\n`/home/test/.local/share/rode/codex/0.166.0/x86_64/codex-code-node-host`\nNo files were changed.",
            "saved-2": "Variables and breakpoints survive closing the desktop and reconnecting over SSH.",
            "saved-3": "Checking the running allocation and reviewing the latest logs.",
        },
        updated={
            "saved-0": int(time.time()) - 420,
            "saved-1": int(time.time()) - 39600,
            "saved-2": int(time.time()) - 86400,
            "saved-3": int(time.time()) - 120,
        },
    )
    (root / "history-fixture.json").write_text(json.dumps(history))
    env = dict(
        os.environ,
        XDG_CONFIG_HOME=str(base / "config"),
        XDG_DATA_HOME=str(base / "data"),
        RODE_SESSION_DIR=str(base / "runtime"),
    )

    def calls():
        path = root / "agent-rpc.jsonl"
        return [json.loads(line) for line in path.read_text().splitlines()] if path.exists() else []

    def deleted():
        path = root / "history-deleted.json"
        return json.loads(path.read_text()) if path.exists() else []

    log_path = ARTIFACTS / "history.log"
    window = None
    with log_path.open("w") as log:
        process = subprocess.Popen(
            [str(BINARY), "--theme", "carbon", "--sandbox", str(root)],
            env=env,
            stdout=log,
            stderr=log,
        )
        try:

            def mapped():
                assert_running(process, log_path)
                tree = subprocess.check_output(["xwininfo", "-root", "-tree"], text=True)
                match = re.search(r'(0x[0-9a-f]+) "RODE.*project', tree)
                return int(match[1], 16) if match and "[Opening project]" not in match[0] else None

            window = wait(mapped, "No prepared project window")
            time.sleep(0.8)
            focus_window(window)
            info = subprocess.check_output(["xwininfo", "-id", hex(window)], text=True)
            header = 157 if "[Sandbox]" in info else 187
            if "[Sandbox]" not in info:
                click(1331, 820)
            key("a", ("Control_L", "Shift_L"))
            time.sleep(0.4)
            click(1213, header)
            click(1110, header + 94)
            key("a", ("Control_L",))
            text("./agent.py")
            click(1213, header)
            click(1340, header + 74)
            wait(
                lambda: any(c.get("method") == "thread/list" for c in calls()),
                "History did not open",
            )
            time.sleep(0.4)
            screenshot(window, "list.png")
            # Long multiline backend titles must stay compact, including in a
            # narrow tool panel where the toolbar moves onto its own line.
            resize(window, 760, 900)
            screenshot(window, "compact.png")
            resize(window, 1380, 900)
            click(1275, header + 279)
            time.sleep(0.2)
            screenshot(window, "confirm-one.png")
            assert not deleted(), "Opening deletion confirmation removed a chat"
            click(420, header + 330)
            time.sleep(0.2)
            assert not deleted(), "Cancelling deletion removed a chat"
            click(1275, header + 279)
            time.sleep(0.2)
            click(550, header + 330)
            wait(lambda: deleted() == ["saved-0"], "Individual chat deletion was not acknowledged")
            time.sleep(0.3)
            screenshot(window, "deleted-one.png")
            # A filter does not turn Delete all into Delete visible rows. Grow
            # the backend snapshot past one page after the initial four-row view.
            history["count"] = 40
            (root / "history-fixture.json").write_text(json.dumps(history))
            click(700, header + 179)
            text("no matching chats")
            screenshot(window, "filtered-empty.png")
            click(1280, header + 113)
            time.sleep(0.2)
            screenshot(window, "confirm-all.png")
            click(550, header + 330)
            expected = sorted(["archived-1"] + [f"saved-{n}" for n in range(40) if n != 3])
            wait(
                lambda: deleted() == expected,
                "Delete all missed unloaded, archived or protected chats",
            )
            time.sleep(0.3)
            screenshot(window, "deleted-all.png")
            assert all(
                c.get("method") not in ("thread/start", "thread/resume", "turn/start")
                for c in calls()
            ), "History deletion started inference or resumed a chat"
            # Refresh the protected chat as idle and resume it with the primary
            # card target; the adjacent Delete button must never resume a chat.
            history["active"] = []
            (root / "history-fixture.json").write_text(json.dumps(history))
            click(700, header + 179)
            key("a", ("Control_L",))
            key("BackSpace")
            listed_before = sum(c.get("method") == "thread/list" for c in calls())
            click(1190, header + 113)
            wait(
                lambda: sum(c.get("method") == "thread/list" for c in calls()) > listed_before,
                "History did not refresh",
            )
            time.sleep(0.5)
            click(600, header + 264)
            wait(
                lambda: any(
                    c.get("method") == "thread/resume" and c["params"]["threadId"] == "saved-3"
                    for c in calls()
                ),
                "Conversation card did not resume the selected chat",
            )
            time.sleep(0.5)
            screenshot(window, "resumed.png")
            assert all(c.get("method") not in ("thread/start", "turn/start") for c in calls()), (
                "Restoring history started inference"
            )
            history["refresh_error"] = True
            (root / "history-fixture.json").write_text(json.dumps(history))
            listed_before = sum(c.get("method") == "thread/list" for c in calls())
            click(1340, header + 74)
            wait(
                lambda: sum(c.get("method") == "thread/list" for c in calls()) > listed_before,
                "History did not show the failed backend refresh",
            )
            time.sleep(0.4)
            screenshot(window, "backend-error.png")
            close_window(window, process)
        except Exception:
            if window and process.poll() is None:
                screenshot(window, "failure.png")
            raise
        finally:
            if process.poll() is None:
                process.terminate()
                process.wait(timeout=5)
print(
    "History layout: long titles, compact panels, filter, protected bulk deletion and card resume passed"
)
