#!/usr/bin/env python3
"""Application areas preserve project drafts, file buffers and the original shell."""

import os
from pathlib import Path

os.environ["RODE_UI_ARTIFACTS"] = str(
    Path(__file__).resolve().parent.parent / "target/navigation-areas-ui"
)
from ui_fixture import native_session, project_args
from ui_harness import fixture_directory, fixture_environment, key, screenshot, text

with fixture_directory(prefix="rode-areas-ui-") as base:
    project = base / "project"
    project.mkdir()
    source = project / "main.txt"
    source.write_text("original source\n")
    second = project / "second.txt"
    second.write_text("explicit file opening\n")
    with native_session(project_args(project), fixture_environment(base), project, "areas") as ui:
        ui.open_file("main.txt")
        ui.document(source)
        key("End", ("Control_L",))
        text("unsaved project edit")
        ui.source("original source\nunsaved project edit")
        ui.click("workspace-tab:Chat")
        ui.click("agent-composer")
        text("draft retained across application areas")
        ui.value("agent-composer-value", "draft retained across application areas")
        ui.click("activity:System monitor")
        ui.control("monitor-start")
        ui.absent("workspace-code")
        ui.absent("pane-Explorer")
        ui.click("activity:Connections")
        ui.absent("workspace-code")
        assert ui.layout()["project"]["root"] == str(project)
        ui.command("Agent: Chat")
        ui.value("agent-composer-value", "draft retained across application areas")
        ui.click("activity:Connections")
        ui.click("activity:Projects")
        ui.value("agent-composer-value", "draft retained across application areas")
        ui.click("workspace-code")
        ui.source("original source\nunsaved project edit")
        assert source.read_text() == "original source\n"
        key("s", ("Control_L",))
        ui.control("source-editor-saved")
        ui.terminal()
        text("echo $$ > first-shell.pid\n")
        ui.wait(
            lambda: (project / "first-shell.pid").exists(),
            "Initial shell did not acknowledge input",
        )
        ui.click("activity:System monitor")
        ui.click("monitor-start")
        ui.control("monitor-sample")
        ui.click("activity:Projects")
        terminal = ui.terminal_input()
        ui.control(terminal, focused=True)
        text("echo $$ > returned-shell.pid\n")
        ui.wait(
            lambda: (project / "returned-shell.pid").exists(),
            "Returning terminal did not accept input",
        )
        assert (project / "first-shell.pid").read_text() == (
            project / "returned-shell.pid"
        ).read_text()
        ui.click("activity:App settings")
        ui.absent("workspace-code")
        screenshot(ui.window, "app-settings.png")
        ui.click("activity:Projects")
        ui.click("workspace-code")
        ui.resize(850, 560)
        ui.control("workspace-tools")
        screenshot(ui.window, "project-toolbar-compact.png")
        ui.click("workspace-tools")
        ui.click("workspace-tab:Problems")
        ui.control("workbench-panel:PROBLEMS")
        screenshot(ui.window, "project-problems.png")
        ui.click("activity:Connections")
        ui.open_file("second.txt")
        ui.source("explicit file opening\n")
        ui.control("workspace-code-selected")
        ui.open_file("main.txt")
        ui.source("original source\nunsaved project edit")

print(
    "Application areas preserved chat drafts, unsaved files and the same shell; compact project navigation passed."
)
