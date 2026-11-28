#!/usr/bin/env python3
"""Undo one completed prompt through native controls on a local or SSH host."""

import os
import shutil
import sys
from pathlib import Path

os.environ["RODE_UI_ARTIFACTS"] = str(
    Path(__file__).resolve().parent.parent / "target/agent-undo-ui"
)
from ui_fixture import native_session, project_args
from ui_harness import (
    ROOT,
    fixture_directory,
    fixture_environment,
    key,
    read_json_lines,
    screenshot,
    stop_fixture_agents,
    text,
)


def run():
    remote = os.environ.get("RODE_AGENT_UNDO_REMOTE") == "1"
    backend = os.environ.get("RODE_AGENT_UNDO_SSH_BACKEND", "russh")
    label = backend if remote else "local"
    # OpenSSH appends a random suffix to its Unix socket during master startup;
    # leave room for that suffix when the fixture root is inside a long checkout.
    with fixture_directory(prefix="au-") as base:
        project = base / "project"
        project.mkdir()
        source = project / "edited.txt"
        source.write_text("before\n")
        (project / "continue").touch()
        (project / "record-inputs").touch()
        shutil.copy2(ROOT / "tests/fixtures/persistent_agent.py", project / "agent.py")
        (project / "agent.py").chmod(0o755)
        environment = fixture_environment(base)
        args = project_args(project, remote=remote)
        if remote:
            args += ["--ssh-backend", backend]
        try:
            with native_session(args, environment, project, label) as ui:
                ui.command("Agent: Chat")
                ui.click("agent-settings")
                ui.click("agent-codex-executable")
                ui.fill("agent-codex-executable", "./agent.py")
                ui.click("agent-settings")

                def send():
                    ui.click("agent-composer")
                    ui.fill("agent-composer", "undo-edit")
                    before = ui.layout()["pass"]
                    key("Return")
                    ui.control("agent-composer-empty", after_pass=before)
                    ui.control("agent-undo", after_pass=before)
                    ui.control("agent-idle")

                send()
                controls = {button["name"]: button["rect"] for button in ui.layout()["buttons"]}
                row = controls["agent-undo"]
                for name in ["agent-history", "agent-running", "agent-status"]:
                    rect = controls[name]
                    assert abs((rect[1] + rect[3] - row[1] - row[3]) / 2) <= 1, (
                        f"{name} is not aligned with Undo last edits: {rect} vs {row}"
                    )
                row = controls["agent-send"]
                assert controls["agent-composer"][3] < row[1], "Send overlaps the draft"
                for name in ["agent-model-menu", "agent-access-menu", "agent-persistence"]:
                    rect = controls[name]
                    assert abs((rect[1] + rect[3] - row[1] - row[3]) / 2) <= 1, (
                        f"{name} is not aligned with Send: {rect} vs {row}"
                    )
                assert controls["agent-model-menu"][2] < controls["agent-access-menu"][0]
                assert controls["agent-access-menu"][2] < controls["agent-persistence"][0]
                assert source.read_text() == "after 1\n"
                # An open clean document must reload after the host acknowledges
                # Undo, without discarding a separately edited buffer.
                ui.open_file("edited.txt")
                ui.project_editor()
                ui.source("after 1\n")
                ui.click("source-editor")
                key("End", ("Control_L",))
                text("unsaved draft")
                ui.source("after 1\nunsaved draft")
                ui.command("Agent: Chat")
                ui.control("agent-undo", enabled=False)
                assert source.read_text() == "after 1\n"
                ui.project_editor()
                key("z", ("Control_L",))
                ui.source("after 1\n")
                ui.control("source-editor-saved")
                ui.command("Agent: Chat")
                ui.click("agent-undo")
                ui.value("agent-undo-revision", "1", enabled=False)
                ui.control("agent-idle")
                assert source.read_text() == "before\n"
                ui.project_editor()
                ui.source("before\n")
                ui.command("Agent: Chat")

                send()
                assert source.read_text() == "after 2\n"
                source.write_text("my later changes\n")
                ui.click("agent-undo")
                ui.value("agent-undo-revision", "2")
                ui.control("agent-idle")
                assert source.read_text() == "my later changes\n"
                calls = read_json_lines(project / "backend-rpc.jsonl")
                assert len([call for call in calls if call.get("method") == "turn/start"]) == 2
                assert not (project / ".git").exists()
                screenshot(ui.window, label + ".png")
        finally:
            if (
                not remote
                and not stop_fixture_agents(environment, project)
                and sys.exc_info()[0] is None
            ):
                raise AssertionError("Fixture agent did not stop")
    print(f"{label}: native agent Undo, clean-buffer reload and later-edit protection passed.")


if __name__ == "__main__":
    run()
