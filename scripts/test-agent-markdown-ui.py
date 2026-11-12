#!/usr/bin/env python3
"""Stream Markdown through a fixture agent and copy the rendered response at native pane sizes."""

import hashlib
import os
import shutil
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
os.environ["RODE_UI_ARTIFACTS"] = str(ROOT / "target/agent-markdown-ui")
from ui_fixture import native_session, project_args
from ui_harness import (
    display,
    fixture_directory,
    fixture_environment,
    key,
    move_pointer,
    screenshot,
    stop_fixture_agents,
    x11,
    xtest,
)

source = (ROOT / "tests/fixtures/agent-markdown.md").read_text()
expected = (ROOT / "tests/fixtures/agent-markdown.txt").read_text().removesuffix("\n")
copy_button = "agent-copy-response:" + hashlib.sha256(source.encode()).hexdigest()
body = "agent-response:" + hashlib.sha256(source.encode()).hexdigest()

with fixture_directory(prefix="rode-md-") as base:
    project = base / "project"
    project.mkdir()
    shutil.copy2(ROOT / "tests/fixtures/agent.py", project / "agent.py")
    (project / "agent.py").chmod(0o700)
    (project / "markdown-response.md").write_text(source)
    environment = fixture_environment(base)
    try:
        with native_session(project_args(project), environment, project, "markdown") as ui:
            key("a", ("Control_L", "Shift_L"))
            ui.control("agent-composer")
            ui.click("agent-settings")
            ui.edit("agent-codex-executable", "./agent.py")
            ui.click("agent-settings")
            ui.absent("agent-codex-executable")
            ui.edit("agent-composer", "markdown-tables")
            ui.press("agent-composer", "Return")
            ui.control(copy_button)
            ui.control("agent-idle")
            for width in (1380, 900, 640, 1380):
                ui.resize(width, 900)
                # Resizing can leave the reply header above the transcript's
                # viewport. One native wheel gesture reveals its actual button.
                ui.control(body, hovered=True)
                for _ in range(16):
                    xtest.XTestFakeButtonEvent(display, 4, 1, 0)
                    xtest.XTestFakeButtonEvent(display, 4, 0, 0)
                x11.XFlush(display)
                # A previous size's successful copy must not satisfy this one.
                sentinel = f"Markdown clipboard pending at {width}px"
                subprocess.run(
                    ["xclip", "-selection", "clipboard"],
                    input=sentinel,
                    text=True,
                    check=True,
                    timeout=5,
                )
                ui.wait(lambda: ui.clipboard() == sentinel, "Clipboard sentinel was not installed")
                ui.click(copy_button)
                ui.wait(lambda: ui.clipboard() == expected, "Rendered Markdown copy changed")
                move_pointer(*ui.control("menu-file"))
                screenshot(ui.window, f"markdown-{width}.png")
                if width == 640:
                    ui.control(body, hovered=True)
                    before = ui.layout()["pass"]
                    for _ in range(16):
                        xtest.XTestFakeButtonEvent(display, 7, 1, 0)
                        xtest.XTestFakeButtonEvent(display, 7, 0, 0)
                    x11.XFlush(display)
                    ui.control(body, after_pass=before)
                    move_pointer(*ui.control("menu-file"))
                    screenshot(ui.window, "markdown-640-right.png")
    finally:
        if not stop_fixture_agents(environment, project) and sys.exc_info()[0] is None:
            raise AssertionError("The Markdown fixture's agent did not stop")

print(
    "Agent Markdown: streamed tables, inline styles, math, code and lists copy correctly at wide and narrow window sizes."
)
