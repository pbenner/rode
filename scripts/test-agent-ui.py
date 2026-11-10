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
    assert_running,
    click,
    click_button,
    close_window,
    display,
    focus_window,
    key,
    palette_command,
    read_json_lines,
    screenshot,
    text,
    wait_for_button,
    wait_for_project,
    wait_for_ui_state,
    wait_for_window,
    window_geometry,
    x11,
    xtest,
)

x11.XResizeWindow.argtypes = [ctypes.c_void_p, ctypes.c_ulong, ctypes.c_uint, ctypes.c_uint]


def wait_for(check, message):
    deadline = time.monotonic() + 10
    while time.monotonic() < deadline:
        assert_running(process, ARTIFACTS / "chat.log")
        if check():
            return
        time.sleep(0.05)
    raise AssertionError(message)


def window_for(process):
    log = ARTIFACTS / "chat.log"
    state_path = base / "ui-state.json"
    window = wait_for_window(process, log)
    wait_for_project(window, process, log, root)
    focus_window(window)
    state = wait_for_ui_state(process, log, state_path, lambda _: True, "No native layout")
    if any(button["name"] == "notice-dismiss" for button in state["buttons"]):
        click_button(window, process, log, state_path, "notice-dismiss")
        wait_for_ui_state(
            process,
            log,
            state_path,
            lambda state: all(button["name"] != "notice-dismiss" for button in state["buttons"]),
            "The startup notice did not close",
        )
    return window


def rpc(root):
    return read_json_lines(root / "agent-rpc.jsonl")


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


def click_control(name):
    """Click a named native widget once, after its enabled geometry is rendered."""
    click_button(window, process, ARTIFACTS / "chat.log", base / "ui-state.json", name)


def wait_layout(check, message):
    return wait_for_ui_state(
        process, ARTIFACTS / "chat.log", base / "ui-state.json", check, message, timeout=10
    )


def absent(name):
    wait_layout(
        lambda state: all(button["name"] != name for button in state["buttons"]),
        name + " did not close",
    )


def replace_control(name, value):
    """Edit fixture settings through native input and acknowledge their complete value."""
    click_control(name)
    control(name, focused=True)
    before = rendered_pass()
    key("a", ("Control_L",))
    text(value)
    control(name, focused=True, changed_after=before)
    key("a", ("Control_L",))
    key("c", ("Control_L",))
    copied_text(value, name)
    before = rendered_pass()
    key("End", ("Control_L",))
    control(name, focused=True, after_pass=before)


def close_settings(field):
    click_control("agent-settings")
    absent(field)


def resize(width, height, content="agent-composer"):
    before = rendered_pass()
    x11.XResizeWindow(display, window, width, height)
    x11.XFlush(display)
    wait_layout(
        lambda state: (
            state["pass"] > before
            and any(button["name"] == content for button in state["buttons"])
            and all(
                abs(logical * state["pixels_per_point"] - physical) <= 2
                for logical, physical in zip(state["size"], (width, height))
            )
        ),
        f"The {width}x{height} layout did not finish rendering",
    )


def controls_idle():
    """Close residual popups and wait for that input's fresh idle frame."""
    before = rendered_pass()
    key("Escape")
    control("agent-idle", after_pass=before)


def open_controls(name):
    """Wait beyond preference saving and request receipt for completed host settings."""
    controls_idle()
    click_control(name)


def select_acp_control(
    preferences, option, value, *, field="acp_options", permissions=False, captures=()
):
    """Keep native selection, saved preference, backend request and idle acknowledgement distinct."""

    def matches(actual):
        # Preserve strict boolean checks: Python considers True == 1.
        return actual is value if isinstance(value, bool) else actual == value

    previous = len(rpc(root))
    open_controls("agent-access-menu" if permissions else "agent-model-menu")
    if captures:
        screenshot(window, captures[0])
    if isinstance(value, bool):
        click_control(f"agent-acp-option-{option}")
    else:
        # Native submenus open on hover; only activate the actual choice.
        control(f"agent-acp-option-{option}", hovered=True)
        if len(captures) > 1:
            screenshot(window, captures[1])
        click_control(f"agent-acp-choice-{option}-{value}")
    wait_for(
        lambda: matches(agent_preferences(preferences).get(field, {}).get(option)),
        f"{field} {option} selection was not saved",
    )
    wait_for(
        lambda: any(
            call.get("method") == "session/set_config_option"
            and call["params"].get("configId") == option
            and matches(call["params"].get("value"))
            and (not isinstance(value, bool) or call["params"].get("type") == "boolean")
            for call in rpc(root)[previous:]
        ),
        f"{field} {option} selection did not reach the agent",
    )
    # The peer can have logged this request while its reply is still pending.
    # A fresh idle frame includes both host queue and provider acknowledgements.
    controls_idle()


def composer_start():
    """Wait for the native editor to place an unselected caret at character zero."""
    before = rendered_pass()
    key("Home", ("Control_L",))
    control("agent-composer-at-start", focused=True, after_pass=before)


def focus_composer():
    show_tools()
    click_control("agent-composer")
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
    # Verify the native draft before sending. A focus/cleanup failure must not
    # submit a different prompt and masquerade as a missing Enter afterwards.
    control("agent-composer", focused=True)
    key("a", ("Control_L",))
    key("c", ("Control_L",))
    copied_text(message, "draft-before-submit")
    before = rendered_pass()
    key("End", ("Control_L",))
    control("agent-composer", focused=True, after_pass=before)
    control("agent-send")
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
    control("agent-composer")


def select_provider(preferences, provider):
    """Select a known native menu row and wait for the saved provider identity."""
    click_control("agent-provider-menu")
    click_control("agent-provider-" + provider.capitalize())
    wait_for(
        lambda: agent_preferences(preferences).get("provider") == provider,
        f"Provider {provider!r} was not selected",
    )
    field = {
        "codex": "agent-codex-executable",
        "local": "agent-local-model",
        "acp": "agent-acp-executable",
        "claude": "agent-claude-executable",
    }[provider]
    control(field)


def select_personality(preferences, value, capture=False):
    """Use rendered tone controls and verify the persisted selection."""
    click_button(window, process, ARTIFACTS / "chat.log", base / "ui-state.json", "agent-tone")
    if capture:
        screenshot(window, "personality-menu.png")
    click_button(
        window,
        process,
        ARTIFACTS / "chat.log",
        base / "ui-state.json",
        f"agent-tone-choice-{value}",
    )
    wait_for(
        lambda: agent_preferences(preferences).get("personality") == value,
        f"Personality choice {value!r} was not saved",
    )
    # Saving a preference is independent of the host accepting it. Wait for
    # the rendered readiness state before submitting the following prompt.
    wait_for_button(
        window, process, ARTIFACTS / "chat.log", base / "ui-state.json", "agent-tone-ready"
    )


def send(root, message, first=False, idle=True):
    focus_composer()
    control("agent-composer-empty", focused=True)
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
    replace_control("file-dialog-path", "prompt.txt")
    key("Return")
    control("source-editor", focused=True)
    key("a", ("Control_L",))
    key("c", ("Control_L",))
    copied_text(prompt, "long-composer-source-copy")
    show_tools()
    click_button(window, process, ARTIFACTS / "chat.log", state_path, "agent-composer")
    control("agent-composer", focused=True)
    paste("agent-composer")
    screenshot(window, "long-composer-end.png")

    before = rendered_pass()
    xtest.XTestFakeMotionEvent(display, -1, *control("agent-composer", hovered=True), 0)
    for _ in range(24):
        xtest.XTestFakeButtonEvent(display, 4, 1, 0)
        xtest.XTestFakeButtonEvent(display, 4, 0, 0)
    x11.XFlush(display)
    control("agent-composer", hovered=True, after_pass=before)
    screenshot(window, "long-composer-scrolled.png")

    # Drag the divider upward; releasing it must return typing to the draft.
    divider_x, divider_y = control("agent-composer-resize", hovered=True)
    xtest.XTestFakeMotionEvent(display, -1, divider_x, divider_y, 0)
    xtest.XTestFakeButtonEvent(display, 1, 1, 0)
    x11.XFlush(display)
    control("agent-composer-resize-active")
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
    xtest.XTestFakeMotionEvent(display, -1, *control("agent-composer-resize", hovered=True), 0)
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
    replace_control("file-dialog-path", "composed.txt")
    key("Return")
    wait_for(lambda: (root / "composed.txt").exists(), "Could not inspect the long chat draft")
    text_matches(
        (root / "composed.txt").read_text(), prompt + " visible-tail", "long-composer-save"
    )
    assert (root / "prompt.txt").read_text() == prompt
    assert not (root / "agent-rpc.jsonl").exists(), "Composing or resizing started an agent"
    show_tools()
    click_button(window, process, ARTIFACTS / "chat.log", state_path, "agent-composer")
    control("agent-composer", focused=True)
    before = json.loads(state_path.read_text())["pass"]
    key("a", ("Control_L",))
    key("BackSpace")
    control("agent-composer-empty", focused=True, after_pass=before)


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
            control("agent-composer")
            assert not (root / "agent-rpc.jsonl").exists(), "Opening chat started a process"
            click_control("agent-settings")
            replace_control("agent-codex-executable", "./agent.py")
            wait_for(
                lambda: (
                    preferences.exists()
                    and agent_preferences(preferences).get("codex") == "./agent.py"
                ),
                "Agent executable preference was not saved immediately",
            )
            close_settings("agent-codex-executable")
            # Tone choices alone must not start an agent.
            select_personality(preferences, "jackson_lamb", capture=True)
            select_personality(preferences, "panne")
            assert not (root / "agent-rpc.jsonl").exists()
            # Default before the first turn leaves the task unmodified.
            select_personality(preferences, "default")
            long_composer(root, window, process, base / "ui-state.json", prompt)
            # Discover and select models/efforts before the first prompt. The
            # temporary catalog connection must never create a conversation.
            open_controls("agent-model-menu")
            wait_for(
                lambda: sum(m.get("method") == "model/list" for m in rpc(root)) == 2,
                "A new chat did not load the complete model catalog",
            )
            screenshot(window, "new-chat-model-controls.png")
            assert all(
                m.get("method") in ("initialize", "initialized", "model/list") for m in rpc(root)
            )
            click_control("agent-model-choice-fixture-deep")
            wait_for(
                lambda: agent_preferences(preferences).get("model") == "fixture-deep",
                "A model could not be selected before the first message",
            )
            key("Escape")
            open_controls("agent-model-menu")
            control("agent-reasoning-menu", hovered=True)
            click_control("agent-reasoning-high")
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
            select_personality(preferences, "panne")
            send(root, "second")
            styled = next(m for m in reversed(rpc(root)) if m.get("method") == "turn/start")
            assert styled["params"]["input"][0]["text"] == "second"
            assert 'name="panne"' in styled["params"]["input"][1]["text"]
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
            palette_command(
                window, process, ARTIFACTS / "chat.log", base / "ui-state.json", "Theme: Carbon"
            )
            wait_for(
                lambda: bool(re.search(r'"color_theme"\s*:\s*"carbon"', preferences.read_text())),
                "The Carbon theme was not saved",
            )
            screenshot(window, "conversation-carbon.png")
            open_controls("agent-model-menu")
            screenshot(window, "codex-model-controls.png")
            click_control("agent-model-choice-fixture-deep")
            wait_for(
                lambda: agent_preferences(preferences).get("model") == "fixture-deep",
                "Selected model was not persisted",
            )
            key("Escape")
            calls_before_effort = len(rpc(root))
            for effort in ("high", "low", "high"):
                open_controls("agent-model-menu")
                control("agent-reasoning-menu", hovered=True)
                screenshot(window, f"codex-effort-{effort}.png")
                click_control(f"agent-reasoning-{effort}")
                wait_for(
                    lambda: agent_preferences(preferences).get("reasoning_effort") == effort,
                    "Reasoning selector did not save " + effort,
                )
                key("Escape")
            assert len(rpc(root)) == calls_before_effort, "Effort selection started a turn"
            open_controls("agent-access-menu")
            screenshot(window, "codex-access-controls.png")
            key("Escape")
            send(root, "approve", idle=False)
            selected_turn = next(m for m in reversed(rpc(root)) if m.get("method") == "turn/start")
            assert selected_turn["params"]["effort"] == "high"
            assert selected_turn["params"]["input"][1] == styled["params"]["input"][1]
            assert not (root / "approved.txt").exists()
            control("agent-permission-choice-0")
            screenshot(window, "permission.png")
            click_control("agent-permission-choice-0")  # The fixture advertises Allow once first.
            wait_for(lambda: (root / "approved.txt").exists(), "Approval was not delivered")
            control("agent-idle")
            send(root, "wait", idle=False)
            follow_up = next(m for m in reversed(rpc(root)) if m.get("method") == "turn/start")
            assert follow_up["params"]["input"][1] == styled["params"]["input"][1]
            wait_for(lambda: (root / "agent-child.pid").exists(), "Long task did not start")

            def new_chat_state():
                state = json.loads((base / "ui-state.json").read_text())
                button = next(item for item in state["buttons"] if item["name"] == "agent-new-chat")
                return state, button

            wait_for(
                lambda: not new_chat_state()[1]["enabled"],
                "New chat was enabled during active work",
            )
            state, button = new_chat_state()
            x, y, _, _ = window_geometry(window)
            left, top, right, bottom = button["rect"]
            scale = state["pixels_per_point"]
            interrupts = sum(m.get("method") == "turn/interrupt" for m in rpc(root))
            click(round(x + (left + right) * scale / 2), round(y + (top + bottom) * scale / 2))
            wait_for(lambda: rendered_pass() > state["pass"], "Disabled click was not rendered")
            assert not new_chat_state()[1]["enabled"], "New chat discarded the active turn"
            assert sum(m.get("method") == "turn/interrupt" for m in rpc(root)) == interrupts
            screenshot(window, "new-chat-disabled.png")
            click_control("agent-stop")
            wait_for(
                lambda: sum(m.get("method") == "turn/interrupt" for m in rpc(root)) > interrupts,
                "Stop did not interrupt the turn",
            )
            control("agent-idle")  # Request receipt alone does not acknowledge a completed Stop.
            # Open and edit a file without saving. Agent edits must not replace that buffer.
            palette_command(
                window, process, ARTIFACTS / "chat.log", base / "ui-state.json", "Open file by path"
            )
            replace_control("file-dialog-path", "main.py")
            key("Return")
            control("source-editor", focused=True)
            key("End", ("Control_L",))
            text("# keep my unsaved change\n")
            key("a", ("Control_L",))
            key("c", ("Control_L",))
            copied_text(
                "# original on disk\n# keep my unsaved change\n", "unsaved-buffer-before-agent"
            )
            send(root, "rewrite")
            wait_for(lambda: source.read_text() == "# written by the agent\n", "Agent did not edit")
            screenshot(window, "unsaved-buffer.png")
            key("space", ("Control_L",))
            key("e")
            control("source-editor", focused=True)
            key("s", ("Control_L", "Shift_L"))
            replace_control("file-dialog-path", "preserved.txt")
            key("Return")
            preserved = root / "preserved.txt"
            wait_for(preserved.exists, "Could not save the preserved user buffer")
            assert "keep my unsaved change" in preserved.read_text()
            assert "written by the agent" not in preserved.read_text()
            # Minimum window keeps the composer and Send button accessible.
            focus_composer()
            resize(850, 560)
            wait_layout(
                lambda state: any(button["name"] == "agent-send" for button in state["buttons"]),
                "Send was clipped out of the compact chat",
            )
            screenshot(window, "minimum-chat.png")
            resize(1380, 900)
            close_window(window, process)
            # A native restart restores executable settings, but not the conversation.
            before = sum(m.get("method") == "initialize" for m in rpc(root))
            process = subprocess.Popen(
                [str(BINARY), "--sandbox", str(root)], env=environment, stdout=log, stderr=log
            )
            window = window_for(process)
            key("a", ("Control_L", "Shift_L"))
            control("agent-composer")
            send(root, "restored")
            selected_turn = next(m for m in reversed(rpc(root)) if m.get("method") == "turn/start")
            assert selected_turn["params"]["effort"] == "high"
            assert sum(m.get("method") == "initialize" for m in rpc(root)) == before + 1
            assert "keep my unsaved change" not in preferences.read_text()
            select_provider(preferences, "local")
            replace_control("agent-local-model", "fixture-local-model")
            wait_for(
                lambda: agent_preferences(preferences).get("local_model") == "fixture-local-model",
                "Model was not saved",
            )
            screenshot(window, "local-model-settings.png")
            close_settings("agent-codex-executable")
            send(root, "local-provider")
            scope = json.loads((root / "agent-scope.json").read_text())
            assert 'model_provider="rode_local"' in scope["argv"]
            resize(850, 560)
            screenshot(window, "minimum-local-chat.png")
            resize(1380, 900)
            select_provider(preferences, "acp")
            (root / "slow-controls").touch()
            replace_control("agent-acp-executable", "./agent.py")
            wait_for(
                lambda: agent_preferences(preferences).get("acp_program") == "./agent.py",
                "ACP executable was not saved",
            )
            close_settings("agent-acp-executable")
            send(root, "acp-provider")
            assert any(m.get("method") == "session/new" for m in rpc(root))
            calls_before_tone = len(rpc(root))
            select_personality(preferences, "default")
            assert len(rpc(root)) == calls_before_tone, "Tone configured the idle ACP agent"
            select_personality(preferences, "jackson_lamb")
            assert len(rpc(root)) == calls_before_tone
            select_acp_control(
                preferences,
                "model",
                "fixture-deep",
                captures=("acp-model-controls.png", "acp-model-submenu.png"),
            )
            select_acp_control(
                preferences,
                "effort",
                "high",
                captures=("acp-effort-controls.png", "acp-effort-submenu.png"),
            )
            select_acp_control(
                preferences,
                "automatic",
                True,
                permissions=True,
                captures=("acp-permission-controls.png",),
            )
            resize(850, 560)
            screenshot(window, "minimum-acp-chat.png")
            before = sum(m.get("method") == "initialize" for m in rpc(root))
            click_button(window, process, log.name, base / "ui-state.json", "agent-new-chat")
            control("agent-composer-empty")
            resize(1380, 900)
            send(root, "new-acp-conversation")
            assert sum(m.get("method") == "initialize" for m in rpc(root)) == before + 1
            # Claude is a distinct provider using the same ACP transport. Its
            # executable and controls must not overwrite the custom ACP profile.
            saved_acp = agent_preferences(preferences)["acp_options"].copy()
            before = len(rpc(root))
            select_provider(preferences, "claude")
            replace_control("agent-claude-executable", "./agent.py")
            wait_for(
                lambda: agent_preferences(preferences).get("claude_program") == "./agent.py",
                "Claude adapter setting was not saved",
            )
            assert len(rpc(root)) == before, "Selecting Claude started an agent"
            close_settings("agent-claude-executable")
            send(root, "claude-provider")
            initialized = next(m for m in reversed(rpc(root)) if m.get("method") == "initialize")
            assert initialized["params"]["protocolVersion"] == 1
            assert agent_preferences(preferences)["acp_options"] == saved_acp
            select_acp_control(preferences, "model", "fixture-deep", field="claude_options")
            assert agent_preferences(preferences)["acp_options"] == saved_acp
            screenshot(window, "claude-provider.png")
            # The native permission selectors persist choices and affect the
            # next Codex turn while the existing outer sandbox remains active.
            select_provider(preferences, "codex")
            close_settings("agent-codex-executable")
            open_controls("agent-access-menu")
            click_button(
                window, process, log.name, base / "ui-state.json", "agent-approval-Never ask"
            )
            wait_for(
                lambda: agent_preferences(preferences).get("approval") == "never",
                "Approval policy was not persisted",
            )
            open_controls("agent-access-menu")  # Selecting a policy closes the native menu.
            sandboxed = "[Sandbox]" in subprocess.check_output(
                ["xwininfo", "-id", hex(window)], text=True
            )
            if sandboxed:
                control("agent-access-project-sandbox")
                controls = {
                    item["name"]: item
                    for item in json.loads((base / "ui-state.json").read_text())["buttons"]
                }
                for choice in ["Project access", "Full access"]:
                    assert not controls[f"agent-access-{choice}"]["enabled"], (
                        "An active project sandbox left duplicate agent sandbox controls enabled"
                    )
            click_button(
                window,
                process,
                log.name,
                base / "ui-state.json",
                "agent-access-project-sandbox" if sandboxed else "agent-access-Full access",
            )
            wait_for(
                lambda: (
                    # Unchanged defaults may be omitted by the preference writer.
                    agent_preferences(preferences).get("access", "project")
                    == ("project" if sandboxed else "full")
                ),
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
            if sandboxed:
                assert last["params"]["sandboxPolicy"]["type"] == "externalSandbox"
            # History is explicit host work; opening it must not send a model prompt.
            prompts_before = sum(m.get("method") == "turn/start" for m in rpc(root))
            click_control("agent-history")
            wait_for(
                lambda: any(m.get("method") == "thread/list" for m in rpc(root)),
                "History did not list saved conversations",
            )
            control("agent-history-ready")
            screenshot(window, "history-list.png")
            # Saved tool output evicts early display rows. Recall must retain
            # user prompts independently of that rolling transcript.
            (root / "history-fixture.json").write_text(json.dumps({"replay_tools": 80}))
            assert sum(m.get("method") == "turn/start" for m in rpc(root)) == prompts_before
            resize(850, 560, content="agent-history-filter")
            screenshot(window, "minimum-history.png")
            resize(1380, 900, content="agent-history-filter")
            click_control("agent-history-more")
            wait_for(
                lambda: sum(m.get("method") == "thread/list" for m in rpc(root)) == 2,
                "History did not load another page",
            )
            control("agent-history-restore:saved-2")
            control("agent-history-ready")
            replace_control("agent-history-filter", "saved-2")
            screenshot(window, "history-filtered.png")
            # The filter exposes the older conversation first. Resolve its real
            # content bounds after layout instead of clicking the card's padding.
            click_button(
                window,
                process,
                ARTIFACTS / "chat.log",
                base / "ui-state.json",
                "agent-history-restore:saved-2",
            )
            wait_for(
                lambda: any(m.get("method") == "thread/resume" for m in rpc(root)),
                "History did not restore the selected conversation",
            )
            control("agent-idle", focused=True)
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
