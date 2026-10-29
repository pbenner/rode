#!/usr/bin/env python3
"""Native agent chat regression checks using a deterministic peer, without inference."""

import ctypes
import json
import os
import re
import shutil
import subprocess
import tempfile
import time
from pathlib import Path

os.environ["RODE_UI_ARTIFACTS"] = str(Path(__file__).resolve().parent.parent / "target/agent-ui")
from ui_harness import (
    ARTIFACTS,
    BINARY,
    ROOT,
    click,
    close_window,
    display,
    key,
    screenshot,
    text,
    theme,
    x11,
)

x11.XResizeWindow.argtypes = [ctypes.c_void_p, ctypes.c_ulong, ctypes.c_uint, ctypes.c_uint]


def wait_for(check, message):
    deadline = time.monotonic() + 10
    while time.monotonic() < deadline:
        if check():
            return
        time.sleep(0.05)
    raise AssertionError(message)


def window_for(process):
    for _ in range(150):
        tree = subprocess.check_output(["xwininfo", "-root", "-tree"], text=True)
        match = re.search(r'(0x[0-9a-f]+) "RODE', tree)
        if match:
            window = int(match[1], 16)
            time.sleep(0.8)
            x11.XSetInputFocus(display, window, 1, 0)
            x11.XFlush(display)
            return window
        assert process.poll() is None, "RODE exited before opening a window"
        time.sleep(0.05)
    raise AssertionError("No RODE window")


def rpc(root):
    path = root / "agent-rpc.jsonl"
    return [json.loads(line) for line in path.read_text().splitlines()] if path.exists() else []


def sent(root, message):
    return any(
        m.get("method") in ("turn/start", "session/prompt")
        and m["params"].get("input", m["params"].get("prompt"))[0]["text"] == message
        for m in rpc(root)
    )


def send(root, message, first=False):
    click(1280, 785)
    text(message)
    if first:
        # An ordinary newline stays in the composer and does not start a process.
        key("Return")
        assert not (root / "agent-rpc.jsonl").exists()
        key("BackSpace")
    key("Return", ("Control_L",))
    wait_for(lambda: sent(root, message), "Composer did not submit: " + message)
    time.sleep(0.3)


with tempfile.TemporaryDirectory(prefix="rode-agent-ui-") as directory:
    base = Path(directory)
    root = base / "project"
    root.mkdir()
    (base / "private.txt").write_text("outside the project")
    source = root / "main.py"
    source.write_text("# original on disk\n")
    shutil.copy2(ROOT / "tests/fixtures/agent.py", root / "agent.py")
    (root / "agent.py").chmod(0o755)
    environment = dict(
        os.environ, XDG_CONFIG_HOME=str(base / "config"), XDG_DATA_HOME=str(base / "data")
    )
    preferences = base / "data/rode/app.ron"
    with (ARTIFACTS / "chat.log").open("w") as log:
        process = subprocess.Popen(
            [str(BINARY), "--sandbox", str(root)], env=environment, stdout=log, stderr=log
        )
        window = None
        try:
            window = window_for(process)
            key("a", ("Control_L", "Shift_L"))
            time.sleep(0.3)
            assert not (root / "agent-rpc.jsonl").exists(), "Opening chat started a process"
            click(1229, 183)  # Settings in the default-width chat panel.
            click(1110, 277)
            key("a", ("Control_L",))
            text("./agent.py")
            wait_for(
                lambda: preferences.exists() and "./agent.py" in preferences.read_text(),
                "Agent executable preference was not saved immediately",
            )
            click(1229, 183)
            send(root, "first", first=True)
            scope = json.loads((root / "agent-scope.json").read_text())
            assert scope["cwd"] == str(root)
            info = subprocess.check_output(["xwininfo", "-id", hex(window)], text=True)
            if "[Sandbox]" in info:
                assert not scope["private_visible"]
            send(root, "second")
            assert sum(m.get("method") == "initialize" for m in rpc(root)) == 1
            screenshot(window, "conversation-phosphor.png")
            theme("carbon")
            screenshot(window, "conversation-carbon.png")
            send(root, "approve")
            assert not (root / "approved.txt").exists()
            screenshot(window, "permission.png")
            click(1047, 371)  # Allow once, after the visible command and Details.
            wait_for(lambda: (root / "approved.txt").exists(), "Approval was not delivered")
            time.sleep(0.3)
            send(root, "wait")
            wait_for(lambda: (root / "agent-child.pid").exists(), "Long task did not start")
            click(1070, 840)  # Stop, without interrupting the project terminal.
            wait_for(
                lambda: any(m.get("method") == "turn/interrupt" for m in rpc(root)),
                "Stop did not interrupt the turn",
            )
            time.sleep(0.3)
            # Open and edit a file without saving. Agent edits must not replace that buffer.
            key("p", ("Control_L", "Shift_L"))
            text("Open file by path")
            key("Return")
            time.sleep(0.2)
            text("main.py")
            key("Return")
            time.sleep(0.3)
            key("End", ("Control_L",))
            text("# keep my unsaved change\n")
            send(root, "rewrite")
            wait_for(lambda: source.read_text() == "# written by the agent\n", "Agent did not edit")
            screenshot(window, "unsaved-buffer.png")
            click(500, 220)
            key("s", ("Control_L", "Shift_L"))
            time.sleep(0.2)
            key("a", ("Control_L",))
            text("preserved.txt")
            key("Return")
            preserved = root / "preserved.txt"
            wait_for(preserved.exists, "Could not save the preserved user buffer")
            assert "keep my unsaved change" in preserved.read_text()
            assert "written by the agent" not in preserved.read_text()
            # Minimum window keeps the composer and Send button accessible.
            x11.XResizeWindow(display, window, 850, 560)
            x11.XFlush(display)
            time.sleep(0.4)
            screenshot(window, "minimum-chat.png")
            x11.XResizeWindow(display, window, 1380, 900)
            x11.XFlush(display)
            time.sleep(0.3)
            close_window(window, process)
            # A native restart restores executable settings, but not the conversation.
            before = sum(m.get("method") == "initialize" for m in rpc(root))
            process = subprocess.Popen(
                [str(BINARY), "--sandbox", str(root)], env=environment, stdout=log, stderr=log
            )
            window = window_for(process)
            key("a", ("Control_L", "Shift_L"))
            time.sleep(0.3)
            send(root, "restored")
            assert sum(m.get("method") == "initialize" for m in rpc(root)) == before + 1
            assert "keep my unsaved change" not in preferences.read_text()
            click(1150, 183)
            click(1160, 251)  # Local model provider opens its settings.
            click(1280, 376)
            text("fixture-local-model")
            wait_for(
                lambda: "fixture-local-model" in preferences.read_text(), "Model was not saved"
            )
            screenshot(window, "local-model-settings.png")
            click(1229, 183)
            send(root, "local-provider")
            scope = json.loads((root / "agent-scope.json").read_text())
            assert 'model_provider="rode_local"' in scope["argv"]
            x11.XResizeWindow(display, window, 850, 560)
            x11.XFlush(display)
            time.sleep(0.4)
            screenshot(window, "minimum-local-chat.png")
            x11.XResizeWindow(display, window, 1380, 900)
            x11.XFlush(display)
            time.sleep(0.3)
            click(1150, 183)
            click(1160, 285)  # ACP, with the same deterministic executable.
            click(1280, 277)
            text("./agent.py")
            click(1229, 183)
            send(root, "acp-provider")
            assert any(m.get("method") == "session/new" for m in rpc(root))
            x11.XResizeWindow(display, window, 850, 560)
            x11.XFlush(display)
            time.sleep(0.4)
            screenshot(window, "minimum-acp-chat.png")
            before = sum(m.get("method") == "initialize" for m in rpc(root))
            click(760, 183)  # New chat remains accessible beside the long provider label.
            x11.XResizeWindow(display, window, 1380, 900)
            x11.XFlush(display)
            time.sleep(0.3)
            send(root, "new-acp-conversation")
            assert sum(m.get("method") == "initialize" for m in rpc(root)) == before + 1
            close_window(window, process)
            print(
                "Agent UI: chat, settings persistence, context, approvals, Stop, themes, and unsaved buffers passed."
            )
        except Exception:
            if window is not None and process.poll() is None:
                screenshot(window, "failure.png")
            raise
        finally:
            if process.poll() is None:
                process.terminate()
                process.wait(timeout=5)
