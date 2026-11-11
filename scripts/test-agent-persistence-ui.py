#!/usr/bin/env python3
"""Exercise live agent persistence, reattachment and opt-out through the native UI."""

import json
import os
import re
import shutil
import subprocess
import time
from pathlib import Path

os.environ["RODE_UI_ARTIFACTS"] = str(
    Path(__file__).resolve().parent.parent / "target/agent-persistence-ui"
)
from ui_harness import (
    ARTIFACTS,
    BINARY,
    ROOT,
    NativeUI,
    assert_running,
    click,
    click_button,
    close_window,
    fixture_directory,
    focus_window,
    key,
    read_json_lines,
    screenshot,
    text,
    wait_for_button,
)


def wait(check, message):
    deadline = time.monotonic() + 15
    while time.monotonic() < deadline:
        result = check()
        if result:
            return result
        time.sleep(0.05)
    raise AssertionError(message)


def mapped(process, log):
    assert_running(process, log)
    tree = subprocess.check_output(["xwininfo", "-root", "-tree"], text=True)
    match = re.search(r'(0x[0-9a-f]+) "RODE.*project', tree)
    if not match or "[Opening project]" in match[0]:
        return None
    return int(match[1], 16)


def show_chat(process, log):
    window = wait(lambda: mapped(process, log), "No prepared project window")
    time.sleep(0.8)
    focus_window(window)
    if "[Sandbox]" not in subprocess.check_output(["xwininfo", "-id", hex(window)], text=True):
        click(1331, 820)  # The fallback notification would otherwise cover Persist.
    key("a", ("Control_L", "Shift_L"))
    time.sleep(0.4)
    return window


with fixture_directory(prefix="rode-agent-persistence-ui-") as base:
    root = base / "project"
    root.mkdir()
    shutil.copy2(ROOT / "tests/fixtures/persistent_agent.py", root / "agent.py")
    (root / "record-inputs").touch()
    saved_prompts = ["oldest saved prompt", "same earlier prompt", "same earlier prompt"]
    (root / "saved-prompts.json").write_text(json.dumps(saved_prompts))
    attachment = base / "queued-document.txt"
    attachment.write_text("Attachment retained across restart")
    (root / "agent.py").chmod(0o755)
    environment = dict(
        os.environ,
        XDG_CONFIG_HOME=str(base / "config"),
        XDG_DATA_HOME=str(base / "data"),
        RODE_SESSION_DIR=str(base / "runtime"),
        RODE_UI_TEST_STATE=str(base / "ui-state.json"),
    )

    def infos():
        output = subprocess.check_output(
            [str(BINARY), "--session-service", "request"],
            input=b'{"action":"agents"}',
            env=environment,
        )
        response = json.loads(output)
        assert response["kind"] == "agents", response
        return response["data"]

    log_path = ARTIFACTS / "persistent.log"
    processes = []
    with log_path.open("w") as log:
        try:
            process = subprocess.Popen(
                [str(BINARY), "--sandbox", str(root)], env=environment, stdout=log, stderr=log
            )
            processes.append(process)
            window = show_chat(process, log_path)
            info = subprocess.check_output(["xwininfo", "-id", hex(window)], text=True)
            header = 157 if "[Sandbox]" in info else 187
            click(1213, header)  # Settings, existing executable: no installation or inference.
            click(1110, header + 94)
            key("a", ("Control_L",))
            text("./agent.py")
            click(1213, header)
            screenshot(window, "ready.png")
            assert not (root / "agent.pid").exists()
            assert not infos(), "Opening chat started a native agent"

            def persistence(mode):
                click_button(window, process, log_path, base / "ui-state.json", "agent-persistence")
                click_button(
                    window, process, log_path, base / "ui-state.json", f"agent-persist-{mode}"
                )

            persistence("on")
            assert not infos(), "Choosing persistence before Send started an agent"
            persistence("auto")
            click(1280, 785)
            text("work")
            key("Return")
            wait(lambda: (root / "work-started").exists(), "Agent did not start")
            first = wait(lambda: infos(), "Native agent missing")[0]
            assert first["persistent"] == "auto", "New chats must default to Auto"
            pid = (root / "agent.pid").read_text()
            screenshot(window, "auto.png")
            persistence("on")
            wait(
                lambda: infos() and infos()[0]["persistent"] is True,
                "Persist On was not acknowledged",
            )
            screenshot(window, "on.png")
            click(460, 850)  # Attach a desktop file to a waiting prompt.
            time.sleep(0.3)
            key("a", ("Control_L",))
            text(str(attachment))
            key("Return")
            time.sleep(0.8)
            click(700, 790)
            text("queued across restart")
            key("Return")
            # Close immediately: clean exit must wait for host acknowledgement.
            close_window(window, process)
            wait(lambda: infos() and not infos()[0]["attached"], "View did not detach")
            attachment.write_text("Changed after Rode closed")
            (root / "continue").touch()
            wait(
                lambda: (root / "work-done").exists() and (root / "work-done").read_text() == "2",
                "Queued work did not continue after RODE closed",
            )
            assert (root / "agent.pid").read_text() == pid
            time.sleep(0.4)
            assert (root / "work-done").read_text() == "2"
            # Another window may have changed this project's saved executable.
            # Attaching must restore the live backend's settings before follow-up.
            preferences = base / "data/rode/app.ron"
            saved = preferences.read_text()
            assert "./agent.py" in saved
            preferences.write_text(saved.replace("./agent.py", "./not-existing-agent"))
            process = subprocess.Popen(
                [str(BINARY), "--sandbox", str(root)], env=environment, stdout=log, stderr=log
            )
            processes.append(process)
            window = show_chat(process, log_path)
            ui = NativeUI(window, process, log_path, base / "ui-state.json", root)
            key("p", ("Control_L", "Shift_L"))
            text("Agent: Running sessions")
            key("Return")
            time.sleep(0.8)
            screenshot(window, "running.png")
            # The only card is our project; its Attach action is in the right column.
            click(898, 421)
            time.sleep(0.8)
            screenshot(window, "reattached.png")
            wait(lambda: infos() and infos()[0]["attached"], "Running did not reattach")
            wait(
                lambda: "./not-existing-agent" not in preferences.read_text(),
                "Reattach did not restore the live backend settings",
            )
            wait(
                lambda: (root / "work-done").read_text() == "2",
                "Completed queued work was lost on restart/reattach",
            )
            recorded = read_json_lines(root / "recorded-inputs.jsonl")
            assert len(recorded) == 2
            assert recorded[1][0]["text"] == "queued across restart"
            assert any(
                "Attachment retained across restart" in block.get("text", "")
                for block in recorded[1]
            )
            assert not any(
                "Changed after Rode closed" in block.get("text", "") for block in recorded[1]
            )
            # Older controllers can expose only the final transcript messages.
            # These earlier backend-owned prompts are absent from its recall
            # snapshot; Up must page them without claiming/restarting the chat.
            click(1280, 785)
            text("unsent after restart")
            ui.value("agent-composer-value", "unsent after restart", focused=True)

            def start_of_draft():
                before = ui.layout()["pass"]
                key("Home", ("Control_L",))
                ui.control("agent-composer-at-start", focused=True, after_pass=before)

            def recall(direction, expected):
                before = ui.layout()["pass"]
                key(direction)
                ui.control("agent-prompt-recall-ready", after_pass=before)
                ui.value("agent-composer-value", expected, focused=True, after_pass=before)
                # Recall runs before the text editor processes cursor keys. If
                # Home and Up share a frame, Up can still see the old selection.
                start_of_draft()

            start_of_draft()

            # Cancel a deliberately stalled initial lookup. Send must become
            # usable while that metadata peer is still unable to initialize.
            for expected in ["queued across restart", "work"]:
                recall("Up", expected)
            stalled = root / "delay-prompt-bootstrap"
            stalled.touch()
            key("Up")
            wait_for_button(
                window, process, log_path, base / "ui-state.json", "agent-prompt-recall-loading"
            )
            recall("Down", "queued across restart")
            recall("Down", "unsent after restart")
            wait_for_button(window, process, log_path, base / "ui-state.json", "agent-send")
            stalled.unlink()
            recalled = ["queued across restart", "work", *reversed(saved_prompts)]
            for expected in recalled:
                recall("Up", expected)
            for expected in [
                "same earlier prompt",
                "same earlier prompt",
                "work",
                "queued across restart",
                "unsent after restart",
            ]:
                recall("Down", expected)
            key("a", ("Control_L",))
            key("BackSpace")
            assert (root / "agent.pid").read_text() == pid, "Recall restarted the live backend"
            assert (root / "work-done").read_text() == "2", "Recall started inference"
            click(1280, 785)
            text("work again")
            key("Return")
            wait(
                lambda: (root / "work-done").read_text() == "3", "Reattached chat did not continue"
            )
            assert (root / "agent.pid").read_text() == pid, "Reattach restarted the backend"
            persistence("off")
            wait(
                lambda: infos() and infos()[0]["persistent"] is False, "Persist did not switch off"
            )
            screenshot(window, "off-again.png")
            close_window(window, process)
            wait(lambda: not infos(), "Nonpersistent agent survived the last view")
            print(
                "Native agent persistence, queued attachment restoration, reattachment and opt-out passed"
            )
        finally:
            for process in processes:
                if process.poll() is None:
                    process.terminate()
                    process.wait(timeout=10)
            for info in infos():
                subprocess.run(
                    [str(BINARY), "--session-service", "request"],
                    input=json.dumps(
                        dict(action="end_agent", id=info["id"], policy=info["policy"])
                    ).encode(),
                    env=environment,
                    check=True,
                )
