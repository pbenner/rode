#!/usr/bin/env python3
"""Explicit workspace resume, durable drafts and generation-safe window takeover."""

import contextlib
import json
import os
import shutil
import subprocess
import time
from pathlib import Path

os.environ["RODE_UI_ARTIFACTS"] = str(
    Path(__file__).resolve().parent.parent / "target/workspace-sessions-ui"
)
from ui_fixture import capture_failure, native_session, project_args, project_ui
from ui_harness import (
    ARTIFACTS,
    BINARY,
    ROOT,
    fixture_directory,
    fixture_environment,
    focus_window,
    screenshot,
)


def wait(check, message, timeout=20):
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        if result := check():
            return result
        time.sleep(0.05)
    raise AssertionError(message)


def run():
    with fixture_directory(prefix="rode-workspace-sessions-") as base:
        root = base / "project"
        root.mkdir()
        source = root / "notes.txt"
        source.write_text("Saved original\n")
        shutil.copy2(ROOT / "tests/fixtures/persistent_agent.py", root / "agent.py")
        (root / "agent.py").chmod(0o755)
        (root / "continue").touch()
        env = fixture_environment(
            base,
            XDG_DATA_HOME=str(base / "data"),
            XDG_CONFIG_HOME=str(base / "config"),
            RODE_SESSION_DIR=str(base / "runtime"),
            RODE_UI_TEST_STATE=str(base / "layout.json"),
        )
        folder = base / "data/rode/workspace-sessions"

        def records():
            return [json.loads(p.read_text()) for p in folder.glob("s-*.json")]

        def record():
            items = records()
            return items[0] if len(items) == 1 else None

        def rpc(request):
            reply = json.loads(
                subprocess.check_output(
                    [str(BINARY), "--session-service", "request"],
                    input=json.dumps(request).encode(),
                    env=env,
                    timeout=15,
                )
            )
            assert reply["kind"] != "error", reply
            return reply.get("data")

        terminal = None
        agent = None
        try:
            with native_session(project_args(root), env, root, "capture") as ui:
                ui.open_file("notes.txt")
                ui.press("source-editor", "End", ("Control_L",))
                # Source editing uses the same acknowledged native path as ordinary editing.
                from ui_harness import text

                text("Unsaved draft\n")
                ui.source("Saved original\nUnsaved draft\n")
                ui.command("Terminal: New persistent terminal")
                terminal = wait(
                    lambda: next(iter(rpc({"action": "list"})), None), "No terminal created"
                )
                ui.control("terminal-input:" + terminal["id"])
                ui.press("terminal-input:" + terminal["id"], "Return")
                ui.click("workspace-tab:Chat")
                ui.click("agent-settings")
                ui.edit("agent-codex-executable", "./agent.py")
                ui.click("agent-settings")
                ui.click("agent-persistence")
                ui.click("agent-persist-on")
                ui.edit("agent-composer", "work")
                ui.press("agent-composer", "Return")
                ui.control("agent-idle")
                agent = wait(
                    lambda: next(iter(rpc({"action": "agents"})), None), "No agent created"
                )
                ui.edit("agent-composer", "Unsent workspace prompt")
                ui.command("Application: Show Welcome")
                ui.click("workspace-session-keep")
                ui.control("workspace-session-kept")
                saved = wait(record, "Workspace record not durable")
                identity = saved["id"]
                assert saved["snapshot"]["draft"] == "Unsent workspace prompt"
                assert saved["snapshot"]["documents"][0]["draft"]["text"].endswith(
                    "Unsaved draft\n"
                )
                assert source.read_text() == "Saved original\n"
                ui.click("workspace-session-rename:" + identity)
                ui.edit("workspace-session-name", "Pinned workspace")
                ui.click("workspace-session-save-name")
                wait(lambda: record()["name"] == "Pinned workspace", "Initial rename was not saved")
                for index in range(3):
                    previous = record()["owner"]["generation"]
                    ui.click("workspace-session-keep")
                    ui.absent("workspace-session-kept")
                    ui.click("workspace-tab:Chat")
                    ui.edit("agent-composer", f"Edited while paused {index}")
                    ui.command("Application: Show Welcome")
                    ui.click("workspace-session-keep")
                    ui.control("workspace-session-kept")
                    current = wait(record, "Toggle created duplicate workspace entries")
                    assert current["id"] == identity, "Keep created another saved workspace"
                    assert current["name"] == "Pinned workspace", (
                        "Re-enabling lost the workspace name"
                    )
                    assert current["owner"]["generation"] != previous
                    assert current["snapshot"]["draft"] == f"Edited while paused {index}"
                ui.click("workspace-tab:Chat")
                ui.value("agent-composer-value", "Edited while paused 2")
                ui.edit("agent-composer", "Unsent workspace prompt")
                ui.command("Application: Show Welcome")
                ui.control("workspace-session-kept")
                wait(
                    lambda: record()["snapshot"]["draft"] == "Unsent workspace prompt",
                    "Latest prompt was not saved",
                )
                screenshot(ui.window, "saved-workspace.png")
            wait(
                lambda: any(
                    i["id"] == agent["id"] and not i["attached"] for i in rpc({"action": "agents"})
                ),
                "Persistent agent did not detach",
            )
            with native_session(project_args(root), env, root, "resume") as ui:
                ui.control("welcome-screen")
                ui.absent("source-editor")
                ui.click("workspace-session-resume:" + identity)
                ui.control("agent-composer")
                ui.value("agent-composer-value", "Unsent workspace prompt")
                wait(
                    lambda: any(
                        i["id"] == agent["id"] and i["attached"] for i in rpc({"action": "agents"})
                    ),
                    "Original agent was not reattached",
                )
                ui.click("workspace-tab:Terminal")
                ui.control("terminal-input:" + terminal["id"])
                assert any(i["id"] == terminal["id"] for i in rpc({"action": "list"}))
                ui.project_editor()
                ui.source("Saved original\nUnsaved draft\n")
                assert source.read_text() == "Saved original\n"
                ui.command("Application: Show Welcome")
                ui.click("workspace-session-rename:" + identity)
                ui.edit("workspace-session-name", "Research workspace")
                ui.click("workspace-session-save-name")
                wait(lambda: record()["name"] == "Research workspace", "Rename was not saved")
                # A second window shares storage but has a separate probe and process.
                old_controller = record()["snapshot"]["agent"]["controller"]
                second = dict(env, RODE_UI_TEST_STATE=str(base / "second.json"))
                with native_session(project_args(root), second, root, "takeover") as other:
                    other.click("workspace-session-takeover:" + identity)
                    other.click("workspace-session-confirm")
                    wait(
                        lambda: (
                            record()["snapshot"]["agent"]["controller"]
                            not in (None, old_controller)
                        ),
                        "Matching agent control did not move with takeover",
                    )
                    other.click("workspace-tab:Chat")
                    other.control("agent-composer")
                    other.value("agent-composer-value", "Unsent workspace prompt")
                    other.project_editor()
                    other.source("Saved original\nUnsaved draft\n")
                    focus_window(ui.window)
                    ui.control("welcome-screen")
                    ui.layout(
                        lambda state: all(
                            b["name"] != "workspace-session-kept" for b in state["buttons"]
                        ),
                        "Original owner did not relinquish the workspace",
                    )
                    generation = record()["owner"]["generation"]
                    ui.project_editor()
                    ui.edit("source-editor", "Old window edits remain local")
                    # An explicit filesystem acknowledgement from the successor proves
                    # it still controls the record after the old window edits.
                    focus_window(other.window)
                    other.command("Application: Show Welcome")
                    other.click("workspace-session-rename:" + identity)
                    other.edit("workspace-session-name", "After takeover")
                    other.click("workspace-session-save-name")
                    wait(lambda: record()["name"] == "After takeover", "Successor rename failed")
                    assert record()["owner"]["generation"] == generation
                    assert (
                        record()["snapshot"]["documents"][0]["draft"]["text"]
                        != "Old window edits remain local"
                    )
                    screenshot(other.window, "takeover-workspace.png")
                # This window has deliberately lost Keep ownership; normal dirty guards apply.
                focus_window(ui.window)
                ui.press("source-editor", "a", ("Control_L",))
                ui.edit("source-editor", "Saved original\n")
                ui.command("Application: Show Welcome")
                # Restore changed-on-disk drafts as separate unbound buffers.
            source.write_text("Externally changed source\n")
            with native_session(project_args(root), env, root, "conflict") as ui:
                ui.click("workspace-session-resume:" + identity)
                ui.project_editor()
                ui.source("Saved original\nUnsaved draft\n")
                ui.control("source-editor-modified")
                assert source.read_text() == "Externally changed source\n"
                ui.command("Application: Show Welcome")
                screenshot(ui.window, "restored-conflict.png")
            # A hard-killed kept window offers its saved session, without a second
            # crash-recovery copy of the same buffers blocking Welcome.
            log_path = ARTIFACTS / "crash-kept.log"
            with log_path.open("w") as log:
                process = subprocess.Popen(project_args(root), env=env, stdout=log, stderr=log)
                ui = None
                try:
                    ui = project_ui(process, env, root, log_path)
                    ui.click("workspace-session-resume:" + identity)
                    ui.source("Saved original\nUnsaved draft\n")
                    wait(
                        lambda: not list((base / "data/rode/recovery").glob("s-*.json")),
                        "Named checkpoint did not retire duplicate recovery",
                    )
                    process.kill()
                    process.wait(timeout=8)
                except BaseException:
                    capture_failure(
                        "crash-kept",
                        process,
                        ui.window if ui else None,
                        files=((Path(env["RODE_UI_TEST_STATE"]), ".json"), (log_path, ".log")),
                    )
                    raise
                finally:
                    if process.poll() is None:
                        process.kill()
                        process.wait(timeout=8)
            # Removing a saved workspace is explicit and cannot stop its host agent.
            with native_session(project_args(root), env, root, "remove") as ui:
                ui.absent("recovery-restore")
                ui.click("workspace-session-remove:" + identity)
                ui.click("workspace-session-confirm")
                wait(lambda: not records(), "Saved workspace was not removed")
                assert any(i["id"] == agent["id"] for i in rpc({"action": "agents"}))
            print(
                "Workspace sessions: durable drafts, explicit resume, retained agent/terminal, takeover, conflict, crash and removal passed."
            )
        finally:
            if terminal:
                with contextlib.suppress(Exception):
                    rpc(dict(action="end", id=terminal["id"], policy=terminal["policy"]))
            if agent:
                with contextlib.suppress(Exception):
                    rpc(dict(action="end_agent", id=agent["id"], policy=agent["policy"]))


def remote_smoke():
    """Round-trip protected remote reads through both captured SSH backends."""
    for backend in ("russh", "openssh"):
        with fixture_directory(prefix="ws-remote-") as base:
            root = base / "project"
            root.mkdir()
            (root / "notes.txt").write_text("Remote baseline\n")
            env = fixture_environment(
                base,
                XDG_DATA_HOME=str(base / "data"),
                XDG_CONFIG_HOME=str(base / "config"),
                RODE_UI_TEST_STATE=str(base / "layout.json"),
            )
            args = project_args(root, remote=True) + ["--ssh-backend", backend]
            with native_session(args, env, root, backend + "-capture") as ui:
                ui.open_file("notes.txt")
                ui.edit("source-editor", "Remote unsaved draft")
                ui.command("Application: Show Welcome")
                ui.click("workspace-session-keep")
                ui.control("workspace-session-kept")
                folder = base / "data/rode/workspace-sessions"
                saved = json.loads(next(folder.glob("s-*.json")).read_text())
                identity = saved["id"]
                ui.click("workspace-session-keep")
                ui.absent("workspace-session-kept")
                ui.click("workspace-session-keep")
                ui.control("workspace-session-kept")
                entries = list(folder.glob("s-*.json"))
                assert len(entries) == 1
                assert json.loads(entries[0].read_text())["id"] == identity
            with native_session(args, env, root, backend + "-resume") as ui:
                ui.control("welcome-screen")
                ui.click("workspace-session-resume:" + identity)
                ui.source("Remote unsaved draft")
                ui.control("source-editor-modified")
                assert (root / "notes.txt").read_text() == "Remote baseline\n"
            print(backend + ": workspace session restores remote drafts without overwriting files.")


if __name__ == "__main__":
    if os.environ.get("RODE_WORKSPACE_SESSIONS_REMOTE") == "1":
        remote_smoke()
    else:
        run()
