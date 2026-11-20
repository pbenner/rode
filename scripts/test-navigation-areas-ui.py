#!/usr/bin/env python3
"""Application areas preserve project drafts, file buffers and the original shell."""

import json
import os
import socket
from pathlib import Path

os.environ["RODE_UI_ARTIFACTS"] = str(
    Path(__file__).resolve().parent.parent / "target/navigation-areas-ui"
)
from ui_fixture import native_session, project_args
from ui_harness import (
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
    with native_session(project_args(project), environment, project, "areas") as ui:
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
        ui.control("saved-connection-0")
        ui.control("tunnel-saved-load:local-setup")
        ui.control("tunnel-saved-load:remote-setup", enabled=False)
        cards = [
            button["rect"]
            for button in ui.layout()["buttons"]
            if button["name"].startswith(("saved-connection-card:", "tunnel-saved-card:"))
        ]
        assert len(cards) == 4, cards
        assert max(rect[2] for rect in cards) - min(rect[0] for rect in cards) <= 977, cards
        screenshot(ui.window, "connections-library.png")
        ui.click("tunnel-saved-load:local-setup")
        ui.control("tunnel-start")
        ui.click("tunnel-field-Name")
        ui.copy("tunnel-field-Name", "Local compute return")
        ui.click("tunnel-close")
        ui.absent("tunnel-close")
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
        ui.absent("workspace-tools")
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
        ui.control("workspace-code-selected")
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
            ui.control("workspace-code-selected")
            # Clicking the icon raises its existing window without changing root content.
            ui.click(icon)
            child.control(f"workspace-reattach:{title}")
            ui.control("workspace-code-selected")
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
            ui.absent("workspace-code")
            if title == "Monitor":
                ui.control("monitor-stop")
            ui.click("activity:Projects")
            ui.source("original source\nunsaved project edit")
        ui.click("workspace-tab:Chat")
        ui.value("agent-composer-value", "draft retained across application areas")

print(
    "Application areas preserved drafts, files and the same shell; compact navigation and native detach/reattach passed."
)
