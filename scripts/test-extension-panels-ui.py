#!/usr/bin/env python3
"""Native installed-panel enablement, fields, confirmation and window lifecycle."""

import json
import os
import signal
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
os.environ["RODE_UI_ARTIFACTS"] = str(ROOT / "target/extension-panels-ui")
from ui_fixture import native_session
from ui_harness import (
    BINARY,
    fixture_directory,
    fixture_environment,
    focus_window,
    key,
    mouse_button,
    screenshot,
    wait_for_effect,
)


def column_width(state):
    rect = next(
        b["rect"] for b in state["buttons"] if b["name"] == "extension-table-column:test.panel:0:0"
    )
    return rect[2] - rect[0]


with fixture_directory(prefix="rode-panels-") as base:
    env = fixture_environment(base)
    project = base / "project"
    project.mkdir()
    log = base / "provider.jsonl"
    manifest = base / "config/rode/extensions/test.panel.json"
    manifest.parent.mkdir(parents=True)
    manifest.write_text(
        json.dumps(
            dict(
                api=1,
                id="test.panel",
                name="Test dashboard",
                version="1",
                description="Synthetic desktop provider",
                panel=True,
                panel_version=2,
                tool=dict(
                    program="python3", args=[str(ROOT / "tests/fixtures/panel.py"), str(log)]
                ),
            )
        )
    )

    def calls():
        return [json.loads(line) for line in log.read_text().splitlines()] if log.exists() else []

    with native_session(
        [str(BINARY), "--theme", "studio", str(project)], env, project, "panels"
    ) as ui:
        assert not log.exists(), "Manifest discovery started its provider"
        key("x", ("Control_L", "Shift_L"))
        ui.fill("extension-filter", "test.panel")
        ui.click("extension-toggle:test.panel")
        ui.click("extension-edit-settings:test.panel")
        ui.click("extension-settings-json")
        ui.fill("extension-settings-json", '{"configured":true}')
        ui.click("extension-save-settings")
        key("Escape")
        ui.control("extension-panel:test.panel")
        assert not log.exists(), "Enabling the provider started it before opening"
        ui.click("extension-panel:test.panel")
        ui.control("extension-action:refresh")
        assert calls()[0]["cwd"] == str(base / "home"), (
            "Panel inherited the project working directory"
        )
        assert calls()[0]["settings"] == {"configured": True}, (
            "Host JSON settings did not reach the provider"
        )
        # Real header dragging, with frame acknowledgements rather than delays.
        initial_width = column_width(ui.layout())
        grip = "extension-table-column-resize:test.panel:0:0"
        start = ui.control(grip, hovered=True)
        scale = ui.layout()["pixels_per_point"]
        end = (round(start[0] + 120 * scale), start[1])
        mouse_button(*start, 1, True)
        try:
            ui.move_pointer(round(start[0] + 60 * scale), start[1])
            ui.move_pointer(*end)
        finally:
            mouse_button(*end, 1, False)
        ui.layout(
            lambda state: abs(column_width(state) - initial_width - 120) <= 2,
            "Dragging the column edge did not resize it",
        )
        ui.control(grip, focused=True)
        key("Left")
        ui.layout(
            lambda state: abs(column_width(state) - initial_width - 100) <= 2,
            "Focused column divider did not resize with the keyboard",
        )
        ui.click("extension-table:test.panel:0")
        ui.control("extension-table:test.panel:0", focused=True)
        key("End")
        ui.value("extension-table-position:test.panel:0", "end")
        key("Home")
        ui.value("extension-table-position:test.panel:0", "start")
        ui.click("extension-settings")
        ui.click("extension-field:filter")
        ui.fill("extension-field:filter", "keyboard input")
        ui.click("extension-action:refresh")
        wait_for_effect(
            lambda: any(call.get("filter") == "keyboard input" for call in calls()),
            "Native field edits were not delivered",
        )
        ui.layout(
            lambda state: abs(column_width(state) - initial_width - 100) <= 2,
            "Provider refresh lost the column width",
        )
        ui.click(grip)
        key("Home")
        ui.layout(
            lambda state: abs(column_width(state) - initial_width) <= 2,
            "Home did not restore automatic column sizing",
        )
        ui.click("extension-settings")
        ui.click("extension-action:start")
        ui.control("extension-action:stop")
        ui.project_editor()
        ui.click("extension-panel:test.panel")
        ui.control("extension-action:stop")
        assert sum(call["action"] == "open" for call in calls()) == 1
        ui.click("extension-action:commit")
        ui.control("extension-confirm-cancel")
        assert not any(call["action"] == "commit" for call in calls())
        ui.click("extension-confirm-cancel")
        ui.click("extension-action:commit")
        ui.click("extension-confirm-submit")
        wait_for_effect(
            lambda: any(call["action"] == "commit" for call in calls()),
            "Confirmed action was not delivered",
        )
        ui.control("extension-action:stop")
        ui.click("extension-settings")
        ui.click("extension-field:filter")
        ui.fill("extension-field:filter", "Unsent input before provider exit")
        ui.value("extension-field:filter-value", "Unsent input before provider exit")
        os.kill(calls()[-1]["pid"], signal.SIGTERM)
        ui.layout(
            lambda state: any(
                b["name"] == "extension-action:refresh" and not b["enabled"]
                for b in state["buttons"]
            ),
            "Panel transport did not stop",
        )
        ui.click("extension-panel:test.panel")
        ui.control("extension-action:refresh")
        ui.value("extension-field:filter-value", "Unsent input before provider exit")
        assert calls()[-1]["action"] == "open" and calls()[-1]["filter"] == "", (
            "Recovery automatically submitted a draft"
        )
        ui.click("extension-settings")
        screenshot(ui.window, "native-panel.png")
        ui.click("extension-panel:test.panel", mouse_button=3)
        ui.click("area-detach")
        child = ui.child_window("RODE · Test dashboard")
        child.control("extension-action:start")
        child.click("workspace-reattach:Test dashboard")
        focus_window(ui.window)
        ui.control("extension-action:start")
        key("x", ("Control_L", "Shift_L"))
        ui.fill("extension-filter", "test.panel")
        ui.click("extension-toggle:test.panel")
        key("Escape")
        ui.absent("extension-panel:test.panel")
        ui.absent("extension-action:refresh")
    print(
        "Installed panels: resizable tables, opt-in desktop execution, monitoring, confirmation, detaching and disablement passed."
    )
