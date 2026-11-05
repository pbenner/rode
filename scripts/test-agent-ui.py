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
    click_button,
    close_window,
    display,
    focus_window,
    key,
    screenshot,
    text,
    theme,
    wait_for_button,
    x11,
    xtest,
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
            focus_window(window)

            # The project banner moves the chat header. Wait for preparation
            # before clicking its controls, especially during parallel builds.
            def project_ready():
                info = subprocess.check_output(["xwininfo", "-id", hex(window)], text=True)
                return "project" in info and "[Opening project]" not in info

            wait_for(project_ready, "Project did not finish preparing before chat input")
            if "[Sandbox]" not in subprocess.check_output(
                ["xwininfo", "-id", hex(window)], text=True
            ):
                click(1331, 820)  # Dismiss this host's sandbox-unavailable notification.
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


def submitted(root):
    """Read accepted user prompts independently of the window's recalled draft."""
    return [
        m["params"].get("input", m["params"].get("prompt"))[0]["text"]
        for m in rpc(root)
        if m.get("method") in ("turn/start", "session/prompt")
    ]


def control(name, **guards):
    return wait_for_button(
        window, process, ARTIFACTS / "chat.log", base / "ui-state.json", name, **guards
    )


def rendered_pass():
    return json.loads((base / "ui-state.json").read_text())["pass"]


def composer_start():
    """Wait for the native editor to place an unselected caret at character zero."""
    before = rendered_pass()
    key("Home", ("Control_L",))
    control("agent-composer-at-start", focused=True, after_pass=before)


def focus_composer():
    show_tools()
    click(*control("agent-composer"))
    control("agent-composer", focused=True)


def browse(prompts, direction="Up"):
    """Verify each real recall transition before the next dependent key press."""
    for expected in prompts:
        composer_start()
        before = rendered_pass()
        key(direction)
        control("agent-composer", focused=True, changed_after=before)
        key("a", ("Control_L",))
        key("c", ("Control_L",))
        copied_text(expected, f"recall-{direction.lower()}-{before}")


def submit(root, message, idle=True):
    # Count this specific request: repeated prompts must not match an old turn.
    previous = submitted(root).count(message)
    key("Return")
    wait_for(
        lambda: submitted(root).count(message) > previous,
        "Composer did not submit: " + message,
    )
    if idle:
        # A logged backend request precedes UI history/completion acknowledgement.
        # Demand a newer idle frame before navigating its newly accepted history.
        before = rendered_pass()
        key("Home", ("Control_L",))
        control("agent-idle", focused=True, after_pass=before)


def show_tools():
    key("space", ("Control_L",))
    key("p")


def header_click(x, offset):
    info = subprocess.check_output(["xwininfo", "-id", hex(window)], text=True)
    # The sandbox-status strip and wrapping workspace tabs determine the header.
    y = 157 if "[Sandbox]" in info else 187
    if int(re.search(r"Width: (\d+)", info)[1]) < 1000:
        y += 26
    click(x, y + offset)


def select_provider(preferences, provider):
    """Select a known native menu row and wait for the saved provider identity."""
    offsets = {"codex": 35, "claude": 68, "local": 102, "acp": 136}
    header_click(1150, 0)
    header_click(340, offsets[provider])
    wait_for(
        lambda: agent_preferences(preferences).get("provider") == provider,
        f"Provider {provider!r} was not selected",
    )


def select_personality(preferences, value, capture=False):
    """Click the centre of a tone-menu row and verify the persisted selection."""
    # Offsets are relative to the chat header. The native menu's rows are
    # 25 px apart; clicking their gap closes the menu without changing tone.
    row_offsets = {"default": 51, "jackson_lamb": 76}
    header_click(1346, 0)
    if capture:
        screenshot(window, "personality-menu.png")
    header_click(1180, row_offsets[value])
    wait_for(
        lambda: agent_preferences(preferences).get("personality") == value,
        f"Personality choice {value!r} was not saved",
    )


def send(root, message, first=False, idle=True):
    focus_composer()
    for index, line in enumerate(message.split("\n")):
        if index:
            key("Return", ("Shift_L",))
        text(line)
    if first:
        # Shift+Enter stays in the composer and does not start a process.
        key("Return", ("Shift_L",))
        assert not any(m.get("method") in ("turn/start", "session/prompt") for m in rpc(root))
        key("BackSpace")
    submit(root, message, idle=idle)


def copied_text(expected, name):
    """Observe the native clipboard after Copy; never populate the application buffer."""
    deadline = time.monotonic() + 10
    actual = ""
    while time.monotonic() < deadline:
        try:
            result = subprocess.run(
                ["xclip", "-selection", "clipboard", "-o"],
                capture_output=True,
                text=True,
                encoding="utf-8",
                timeout=1,
            )
        except subprocess.TimeoutExpired:
            continue
        if result.returncode == 0:
            actual = result.stdout
            if actual == expected:
                return
        time.sleep(0.05)
    text_matches(actual, expected, name)
    raise AssertionError(f"Clipboard did not provide {name} within 10 seconds")


def text_matches(actual, expected, name):
    """Keep exact fixture contents and a bounded mismatch diagnostic in CI artifacts."""
    if actual == expected:
        return
    (ARTIFACTS / f"{name}-actual.txt").write_text(actual)
    (ARTIFACTS / f"{name}-expected.txt").write_text(expected)
    different = next(
        (index for index, (left, right) in enumerate(zip(actual, expected)) if left != right),
        min(len(actual), len(expected)),
    )
    raise AssertionError(
        f"{name}: expected {len(expected)} characters, got {len(actual)}; "
        f"first difference at {different}: "
        f"{actual[different : different + 80]!r} != {expected[different : different + 80]!r}. "
        f"Full contents: {ARTIFACTS / (name + '-actual.txt')}"
    )


def long_composer(root, window, process, state_path, prompt):
    """Exercise real copy/paste, scrolling, resize and save with input-readiness barriers."""

    def control(name, **guards):
        return wait_for_button(window, process, ARTIFACTS / "chat.log", state_path, name, **guards)

    def paste(name):
        # Focus alone can already be true before Paste reaches the native frame.
        # Wait for this editor's next actual edit before issuing Save As/Copy.
        before = json.loads(state_path.read_text())["pass"]
        key("v", ("Control_L",))
        control(name, focused=True, changed_after=before)

    key("p", ("Control_L",))
    control("file-dialog-path", focused=True)
    text("prompt.txt")
    key("Return")
    control("source-editor", focused=True)
    key("a", ("Control_L",))
    key("c", ("Control_L",))
    copied_text(prompt, "long-composer-source-copy")
    show_tools()
    click(*control("agent-composer"))
    control("agent-composer", focused=True)
    paste("agent-composer")
    screenshot(window, "long-composer-end.png")

    xtest.XTestFakeMotionEvent(display, -1, *control("agent-composer"), 0)
    for _ in range(24):
        xtest.XTestFakeButtonEvent(display, 4, 1, 0)
        xtest.XTestFakeButtonEvent(display, 4, 0, 0)
    x11.XFlush(display)
    time.sleep(0.3)
    screenshot(window, "long-composer-scrolled.png")

    # Drag the divider upward; releasing it must return typing to the draft.
    divider_x, divider_y = control("agent-composer-resize")
    xtest.XTestFakeMotionEvent(display, -1, divider_x, divider_y, 0)
    xtest.XTestFakeButtonEvent(display, 1, 1, 0)
    x11.XFlush(display)
    time.sleep(0.1)
    for step in range(1, 11):
        xtest.XTestFakeMotionEvent(display, -1, divider_x, divider_y - 20 * step, 0)
        x11.XFlush(display)
        time.sleep(0.03)
    xtest.XTestFakeButtonEvent(display, 1, 0, 0)
    x11.XFlush(display)
    wait_for(
        lambda: control("agent-composer-resize")[1] <= divider_y - 180,
        "Dragging did not expand the message input",
    )
    control("agent-composer", focused=True)
    key("End", ("Control_L",))
    text(" visible-tail")
    screenshot(window, "long-composer-resized.png")
    key("a", ("Control_L",))
    key("c", ("Control_L",))
    copied_text(prompt + " visible-tail", "long-composer-draft-copy")
    xtest.XTestFakeMotionEvent(display, -1, *control("agent-composer-resize"), 0)
    for _ in range(2):
        xtest.XTestFakeButtonEvent(display, 1, 1, 0)
        xtest.XTestFakeButtonEvent(display, 1, 0, 0)
        x11.XFlush(display)
        time.sleep(0.05)
    wait_for(
        lambda: abs(control("agent-composer-resize")[1] - divider_y) <= 2,
        "Double-click did not reset the message input height",
    )
    screenshot(window, "long-composer-reset.png")
    # Inspect what was retained through resize/scroll using the editor's ordinary
    # save path. Readiness metadata contains no draft/document text.
    key("n", ("Control_L",))
    control("source-editor", focused=True)
    paste("source-editor")
    key("s", ("Control_L", "Shift_L"))
    control("file-dialog-path", focused=True)
    key("a", ("Control_L",))
    text("composed.txt")
    key("Return")
    wait_for(lambda: (root / "composed.txt").exists(), "Could not inspect the long chat draft")
    text_matches(
        (root / "composed.txt").read_text(), prompt + " visible-tail", "long-composer-save"
    )
    assert (root / "prompt.txt").read_text() == prompt
    assert not (root / "agent-rpc.jsonl").exists(), "Composing or resizing started an agent"
    show_tools()
    click(1280, 785)
    key("a", ("Control_L",))
    key("BackSpace")


with tempfile.TemporaryDirectory(prefix="rode-agent-ui-") as directory:
    base = Path(directory)
    root = base / "project"
    root.mkdir()
    (base / "private.txt").write_text("outside the project")
    source = root / "main.py"
    source.write_text("# original on disk\n")
    prompt = (
        "\n".join(
            f"Line {i:02}: a long message with λ and 🦀 and wrapping text " * 2 for i in range(60)
        )
        + "\nFINAL_LINE"
    )
    (root / "prompt.txt").write_text(prompt)
    shutil.copy2(ROOT / "tests/fixtures/agent.py", root / "agent.py")
    (root / "agent.py").chmod(0o755)
    environment = dict(
        os.environ,
        XDG_CONFIG_HOME=str(base / "config"),
        XDG_DATA_HOME=str(base / "data"),
        RODE_UI_TEST_STATE=str(base / "ui-state.json"),
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
            header_click(1213, 0)  # Settings in the default-width chat panel.
            header_click(1110, 94)
            key("a", ("Control_L",))
            text("./agent.py")
            wait_for(
                lambda: preferences.exists() and "./agent.py" in preferences.read_text(),
                "Agent executable preference was not saved immediately",
            )
            header_click(1213, 0)
            # Tone choices alone must not start an agent.
            select_personality(preferences, "jackson_lamb", capture=True)
            assert not (root / "agent-rpc.jsonl").exists()
            # Default before the first turn leaves the task unmodified.
            select_personality(preferences, "default")
            long_composer(root, window, process, base / "ui-state.json", prompt)
            # Discover and select models/efforts before the first prompt. The
            # temporary catalog connection must never create a conversation.
            click(1090, 752)
            wait_for(
                lambda: sum(m.get("method") == "model/list" for m in rpc(root)) == 2,
                "A new chat did not load the complete model catalog",
            )
            screenshot(window, "new-chat-model-controls.png")
            assert all(
                m.get("method") in ("initialize", "initialized", "model/list") for m in rpc(root)
            )
            click(1090, 633)
            wait_for(
                lambda: agent_preferences(preferences).get("model") == "fixture-deep",
                "A model could not be selected before the first message",
            )
            key("Escape")
            click(1090, 752)
            click(1075, 693)
            click(960, 744)
            wait_for(
                lambda: agent_preferences(preferences).get("reasoning_effort") == "high",
                "Reasoning effort could not be selected before the first message",
            )
            key("Escape")
            send(root, "first\nsecond line", first=True)
            first_turn = next(m for m in rpc(root) if m.get("method") == "turn/start")
            assert first_turn["params"]["model"] == "fixture-deep"
            assert first_turn["params"]["effort"] == "high"
            scope = json.loads((root / "agent-scope.json").read_text())
            assert scope["cwd"] == str(root)
            info = subprocess.check_output(["xwininfo", "-id", hex(window)], text=True)
            if "[Sandbox]" in info:
                assert not scope["private_visible"]
            select_personality(preferences, "jackson_lamb")
            send(root, "second")
            styled = next(m for m in reversed(rpc(root)) if m.get("method") == "turn/start")
            assert styled["params"]["input"][0]["text"] == "second"
            assert 'name="jackson_lamb"' in styled["params"]["input"][1]["text"]
            assert sum(m.get("method") == "initialize" for m in rpc(root)) == 2
            # Recall is local editing. Assert each transition through the real
            # clipboard instead of assuming several X11 keys rendered separately.
            focus_composer()
            text("saved draft")
            before_recall = len(rpc(root))
            browse(["second", "first\nsecond line"])
            browse(["second", "saved draft"], direction="Down")
            assert len(rpc(root)) == before_recall, "History browsing sent an agent request"
            key("End", ("Control_L",))
            text(" restored")
            submit(root, "saved draft restored")
            browse(["saved draft restored", "second"])
            key("End", ("Control_L",))
            text(" recalled")
            submit(root, "second recalled")
            # Tool output may evict user messages from the display transcript;
            # recall must still traverse every sent prompt, in both directions.
            send(root, "history-burst")
            send(root, "newest after burst")
            browse(["newest after burst"])
            key("End", ("Control_L",))
            text(" recalled after eviction")
            submit(root, "newest after burst recalled after eviction")
            history = [
                "first\nsecond line",
                "second",
                "saved draft restored",
                "second recalled",
                "history-burst",
                "newest after burst",
                "newest after burst recalled after eviction",
            ]
            focus_composer()
            text("draft after burst")
            before_recall = len(rpc(root))
            browse(reversed(history[1:]))
            browse(history[2:] + ["draft after burst"], direction="Down")
            assert len(rpc(root)) == before_recall, "Browsing retained prompts sent a request"
            key("End", ("Control_L",))
            text(" restored")
            submit(root, "draft after burst restored")
            browse(reversed(history + ["draft after burst restored"]))
            key("End", ("Control_L",))
            text(" from retained history")
            submit(root, "first\nsecond line from retained history")
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
            send(root, "approve", idle=False)
            selected_turn = next(m for m in reversed(rpc(root)) if m.get("method") == "turn/start")
            assert selected_turn["params"]["effort"] == "high"
            assert selected_turn["params"]["input"][1] == styled["params"]["input"][1]
            assert not (root / "approved.txt").exists()
            screenshot(window, "permission.png")
            header_click(350, 188)  # Allow once, after the visible command and Details.
            wait_for(lambda: (root / "approved.txt").exists(), "Approval was not delivered")
            time.sleep(0.3)
            send(root, "wait", idle=False)
            follow_up = next(m for m in reversed(rpc(root)) if m.get("method") == "turn/start")
            assert follow_up["params"]["input"][1] == styled["params"]["input"][1]
            wait_for(lambda: (root / "agent-child.pid").exists(), "Long task did not start")
            click(380, 840)  # Stop, without interrupting the project terminal.
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
            key("space", ("Control_L",))
            key("e")
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
            select_provider(preferences, "local")
            header_click(1280, 193)
            text("fixture-local-model")
            wait_for(
                lambda: "fixture-local-model" in preferences.read_text(), "Model was not saved"
            )
            screenshot(window, "local-model-settings.png")
            header_click(1213, 0)
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
            select_provider(preferences, "acp")
            header_click(1280, 94)
            text("./agent.py")
            header_click(1213, 0)
            send(root, "acp-provider")
            assert any(m.get("method") == "session/new" for m in rpc(root))
            calls_before_tone = len(rpc(root))
            select_personality(preferences, "default")
            assert len(rpc(root)) == calls_before_tone, "Tone configured the idle ACP agent"
            select_personality(preferences, "jackson_lamb")
            assert len(rpc(root)) == calls_before_tone
            click(1090, 752)
            screenshot(window, "acp-model-controls.png")
            click(1000, 645)
            screenshot(window, "acp-model-submenu.png")
            click(820, 670)
            wait_for(
                lambda: (
                    agent_preferences(preferences).get("acp_options", {}).get("model")
                    == "fixture-deep"
                ),
                "ACP model submenu did not save its selection",
            )
            key("Escape")
            click(1090, 752)
            click(1000, 693)
            screenshot(window, "acp-effort-submenu.png")
            click(820, 718)
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
            click_button(
                window, process, log.name, base / "ui-state.json", "agent-acp-option-automatic"
            )
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
            header_click(760, 0)  # New chat remains accessible beside the long provider label.
            x11.XResizeWindow(display, window, 1380, 900)
            x11.XFlush(display)
            time.sleep(0.3)
            send(root, "new-acp-conversation")
            assert sum(m.get("method") == "initialize" for m in rpc(root)) == before + 1
            # Claude is a distinct provider using the same ACP transport. Its
            # executable and controls must not overwrite the custom ACP profile.
            saved_acp = agent_preferences(preferences)["acp_options"].copy()
            before = len(rpc(root))
            select_provider(preferences, "claude")
            header_click(1280, 94)
            key("a", ("Control_L",))
            text("./agent.py")
            wait_for(
                lambda: agent_preferences(preferences).get("claude_program") == "./agent.py",
                "Claude adapter setting was not saved",
            )
            assert len(rpc(root)) == before, "Selecting Claude started an agent"
            header_click(1213, 0)
            send(root, "claude-provider")
            initialized = next(m for m in reversed(rpc(root)) if m.get("method") == "initialize")
            assert initialized["params"]["protocolVersion"] == 1
            assert agent_preferences(preferences)["acp_options"] == saved_acp
            click(1090, 752)
            click(1000, 645)
            click(820, 670)
            wait_for(
                lambda: (
                    agent_preferences(preferences).get("claude_options", {}).get("model")
                    == "fixture-deep"
                ),
                "Claude model control did not save its separate selection",
            )
            assert agent_preferences(preferences)["acp_options"] == saved_acp
            key("Escape")
            screenshot(window, "claude-provider.png")
            # The native permission selectors persist choices and affect the
            # next Codex turn while the existing outer sandbox remains active.
            select_provider(preferences, "codex")
            header_click(1213, 0)  # Hide the automatically opened provider settings.
            click(1280, 752)
            click_button(
                window, process, log.name, base / "ui-state.json", "agent-approval-Never ask"
            )
            wait_for(
                lambda: agent_preferences(preferences).get("approval") == "never",
                "Approval policy was not persisted",
            )
            click(1280, 752)  # Selecting a policy closes the native menu.
            click_button(
                window, process, log.name, base / "ui-state.json", "agent-access-Full access"
            )
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
            header_click(1340, 74)
            wait_for(
                lambda: any(m.get("method") == "thread/list" for m in rpc(root)),
                "History did not list saved conversations",
            )
            time.sleep(0.3)
            screenshot(window, "history-list.png")
            # Saved tool output evicts early display rows. Recall must retain
            # user prompts independently of that rolling transcript.
            (root / "history-fixture.json").write_text(json.dumps({"replay_tools": 80}))
            assert sum(m.get("method") == "turn/start" for m in rpc(root)) == prompts_before
            x11.XResizeWindow(display, window, 850, 560)
            x11.XFlush(display)
            time.sleep(0.3)
            screenshot(window, "minimum-history.png")
            x11.XResizeWindow(display, window, 1380, 900)
            x11.XFlush(display)
            time.sleep(0.3)
            header_click(700, 349)  # Load older metadata without prompting the model.
            wait_for(
                lambda: sum(m.get("method") == "thread/list" for m in rpc(root)) == 2,
                "History did not load another page",
            )
            header_click(700, 175)
            text("saved-2")
            time.sleep(0.2)
            screenshot(window, "history-filtered.png")
            # The filter exposes the older conversation first. Resolve its real
            # content bounds after layout instead of clicking the card's padding.
            click_button(
                window,
                process,
                ARTIFACTS / "chat.log",
                base / "ui-state.json",
                "Restore conversation",
            )
            wait_for(
                lambda: any(m.get("method") == "thread/resume" for m in rpc(root)),
                "History did not restore the selected conversation",
            )
            time.sleep(0.3)
            screenshot(window, "history-restored.png")
            focus_composer()
            control("agent-idle", focused=True)
            browse(["Earlier question 2", "Earlier question 1"])
            key("End", ("Control_L",))
            text(" reused")
            submit(root, "Earlier question 1 reused")
            send(root, "continue-saved-work")
            last = next(m for m in reversed(rpc(root)) if m.get("method") == "turn/start")
            assert last["params"]["threadId"] == "saved-2"
            browse(
                [
                    "continue-saved-work",
                    "Earlier question 1 reused",
                    "Earlier question 2",
                    "Earlier question 1",
                ]
            )
            key("End", ("Control_L",))
            text(" after continuation")
            submit(root, "Earlier question 1 after continuation")
            close_window(window, process)
            print(
                "Agent UI: long-draft scrolling/resizing, Enter/Shift+Enter, personalities, history restore, model/access selectors, saved policies, chat context, approvals, Stop, themes, and unsaved buffers passed."
            )
        except Exception:
            # The temporary project disappears on exit; preserve the peer's
            # actual requests so CI explains wrong, missing or duplicate sends.
            if (root / "agent-rpc.jsonl").exists():
                shutil.copy2(root / "agent-rpc.jsonl", ARTIFACTS / "failure-rpc.jsonl")
                (ARTIFACTS / "failure-prompts.json").write_text(
                    json.dumps(submitted(root), ensure_ascii=False, indent=2)
                )
            if (base / "ui-state.json").exists():
                shutil.copy2(base / "ui-state.json", ARTIFACTS / "failure-state.json")
            if window is not None and process.poll() is None:
                screenshot(window, "failure.png")
            raise
        finally:
            if process.poll() is None:
                process.terminate()
                process.wait(timeout=5)
