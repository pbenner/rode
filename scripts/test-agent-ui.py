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


def agent_preferences(path):
    """Read the fixture's one workspace profile from the shared RON string map."""
    match = re.search(r'"agent_preferences"\s*:\s*("(?:\\.|[^"\\])*")', path.read_text())
    assert match, "No saved agent settings"
    return next(iter(json.loads(json.loads(match[1])).values()))


def sent(root, message):
    return any(
        m.get("method") in ("turn/start", "session/prompt")
        and m["params"].get("input", m["params"].get("prompt"))[0]["text"] == message
        for m in rpc(root)
    )


def send(root, message, first=False):
    click(1280, 785)
    for index, line in enumerate(message.split("\n")):
        if index:
            key("Return", ("Shift_L",))
        text(line)
    if first:
        # Shift+Enter stays in the composer and does not start a process.
        key("Return", ("Shift_L",))
        assert not (root / "agent-rpc.jsonl").exists()
        key("BackSpace")
    key("Return")
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
            click(1213, 183)  # Settings in the default-width chat panel.
            click(1110, 277)
            key("a", ("Control_L",))
            text("./agent.py")
            wait_for(
                lambda: preferences.exists() and "./agent.py" in preferences.read_text(),
                "Agent executable preference was not saved immediately",
            )
            click(1213, 183)
            click(1346, 183)  # Tone menu: choices alone must not start an agent.
            screenshot(window, "personality-menu.png")
            click(1180, 248)
            wait_for(
                lambda: agent_preferences(preferences).get("personality") == "jackson_lamb",
                "Personality choice was not saved",
            )
            assert not (root / "agent-rpc.jsonl").exists()
            click(1346, 183)
            click(1180, 229)  # Default before the first turn leaves the task unmodified.
            wait_for(
                lambda: agent_preferences(preferences).get("personality") == "default",
                "Default personality was not saved",
            )
            send(root, "first\nsecond line", first=True)
            scope = json.loads((root / "agent-scope.json").read_text())
            assert scope["cwd"] == str(root)
            info = subprocess.check_output(["xwininfo", "-id", hex(window)], text=True)
            if "[Sandbox]" in info:
                assert not scope["private_visible"]
            click(1346, 183)
            click(1180, 248)
            send(root, "second")
            styled = next(m for m in reversed(rpc(root)) if m.get("method") == "turn/start")
            assert styled["params"]["input"][0]["text"] == "second"
            assert 'name="jackson_lamb"' in styled["params"]["input"][1]["text"]
            assert sum(m.get("method") == "initialize" for m in rpc(root)) == 1
            screenshot(window, "conversation-phosphor.png")
            theme("carbon")
            screenshot(window, "conversation-carbon.png")
            click(1090, 752)
            screenshot(window, "codex-model-controls.png")
            click(1090, 633)  # Advertised Fixture Deep model, not a hard-coded product catalog.
            wait_for(
                lambda: agent_preferences(preferences).get("model") == "fixture-deep",
                "Selected model was not persisted",
            )
            key("Escape")
            calls_before_effort = len(rpc(root))
            for effort, row in (("high", 744), ("low", 719), ("high", 744)):
                click(1090, 752)
                click(1075, 693)
                screenshot(window, f"codex-effort-{effort}.png")
                click(960, row)
                wait_for(
                    lambda: agent_preferences(preferences).get("reasoning_effort") == effort,
                    "Reasoning selector did not save " + effort,
                )
                key("Escape")
            assert len(rpc(root)) == calls_before_effort, "Effort selection started a turn"
            click(1280, 752)
            screenshot(window, "codex-access-controls.png")
            key("Escape")
            send(root, "approve")
            selected_turn = next(m for m in reversed(rpc(root)) if m.get("method") == "turn/start")
            assert selected_turn["params"]["effort"] == "high"
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
            selected_turn = next(m for m in reversed(rpc(root)) if m.get("method") == "turn/start")
            assert selected_turn["params"]["effort"] == "high"
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
            click(1213, 183)
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
            click(1213, 183)
            send(root, "acp-provider")
            assert any(m.get("method") == "session/new" for m in rpc(root))
            calls_before_tone = len(rpc(root))
            click(1346, 183)
            click(1180, 229)
            wait_for(
                lambda: agent_preferences(preferences).get("personality") == "default",
                "ACP Default personality was not saved",
            )
            assert len(rpc(root)) == calls_before_tone, "Tone configured the idle ACP agent"
            click(1346, 183)
            click(1180, 248)
            wait_for(
                lambda: agent_preferences(preferences).get("personality") == "jackson_lamb",
                "ACP personality was not saved",
            )
            assert len(rpc(root)) == calls_before_tone
            click(1180, 752)
            screenshot(window, "acp-model-controls.png")
            click(1000, 645)
            screenshot(window, "acp-model-submenu.png")
            click(750, 670)
            wait_for(
                lambda: (
                    agent_preferences(preferences).get("acp_options", {}).get("model")
                    == "fixture-deep"
                ),
                "ACP model submenu did not save its selection",
            )
            key("Escape")
            click(1180, 752)
            click(1000, 693)
            screenshot(window, "acp-effort-submenu.png")
            click(750, 718)
            wait_for(
                lambda: (
                    agent_preferences(preferences).get("acp_options", {}).get("effort") == "high"
                ),
                "ACP reasoning submenu did not save its selection",
            )
            wait_for(
                lambda: any(
                    m.get("method") == "session/set_config_option"
                    and m["params"].get("configId") == "effort"
                    and m["params"].get("value") == "high"
                    for m in rpc(root)
                ),
                "ACP reasoning selection did not reach the agent",
            )
            key("Escape")
            click(1300, 752)
            screenshot(window, "acp-permission-controls.png")
            click(1007, 557)  # An agent-advertised boolean, applied without another message.
            wait_for(
                lambda: (
                    agent_preferences(preferences).get("acp_options", {}).get("automatic") is True
                ),
                "ACP option was not saved",
            )
            wait_for(
                lambda: any(
                    m.get("method") == "session/set_config_option"
                    and m["params"].get("configId") == "automatic"
                    and m["params"].get("type") == "boolean"
                    for m in rpc(root)
                ),
                "ACP option was not sent to the running agent",
            )
            key("Escape")
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
            # The native permission selectors persist choices and affect the
            # next Codex turn while the existing outer sandbox remains active.
            click(1150, 183)
            click(1160, 218)
            click(1213, 183)  # Hide the automatically opened provider settings.
            click(1280, 752)
            click(1090, 645)  # Never ask.
            wait_for(
                lambda: agent_preferences(preferences).get("approval") == "never",
                "Approval policy was not persisted",
            )
            click(1280, 752)  # Selecting a policy closes the native menu.
            click(1090, 557)  # Full access inside the active project sandbox.
            wait_for(
                lambda: agent_preferences(preferences).get("access") == "full",
                "Tool access was not persisted",
            )
            screenshot(window, "selected-access-controls.png")
            key("Escape")
            (root / "approved.txt").unlink(missing_ok=True)
            send(root, "approve")
            wait_for(
                lambda: any(
                    m.get("id") == 400 and m.get("result", {}).get("decision") == "decline"
                    for m in rpc(root)
                ),
                "Never-ask policy did not decline the tool request",
            )
            assert not (root / "approved.txt").exists()
            last = next(m for m in reversed(rpc(root)) if m.get("method") == "turn/start")
            assert last["params"]["model"] == "fixture-deep"
            assert last["params"]["approvalPolicy"] == "never"
            if "[Sandbox]" in subprocess.check_output(["xwininfo", "-id", hex(window)], text=True):
                assert last["params"]["sandboxPolicy"]["type"] == "externalSandbox"
            # History is explicit host work; opening it must not send a model prompt.
            prompts_before = sum(m.get("method") == "turn/start" for m in rpc(root))
            click(1340, 257)
            wait_for(
                lambda: any(m.get("method") == "thread/list" for m in rpc(root)),
                "History did not list saved conversations",
            )
            time.sleep(0.3)
            screenshot(window, "history-list.png")
            assert sum(m.get("method") == "turn/start" for m in rpc(root)) == prompts_before
            x11.XResizeWindow(display, window, 850, 560)
            x11.XFlush(display)
            time.sleep(0.3)
            screenshot(window, "minimum-history.png")
            x11.XResizeWindow(display, window, 1380, 900)
            x11.XFlush(display)
            time.sleep(0.3)
            click(1200, 425)  # Load older metadata without prompting the model.
            wait_for(
                lambda: sum(m.get("method") == "thread/list" for m in rpc(root)) == 2,
                "History did not load another page",
            )
            click(1200, 308)
            text("saved-2")
            time.sleep(0.2)
            screenshot(window, "history-filtered.png")
            click(1200, 363)  # The filter exposes the older conversation first.
            wait_for(
                lambda: any(m.get("method") == "thread/resume" for m in rpc(root)),
                "History did not restore the selected conversation",
            )
            time.sleep(0.3)
            screenshot(window, "history-restored.png")
            send(root, "continue-saved-work")
            last = next(m for m in reversed(rpc(root)) if m.get("method") == "turn/start")
            assert last["params"]["threadId"] == "saved-2"
            close_window(window, process)
            print(
                "Agent UI: Enter/Shift+Enter, personalities, history restore, model/access selectors, saved policies, chat context, approvals, Stop, themes, and unsaved buffers passed."
            )
        except Exception:
            if window is not None and process.poll() is None:
                screenshot(window, "failure.png")
            raise
        finally:
            if process.poll() is None:
                process.terminate()
                process.wait(timeout=5)
