#!/usr/bin/env python3
"""Delete saved chats through native controls using only isolated backend fixtures."""

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
    focus_window,
    key,
    screenshot,
    text,
)


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
    (root / "history-fixture.json").write_text(json.dumps(dict(archived=True)))
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
            [str(BINARY), "--sandbox", str(root)], env=env, stdout=log, stderr=log
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
            click(1340, header + 180)
            time.sleep(0.2)
            screenshot(window, "confirm-one.png")
            assert not deleted(), "Opening deletion confirmation removed a chat"
            click(370, header + 219)
            time.sleep(0.2)
            assert not deleted(), "Cancelling deletion removed a chat"
            click(1340, header + 180)
            time.sleep(0.2)
            click(505, header + 223)
            wait(lambda: deleted() == ["saved-1"], "Individual chat deletion was not acknowledged")
            screenshot(window, "deleted-one.png")
            # A filter does not turn Delete all into Delete visible rows.
            click(1200, header + 125)
            text("no matching chats")
            click(580, header + 105)
            time.sleep(0.2)
            screenshot(window, "confirm-all.png")
            click(505, header + 240)
            wait(
                lambda: deleted() == ["archived-1", "saved-1", "saved-2"],
                "Delete all missed unloaded or archived chats",
            )
            time.sleep(0.3)
            screenshot(window, "deleted-all.png")
            assert all(
                c.get("method") not in ("thread/start", "thread/resume", "turn/start")
                for c in calls()
            ), "History deletion started inference or resumed a chat"
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
    "History deletion: individual confirmation/cancel, unloaded and archived bulk removal, and no inference passed"
)
