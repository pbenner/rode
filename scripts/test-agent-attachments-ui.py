#!/usr/bin/env python3
"""Attach local images/documents, queue snapshots and inspect real protocol payloads."""

import json
import os
import shutil
import struct
import subprocess
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
    assert_running,
    click,
    close_window,
    display,
    key,
    screenshot,
    text,
    wait_for_window,
    x11,
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
                path = project / "agent-rpc.jsonl"
                if not path.exists():
                    return []
                return [
                    call["params"]["input"]
                    for line in path.read_text().splitlines()
                    if (call := json.loads(line)).get("method") == "turn/start"
                ]

            def attach(path, busy=False, connected=False):
                click(460 if busy else 500 if connected else 410, 850)
                time.sleep(0.3)
                key("a", ("Control_L",))
                text(str(path))
                key("Return")
                time.sleep(0.8)

            window = wait_for_window(process, log_path)
            x11.XSetInputFocus(display, window, 1, 0)
            x11.XFlush(display)
            time.sleep(0.8)
            key("a", ("Control_L", "Shift_L"))
            time.sleep(0.4)
            attach(image)
            screenshot(window, "image-attached.png")
            click(480, 757)  # Remove the first image before adding it again.
            attach(image)
            attach(document)
            document.write_text("Changed after attaching")
            screenshot(window, "ready-to-send.png")
            click(700, 790)
            text("Inspect these files")
            key("Return")
            wait(lambda: len(turns()) == 1, "Attached message did not reach the agent")
            first = turns()[0]
            assert first[0]["text"] == "Inspect these files"
            assert sum(block.get("type") == "image" for block in first) == 1
            assert any(block.get("text", "").endswith(snapshot) for block in first)
            attach(pdf, busy=True)
            attach(queued, busy=True)
            screenshot(window, "queued-files-draft.png")
            click(700, 790)
            text("Follow-up with documents")
            key("Return")
            time.sleep(0.4)
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
            time.sleep(0.6)
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
            if process.poll() is None:
                process.terminate()
                process.wait(timeout=10)
