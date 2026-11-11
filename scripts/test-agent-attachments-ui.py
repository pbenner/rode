#!/usr/bin/env python3
"""Attach local images/documents, queue snapshots and inspect real protocol payloads."""

import hashlib
import json
import os
import shutil
import struct
import subprocess
import sys
import tempfile
import time
import zlib
from pathlib import Path

os.environ["RODE_UI_ARTIFACTS"] = str(
    Path(__file__).resolve().parent.parent / "target/agent-attachments-ui"
)
from ui_harness import (
    ARTIFACTS,
    BINARY,
    ROOT,
    NativeUI,
    assert_running,
    close_window,
    focus_window,
    key,
    read_json_lines,
    screenshot,
    stop_fixture_agents,
    stop_process,
    wait_for_project,
    wait_for_window,
)

with tempfile.TemporaryDirectory(prefix="rode-attach-ui-") as directory:
    base = Path(directory)
    project = base / "project"
    project.mkdir()
    fixture = (ROOT / "tests/fixtures/persistent_agent.py").read_text()
    fixture = fixture.replace(
        "    message = json.loads(line)\n",
        '    message = json.loads(line)\n    with (root / "agent-rpc.jsonl").open("a") as log:\n        log.write(json.dumps(message) + "\\n")\n',
    )
    (project / "agent.py").write_text(fixture)
    (project / "agent.py").chmod(0o755)
    # Attachments live outside the project, like images picked from the desktop for SSH chats.
    image = base / "image.png"

    def chunk(kind, data):
        return (
            struct.pack(">I", len(data)) + kind + data + struct.pack(">I", zlib.crc32(kind + data))
        )

    image.write_bytes(
        b"\x89PNG\r\n\x1a\n"
        + chunk(b"IHDR", struct.pack(">IIBBBBB", 1, 1, 8, 6, 0, 0, 0))
        + chunk(b"IDAT", zlib.compress(b"\x00\xff\x00\x00\xff"))
        + chunk(b"IEND", b"")
    )
    document = base / "notes.md"
    snapshot = "Native document snapshot λ\n" * 15000 + "COMPLETE-END"
    document.write_text(snapshot)
    queued = base / "queued.txt"
    queued.write_text("Queued snapshot")
    pdf = base / "pages.pdf"
    shutil.copy2(ROOT / "tests/fixtures/pages.pdf", pdf)
    config = base / "data/rode"
    config.mkdir(parents=True)
    profile = json.dumps(["LOCAL", str(project), ""], separators=(",", ":"))
    (config / "app.ron").write_text(
        json.dumps({"agent_preferences": json.dumps({profile: {"codex": "./agent.py"}})})
    )
    environment = dict(
        os.environ,
        XDG_CONFIG_HOME=str(base / "config"),
        XDG_DATA_HOME=str(base / "data"),
        RODE_SESSION_DIR=str(base / "runtime"),
        RODE_UI_TEST_STATE=str(base / "ui-state.json"),
    )
    log_path = ARTIFACTS / "attachments.log"
    with log_path.open("w") as log:
        process = subprocess.Popen(
            [str(BINARY), str(project)], env=environment, stdout=log, stderr=log
        )
        window = None
        try:

            def wait(check, message):
                deadline = time.monotonic() + 20
                while time.monotonic() < deadline:
                    assert_running(process, log_path)
                    if check():
                        return
                    time.sleep(0.05)
                raise AssertionError(message)

            def turns():
                return [
                    call["params"]["input"]
                    for call in read_json_lines(project / "agent-rpc.jsonl")
                    if call.get("method") == "turn/start"
                ]

            def attachment(path):
                return "agent-attachment:" + hashlib.sha256(path.name.encode()).hexdigest()

            def attach(path):
                ui.click("agent-attach")
                ui.fill_path(path)
                key("Return")
                ui.absent("file-dialog-path")
                ui.control(attachment(path))

            def send(message):
                ui.click("agent-composer")
                ui.fill("agent-composer", message)
                ui.control("agent-send")
                before = ui.layout()["pass"]
                key("Return")
                ui.control("agent-composer-empty", after_pass=before)

            window = wait_for_window(process, log_path)
            wait_for_project(window, process, log_path, project)
            focus_window(window)
            ui = NativeUI(window, process, log_path, base / "ui-state.json", project)
            key("a", ("Control_L", "Shift_L"))
            ui.control("agent-composer")
            attach(image)
            screenshot(window, "image-attached.png")
            ui.click("agent-attachment-remove:0")
            ui.absent(attachment(image))
            attach(image)
            attach(document)
            document.write_text("Changed after attaching")
            screenshot(window, "ready-to-send.png")
            send("Inspect these files")
            wait(lambda: len(turns()) == 1, "Attached message did not reach the agent")
            first = turns()[0]
            assert first[0]["text"] == "Inspect these files"
            assert sum(block.get("type") == "image" for block in first) == 1
            assert any(block.get("text", "").endswith(snapshot) for block in first)
            attach(pdf)
            attach(queued)
            screenshot(window, "queued-files-draft.png")
            send("Follow-up with documents")
            digest = hashlib.sha256(b"Follow-up with documents").hexdigest()
            ui.control("agent-queue-0-value:" + digest)
            queued.write_text("Changed after queuing")
            assert len(turns()) == 1, "Queued attachments interrupted the active turn"
            screenshot(window, "queued-files.png")
            (project / "continue").write_text("")
            wait(lambda: len(turns()) == 2, "Queued attachments did not dispatch")
            second = turns()[1]
            assert second[0]["text"] == "Follow-up with documents"
            assert any(block.get("text", "").endswith("Queued snapshot") for block in second)
            assert any("Attached document: pages.pdf" in block.get("text", "") for block in second)
            assert not any("Changed after" in block.get("text", "") for block in second)
            ui.control("agent-idle")
            screenshot(window, "sent-files.png")
            close_window(window, process)
            print(
                "Native attachments: image/document/PDF payloads, large prompts and immutable queued snapshots passed."
            )
        except BaseException:
            if window and process.poll() is None:
                screenshot(window, "failure.png")
            raise
        finally:
            stop_process(process)
            if not stop_fixture_agents(environment, project) and sys.exc_info()[0] is None:
                raise AssertionError("The fixture's agent service did not clean up")
