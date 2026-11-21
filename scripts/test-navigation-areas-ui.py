#!/usr/bin/env python3
"""Application areas preserve project drafts, file buffers and the original shell."""

import contextlib
import json
import os
import socket
from pathlib import Path

os.environ["RODE_UI_ARTIFACTS"] = str(
    Path(__file__).resolve().parent.parent / "target/navigation-areas-ui"
)
from ui_fixture import native_session, project_args
from ui_harness import (
    BINARY,
    fixture_directory,
    fixture_environment,
    focus_window,
    key,
    request_close,
    screenshot,
    text,
)

with fixture_directory(prefix="rode-areas-ui-") as base:
    project = base / "project"
    project.mkdir()
    source = project / "main.txt"
    source.write_text("original source\n")
    second = project / "second.txt"
    second.write_text("explicit file opening\n")
    environment = fixture_environment(base)
    profiles = [
        dict(
            name="Research cluster",
            host="cluster.example.invalid",
            user="researcher",
            root="/work/research",
        ),
        dict(
            name="Lab workstation",
            host="lab.example.invalid",
            user="scientist",
            root="/home/scientist",
        ),
    ]
    config = Path(environment["XDG_CONFIG_HOME"]) / "rode"
    config.mkdir()
    (config / "connections.json").write_text(json.dumps(profiles))
    saved = {
        "local-setup": dict(
            host=dict(kind="local", hostname=socket.gethostname()),
            spec=dict(
                name="Local compute return",
                relay_host="relay.example.invalid",
                relay_user="researcher",
            ),
        ),
        "remote-setup": dict(
            host=dict(
                kind="ssh", host="cluster.example.invalid", user="researcher", port="", config=""
            ),
            spec=dict(
                name="Cluster return", relay_host="relay.example.invalid", relay_user="researcher"
            ),
        ),
    }
    data = Path(environment["XDG_DATA_HOME"]) / "rode"
    data.mkdir()
    (data / "app.ron").write_text(json.dumps({"tunnel_profiles": json.dumps(saved)}))
    # No path argument prepares CWD while Welcome remains an independent view.
    with (
        contextlib.chdir(project),
        native_session([str(BINARY), "--theme", "carbon"], environment, project, "cwd") as ui,
    ):
        ui.control("welcome-screen")
        ui.control("workspace-explorer")
        ui.control("workspace-sidebar-closed")
        ui.absent("pane-Explorer")
        screenshot(ui.window, "welcome-cwd.png")
        ui.click("workspace-explorer")
        ui.control(f"explorer-row:{source}")
        ui.click("activity:Projects")
        ui.control("welcome-screen")
        ui.absent("pane-Explorer")
        ui.click("workspace-sidebar")
        ui.control("agent-composer")
        ui.click("activity:Projects")
        ui.control("welcome-screen")
        ui.absent("agent-composer")
        assert ui.layout()["project"]["root"] == str(project)

    with native_session(project_args(project), environment, project, "areas") as ui:
        ui.control("welcome-screen")
        ui.absent("pane-Explorer")
        ui.open_file("main.txt")
        ui.document(source)
        key("End", ("Control_L",))
        text("unsaved project edit")
        ui.source("original source\nunsaved project edit")
        ui.click("workspace-explorer")
        ui.absent("bookmark-project")
        ui.click("activity:Connections")
        ui.click("bookmark-project")
        ui.control("project-bookmarked")
        ui.dismiss_notice()
        ui.control("pane-Connections")
        ui.layout(
            lambda state: any(b["name"].startswith("saved-project:") for b in state["buttons"]),
            "The project bookmark did not move to Connections",
        )
        ui.control("source-editor")
        ui.absent("pane-Explorer")
        ui.click("workspace-explorer")
        ui.control("pane-Explorer")
        ui.absent("saved-connection-0")
        ui.layout(
            lambda state: not any(b["name"].startswith("saved-project:") for b in state["buttons"]),
            "Explorer still contains the saved-project library",
        )
        ui.click("workspace-explorer")
        ui.click("workspace-tab:Chat")
        ui.click("agent-composer")
        text("draft retained across application areas")
        ui.value("agent-composer-value", "draft retained across application areas")
        ui.click("activity:System monitor")
        ui.control("monitor-start")
        ui.control("workspace-sidebar")
        ui.absent("pane-Explorer")
        ui.click("activity:Connections")
        ui.control("workspace-sidebar")
        ui.control("saved-connection-0")
        ui.control("tunnel-saved-load:local-setup")
        ui.control("tunnel-saved-load:remote-setup", enabled=False)
        ui.control("pane-Connections")
        ui.control("monitor-start")
        ui.absent("connect-ssh")
        ui.absent("open-tunnels")
        bookmark = next(
            button["name"]
            for button in ui.layout()["buttons"]
            if button["name"].startswith("saved-project:")
        )
        for section, row in (
            ("Saved projects", bookmark),
            ("SSH connections", "saved-connection-0"),
            ("Saved tunnels", "tunnel-saved-load:local-setup"),
        ):
            ui.click("connections-section:" + section)
            ui.absent(row)
            if section == "Saved projects":
                ui.control("bookmark-project")
            ui.click("connections-section:" + section)
            ui.control(row)
            if section == "Saved projects":
                buttons = {button["name"]: button for button in ui.layout()["buttons"]}
                heading = buttons["connections-section:Saved projects"]["rect"]
                action = buttons["bookmark-project"]["rect"]
                pane = buttons["pane-Connections"]["rect"]
                assert action[2] - action[0] <= 26, action
                assert heading[2] <= action[0], (heading, action)
                assert abs((heading[1] + heading[3] - action[1] - action[3]) / 2) <= 1
                assert 0 <= pane[2] - action[2] <= 12, (pane, action)
        ui.control("tunnel-saved-load:remote-setup", enabled=False)
        rows = [
            button["rect"]
            for button in ui.layout()["buttons"]
            if button["name"].startswith(("saved-connection-", "tunnel-saved-load:"))
        ]
        assert len(rows) == 4, rows
        assert max(rect[2] for rect in rows) - min(rect[0] for rect in rows) <= 450, rows
        buttons = ui.layout()["buttons"]
        active = next(b["rect"] for b in buttons if b["name"] == "connections-manage-tunnels")
        for saved_row in (b["rect"] for b in buttons if b["name"].startswith("tunnel-saved-load:")):
            assert abs(active[0] - saved_row[0]) < 1 and abs(active[2] - saved_row[2]) < 1
            assert abs((active[3] - active[1]) - (saved_row[3] - saved_row[1])) < 1
        screenshot(ui.window, "connections-library.png")
        ui.click("connections-new-tunnel")
        ui.control("tunnel-dialog-new")
        ui.control("tunnel-field-Name")
        ui.absent("tunnel-dialog-active")
        ui.absent("tunnel-tab-list")
        ui.absent("tunnel-tab-new")
        screenshot(ui.window, "new-tunnel.png")
        ui.click("tunnel-close")
        ui.absent("tunnel-close")
        ui.click("connections-manage-tunnels")
        ui.control("tunnel-dialog-active")
        ui.absent("tunnel-dialog-new")
        ui.absent("tunnel-field-Name")
        ui.absent("tunnel-start")
        screenshot(ui.window, "active-tunnels.png")
        ui.click("tunnel-close")
        ui.absent("tunnel-close")
        ui.absent("tunnel-saved-remove:remote-setup")
        ui.click("tunnel-saved-row:remote-setup", mouse_button=3)
        ui.click("tunnel-saved-remove:remote-setup")
        ui.control("tunnel-saved-confirm-delete:remote-setup")
        ui.click("tunnel-saved-cancel-delete:remote-setup")
        ui.control("tunnel-saved-load:remote-setup", enabled=False)
        ui.hint("Manage saved tunnel Cluster return")
        ui.control("tunnel-saved-row:remote-setup", focused=True)
        key("F10", ("Shift_L",))
        ui.click("tunnel-saved-remove:remote-setup")
        ui.click("tunnel-saved-confirm-delete:remote-setup")
        ui.absent("tunnel-saved-load:remote-setup")
        ui.click("tunnel-saved-load:local-setup")
        ui.control("tunnel-start")
        ui.absent("tunnel-tab-saved")
        ui.click("tunnel-field-Name")
        ui.copy("tunnel-field-Name", "Local compute return")
        ui.click("tunnel-close")
        ui.absent("tunnel-close")
        assert ui.layout()["project"]["root"] == str(project)
        ui.command("Agent: Chat")
        ui.value("agent-composer-value", "draft retained across application areas")
        ui.click("activity:Connections")
        ui.absent("pane-Connections")
        ui.value("agent-composer-value", "draft retained across application areas")
        ui.project_editor()
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
        ui.control("workspace-sidebar")
        screenshot(ui.window, "app-settings.png")
        ui.click("activity:Projects")
        ui.project_editor()
        ui.resize(850, 560)
        ui.absent("workspace-tools")
        ui.click("workspace-sidebar")
        for tab in ("Chat", "Terminal", "Git", "Problems", "Tasks", "Settings"):
            ui.control(f"workspace-tab:{tab}")
        ui.absent("workspace-tab:Environment")
        screenshot(ui.window, "project-toolbar-compact.png")
        ui.click("workspace-tab:Problems")
        ui.control("workbench-panel:PROBLEMS")
        screenshot(ui.window, "project-problems.png")
        ui.click("workspace-tab:Settings")
        ui.click("settings-category-Environment")
        ui.control("environment-runtime-node")
        ui.control("environment-apply")
        ui.click("workspace-tab:Tasks")
        ui.click("workspace-tab:Settings")
        ui.control("environment-runtime-node")
        screenshot(ui.window, "project-settings-environment.png")
        ui.click("activity:Connections")
        ui.control("saved-connection-0")
        screenshot(ui.window, "connections-library-compact.png")
        ui.scroll_to_control("tunnel-saved-load:local-setup", over="saved-connection-0")
        screenshot(ui.window, "connections-library-compact-tunnels.png")
        ui.open_file("second.txt")
        ui.source("explicit file opening\n")
        ui.control("pane-Editor")
        ui.open_file("main.txt")
        ui.source("original source\nunsaved project edit")

        # Application windows share this process and retain the root's project view.
        ui.resize(1380, 900)
        for area, title, return_via in (
            ("Connections", "Connections", "rail"),
            ("App settings", "App settings", "header"),
            ("System monitor", "Monitor", "close"),
        ):
            icon = f"activity:{area}"
            ui.click(icon)
            ui.click(icon, mouse_button=3)
            ui.click("area-detach")
            child = ui.child_window(f"RODE · {title}")
            child.control(f"workspace-reattach:{title}")
            ui.control("pane-Editor")
            # Clicking the icon raises its existing window without changing root content.
            ui.click(icon)
            child.control(f"workspace-reattach:{title}")
            ui.control("pane-Editor")
            # Xvfb has no window manager: expose the child before pixel capture.
            focus_window(child.window)
            child.control(f"workspace-reattach:{title}", hovered=True)
            screenshot(child.window, f"detached-{title.lower().replace(' ', '-')}.png")
            if title == "Monitor":
                child.control("monitor-stop")
                child.control("monitor-sample")
            if title == "Connections":
                child.click("tunnel-saved-load:local-setup")
                child.control("tunnel-start")
                child.click("tunnel-close")
                child.absent("tunnel-close")
            if return_via == "rail":
                ui.click(icon, mouse_button=3)
                ui.click("area-reattach")
            elif return_via == "header":
                child.click(f"workspace-reattach:{title}")
            else:
                request_close(child.window)
            ui.control("workspace-sidebar")
            if title == "Monitor":
                ui.control("monitor-stop")
            if title == "Connections":
                ui.control("pane-Connections")
                ui.click("activity:Connections")
                ui.absent("pane-Connections")
            else:
                ui.click("activity:Projects")
            ui.source("original source\nunsaved project edit")
        ui.click("workspace-tab:Chat")
        ui.value("agent-composer-value", "draft retained across application areas")

print(
    "Application areas preserved drafts, files and the same shell; compact navigation and native detach/reattach passed."
)
