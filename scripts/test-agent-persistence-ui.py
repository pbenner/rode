#!/usr/bin/env python3
"""Exercise live agent persistence, reattachment and opt-out through the native UI."""

import json
import os
import re
import shutil
import subprocess
import tempfile
import time
from pathlib import Path

os.environ["RODE_UI_ARTIFACTS"] = str(
    Path(__file__).resolve().parent.parent / "target/agent-persistence-ui"
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


with tempfile.TemporaryDirectory(prefix="rode-agent-persistence-ui-") as directory:
    base = Path(directory)
    root = base / "project"
    root.mkdir()
    shutil.copy2(ROOT / "tests/fixtures/persistent_agent.py", root / "agent.py")
    (root / "record-inputs").touch()
    attachment = base / "queued-document.txt"
    attachment.write_text("Attachment retained across restart")
    (root / "agent.py").chmod(0o755)
    environment = dict(
        os.environ,
        XDG_CONFIG_HOME=str(base / "config"),
        XDG_DATA_HOME=str(base / "data"),
        RODE_SESSION_DIR=str(base / "runtime"),
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
            click(1325, 840)
            assert not infos(), "Choosing persistence before Send started an agent"
            click(1325, 840)
            click(1280, 785)
            text("work")
            key("Return")
            wait(lambda: (root / "work-started").exists(), "Agent did not start")
            first = wait(lambda: infos(), "Native agent missing")[0]
            assert not first["persistent"], "New chats must default to persistence off"
            pid = (root / "agent.pid").read_text()
            screenshot(window, "off.png")
            click(1325, 840)
            wait(lambda: infos() and infos()[0]["persistent"], "Persist button did not enable")
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
            wait(lambda: (root / "work-done").exists(), "Work stopped when RODE closed")
            assert (root / "agent.pid").read_text() == pid
            time.sleep(0.4)
            assert (root / "work-done").read_text() == "1", (
                "A queued prompt ran without an attached view"
            )
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
            key("p", ("Control_L", "Shift_L"))
            text("Agent: Running sessions")
            key("Return")
            time.sleep(0.8)
            screenshot(window, "running.png")
            # The only row is our project; its Attach button follows the badges.
            click(515, 475)
            time.sleep(0.8)
            screenshot(window, "reattached.png")
            wait(lambda: infos() and infos()[0]["attached"], "Running did not reattach")
            wait(
                lambda: "./not-existing-agent" not in preferences.read_text(),
                "Reattach did not restore the live backend settings",
            )
            wait(
                lambda: (root / "work-done").read_text() == "2",
                "Queued prompt was lost on restart/reattach",
            )
            recorded = [
                json.loads(line)
                for line in (root / "recorded-inputs.jsonl").read_text().splitlines()
            ]
            assert len(recorded) == 2
            assert recorded[1][0]["text"] == "queued across restart"
            assert any(
                "Attachment retained across restart" in block.get("text", "")
                for block in recorded[1]
            )
            assert not any(
                "Changed after Rode closed" in block.get("text", "") for block in recorded[1]
            )
            click(1280, 785)
            text("work again")
            key("Return")
            wait(
                lambda: (root / "work-done").read_text() == "3", "Reattached chat did not continue"
            )
            assert (root / "agent.pid").read_text() == pid, "Reattach restarted the backend"
            click(1325, 840)
            wait(lambda: infos() and not infos()[0]["persistent"], "Persist did not switch off")
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
