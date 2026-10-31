#!/usr/bin/env python3
"""Native queued follow-ups with a deterministic agent and no inference."""

import json
import os
import shutil
import subprocess
import tempfile
import time
from pathlib import Path

os.environ["RODE_UI_ARTIFACTS"] = str(
    Path(__file__).resolve().parent.parent / "target/agent-queue-ui"
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
    wait_for_window,
)

with tempfile.TemporaryDirectory(prefix="rode-agent-queue-ui-") as directory:
    base = Path(directory)
    project = base / "project"
    project.mkdir()
    shutil.copy2(ROOT / "tests/fixtures/agent.py", project / "agent.py")
    (project / "agent.py").chmod(0o755)
    config = base / "data/rode"
    config.mkdir(parents=True)
    profile = json.dumps(["LOCAL", str(project), ""], separators=(",", ":"))
    (config / "app.ron").write_text(
        json.dumps({"agent_preferences": json.dumps({profile: {"codex": "./agent.py"}})})
    )
    environment = dict(
        os.environ, XDG_CONFIG_HOME=str(base / "config"), XDG_DATA_HOME=str(base / "data")
    )
    log_path = ARTIFACTS / "queue.log"
    with log_path.open("w") as log:
        process = subprocess.Popen(
            [str(BINARY), str(project)], env=environment, stdout=log, stderr=log
        )
        window = None
        try:

            def wait(check, message):
                deadline = time.monotonic() + 15
                while time.monotonic() < deadline:
                    assert_running(process, log_path)
                    if check():
                        return
                    time.sleep(0.05)
                raise AssertionError(message)

            def prompts():
                path = project / "agent-rpc.jsonl"
                calls = (
                    [json.loads(line) for line in path.read_text().splitlines()]
                    if path.exists()
                    else []
                )
                return [
                    call["params"]["input"][0]["text"]
                    for call in calls
                    if call.get("method") == "turn/start"
                ]

            def send(message):
                click(700, 790)
                text(message)
                key("Return")
                time.sleep(0.2)

            window = wait_for_window(process, log_path)
            focus_window(window)
            key("a", ("Control_L", "Shift_L"))
            time.sleep(0.3)
            send("wait")
            wait(lambda: prompts() == ["wait"], "Initial turn was not started")
            send("first queued")
            send("second queued")
            send("delete this")
            assert prompts() == ["wait"], "A queued prompt interrupted the running turn"
            screenshot(window, "queued.png")
            # The bounded queue sits immediately above the unchanged composer.
            click(347, 657)  # Move the second prompt earlier.
            click(421, 632)  # Edit that prompt in place.
            time.sleep(0.2)
            screenshot(window, "editing.png")
            click(700, 540)
            key("a", ("Control_L",))
            text("edited second queued")
            click(360, 593)  # Save prompt.
            time.sleep(0.2)
            click(480, 682)  # Delete the final prompt.
            click(700, 790)
            text("untouched composer draft")
            click(404, 850)  # Stop pauses the remaining follow-ups.
            time.sleep(0.6)
            assert prompts() == ["wait"], "Stop started the next queued prompt"
            screenshot(window, "paused.png")
            click(440, 637)  # Resume queue.
            wait(lambda: len(prompts()) == 3, "The queue did not resume and drain automatically")
            assert prompts() == ["wait", "edited second queued", "first queued"], prompts()
            click(700, 790)
            key("Return")
            wait(
                lambda: len(prompts()) == 4,
                "The original composer draft was lost during queue editing",
            )
            assert prompts()[-1] == "untouched composer draft"
            screenshot(window, "drained.png")

            send("wait")
            wait(lambda: prompts()[-1] == "wait", "Second held turn did not start")
            send("bad-protocol")
            send("after failure")
            click(404, 850)
            time.sleep(0.6)
            click(440, 637)
            wait(lambda: prompts()[-1] == "bad-protocol", "Failed-turn fixture was not dispatched")
            time.sleep(0.8)
            assert "after failure" not in prompts(), "Failure did not pause the queue"
            screenshot(window, "failed-paused.png")
            click(1260, 115)  # New chat clears the old conversation's queue.
            time.sleep(0.3)
            send("new conversation")
            wait(lambda: prompts()[-1] == "new conversation", "New chat did not accept input")
            time.sleep(0.3)
            assert "after failure" not in prompts(), (
                "A queued prompt leaked into a new conversation"
            )
            close_window(window, process)
            print(
                "Queued prompts: FIFO, editing, reordering, deletion, Stop/failure pause, draft preservation and conversation isolation passed."
            )
        except BaseException:
            if window and process.poll() is None:
                screenshot(window, "failure.png")
            raise
        finally:
            if process.poll() is None:
                process.terminate()
                process.wait(timeout=10)
