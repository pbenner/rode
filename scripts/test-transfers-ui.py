#!/usr/bin/env python3
"""Real native local/SSH transfers, conflict controls and independent navigation."""

import hashlib
import os
from pathlib import Path

os.environ["RODE_UI_ARTIFACTS"] = str(
    Path(__file__).resolve().parent.parent / "target/transfers-ui"
)
from ui_fixture import native_session, project_args
from ui_harness import (
    display,
    fixture_directory,
    fixture_environment,
    key,
    move_pointer,
    screenshot,
    text,
    window_geometry,
    x11,
    xtest,
)


def digest(path):
    return hashlib.sha256(path.read_bytes()).hexdigest() if path.is_file() else None


with fixture_directory(prefix="rode-transfers-ui-") as base:
    local, remote, downloads = (base / name for name in ("local", "remote", "downloads"))
    for directory in (local, remote, downloads):
        directory.mkdir()
    payload = bytes(range(256)) * (12 * 1024 + 7)
    source = local / "payload-日本.bin"
    source.write_bytes(payload)
    (local / "folder").mkdir()
    (local / "folder/empty").mkdir()
    (local / "folder/note.txt").write_text("nested file\n")
    for name in ("drag-note.txt", "batch-a.txt", "batch-b.txt"):
        (local / name).write_text(name + "\n")
    expected = hashlib.sha256(payload).hexdigest()
    current_project = remote
    environment = fixture_environment(base)
    with native_session(project_args(remote, remote=True), environment, remote, "transfers") as ui:
        ui.command("Remote: File transfers")

        def browse(side, directory):
            name = f"transfer-path:{side}"
            ui.click(name)
            ui.fill(name, str(directory))
            prefix = f"transfer-listing:{side}:"
            previous = {b["name"] for b in ui.layout()["buttons"] if b["name"].startswith(prefix)}
            ui.click(f"transfer-Go:{side}")
            ui.layout(
                lambda state: any(
                    b["name"].startswith(prefix) and b["name"] not in previous
                    for b in state["buttons"]
                ),
                "Fresh directory listing was not acknowledged",
            )
            ui.value(f"transfer-directory:{side}", str(directory))

        def select(path):
            ui.click("file-picker-row:" + hashlib.sha256(str(path).encode()).hexdigest())

        def drag(path, side, folder=None):
            source = "file-picker-row:" + hashlib.sha256(str(path).encode()).hexdigest()
            start = ui.control(source, hovered=True)
            xtest.XTestFakeButtonEvent(display, 1, 1, 0)
            x11.XFlush(display)
            try:
                move_pointer(start[0] + 12, start[1])
                ui.control("transfer-dragging")
                state = ui.layout()
                name = (
                    "file-picker-row:" + hashlib.sha256(str(folder).encode()).hexdigest()
                    if folder
                    else f"transfer-drop:{side}"
                )
                rect = next(button["rect"] for button in state["buttons"] if button["name"] == name)
                origin_x, origin_y, _, _ = window_geometry(ui.window)
                scale = state["pixels_per_point"]
                # The lower blank list area targets the current folder, not a row.
                x = (rect[0] + rect[2]) / 2
                y = (rect[1] + rect[3]) / 2 if folder else rect[3] - 8
                move_pointer(round(origin_x + x * scale), round(origin_y + y * scale))
                ui.control(f"transfer-drop-ready:{side}")
                screenshot(ui.window, "dragging.png")
            finally:
                xtest.XTestFakeButtonEvent(display, 1, 0, 0)
                x11.XFlush(display)

        def unchanged_project():
            assert ui.layout()["project"]["root"] == str(current_project)

        def copied(path, checksum):
            ui.wait(
                lambda: digest(path) == checksum, f"Transfer did not publish {path}", timeout=30
            )
            ui.control("transfer-idle")

        browse(0, local)
        ui.value("transfer-directory:1", str(remote))
        selected = "file-picker-checked:" + hashlib.sha256(str(source).encode()).hexdigest()
        select(source)
        ui.control(selected)
        screenshot(ui.window, "selection-list.png")
        key("space")
        ui.absent(selected)
        for side in (0, 1):
            ui.click(f"transfer-view-icons:{side}")
            ui.control(f"transfer-view:{side}:icons")
        select(source)
        ui.control(selected)
        screenshot(ui.window, "selection-icons.png")
        # Move the live browser to its sole native host, then return via its header.
        root_ui = ui
        ui.click("activity:File transfers", mouse_button=3)
        ui.click("area-detach")
        ui = root_ui.child_window("RODE · Transfers")
        ui.control(selected)
        ui.control("transfer-view:0:icons")
        ui.value("transfer-directory:0", str(local))
        ui.value("transfer-directory:1", str(remote))
        screenshot(ui.window, "detached-transfers.png")
        select(source)
        ui.absent(selected)
        screenshot(ui.window, "icon-views.png")
        drag(source, 1)
        copied(remote / source.name, expected)
        ui.click("workspace-reattach:Transfers")
        ui = root_ui
        ui.control("transfer-view:0:icons")
        ui.value("transfer-directory:0", str(local))
        ui.click("activity:Projects")
        ui.control(f"explorer-row:{remote / source.name}")
        ui.click("activity:File transfers")
        assert source.read_bytes() == payload
        unchanged_project()
        screenshot(ui.window, "uploaded.png")

        # Download a whole binary stream through the other direction and channel.
        browse(0, downloads)
        drag(remote / source.name, 0)
        copied(downloads / source.name, expected)
        unchanged_project()

        for side in (0, 1):
            ui.click(f"transfer-view-list:{side}")
            ui.control(f"transfer-view:{side}:list")

        # Folder copies preserve nesting and empty directories.
        browse(0, local)
        select(local / "folder")
        ui.click("transfer-upload")
        ui.wait(lambda: (remote / "folder/empty").is_dir(), "Empty directory missing")
        ui.wait(
            lambda: (
                (remote / "folder/note.txt").is_file()
                and (remote / "folder/note.txt").read_text() == "nested file\n"
            ),
            "Nested file was not copied",
        )
        ui.control("transfer-idle")

        # Drop onto a folder without changing the displayed destination.
        drag(local / "drag-note.txt", 1, remote / "folder")
        copied(remote / "folder/drag-note.txt", digest(local / "drag-note.txt"))
        ui.value("transfer-directory:1", str(remote))
        browse(0, local)
        for name in ("batch-a.txt", "batch-b.txt"):
            select(local / name)
        drag(local / "batch-a.txt", 1)
        for name in ("batch-a.txt", "batch-b.txt"):
            copied(remote / name, digest(local / name))
        unchanged_project()

        # Existing destinations require a real choice; cancellation preserves them.
        browse(0, local)
        select(source)
        ui.click("transfer-upload")
        ui.control("transfer-conflict-Skip")
        screenshot(ui.window, "conflict.png")
        ui.click("transfer-conflict-cancel")
        ui.control("transfer-idle")
        assert digest(remote / source.name) == expected

        browse(0, local)
        select(source)
        ui.click("transfer-upload")
        ui.click("transfer-conflict-Skip")
        ui.control("transfer-idle")
        assert digest(remote / source.name) == expected

        replacement = b"new binary\x00contents" * 300_000
        source.write_bytes(replacement)
        browse(0, local)
        select(source)
        ui.click("transfer-upload")
        ui.click("transfer-conflict-Replace")
        copied(remote / source.name, hashlib.sha256(replacement).hexdigest())
        unchanged_project()
        assert not list(remote.glob(".rode-transfer-*.part"))
        screenshot(ui.window, "completed.png")
        ui.resize(850, 560)
        ui.control("transfer-path:0")
        ui.control("transfer-path:1")
        ui.layout(
            lambda state: {"transfer-upload", "transfer-download", "transfer-idle"}.issubset(
                {button["name"] for button in state["buttons"]}
            ),
            "Compact layout hid transfer controls",
        )
        screenshot(ui.window, "compact.png")
        ui.resize(1380, 900)

        def switch_project(directory):
            ui.command("Open project folder")
            ui.control("project-dialog-ready")
            ui.click("project-dialog-path")
            before = ui.layout()["pass"]
            key("a", ("Control_L",))
            text(str(directory))
            ui.control("project-dialog-path", focused=True, changed_after=before)
            ui.copy("project-dialog-path", str(directory))
            ui.click("project-dialog-browse")
            ui.control("project-dialog-ready")
            ui.click("project-dialog-open")
            ui.absent("project-dialog-path")
            ui.layout(
                lambda state: (
                    (state.get("project") or {}).get("ready")
                    and state["project"]["root"] == str(directory)
                ),
                "Project did not switch on the existing SSH connection",
            )
            ui.click("activity:File transfers")

        # Project grants expire with the project; they never silently widen.
        current_project = base / "second-project"
        current_project.mkdir()
        switch_project(current_project)
        ui.layout(
            lambda state: (
                not any(b["name"].startswith("transfer-listing:1:") for b in state["buttons"])
            ),
            "Expired project transfer policy retained an actionable listing",
        )
        ui.click("transfer-access-menu")
        ui.click("transfer-use-project-access")
        ui.value("transfer-directory:1", str(current_project))
        ui.control("transfer-scope-project")

        # Host access requires an explicit selection, then survives project changes.
        ui.click("transfer-access-menu")
        ui.click("transfer-use-host-access")
        ui.control("transfer-scope-host")
        browse(1, remote)
        current_project = base / "third-project"
        current_project.mkdir()
        switch_project(current_project)
        ui.control("transfer-scope-host")
        ui.value("transfer-directory:1", str(remote))
        browse(0, local)
        (local / "host-scope.txt").write_text("explicit host access\n")
        browse(0, local)
        select(local / "host-scope.txt")
        ui.click("transfer-upload")
        copied(remote / "host-scope.txt", digest(local / "host-scope.txt"))
        unchanged_project()

        # The feature gate hides all entry points without stopping the connection.
        ui.command("Extensions: Manage installed extensions")
        ui.click("extension-filter")
        ui.fill("extension-filter", "rode.transfers")
        ui.click("extension-toggle:rode.transfers")
        key("Escape")
        ui.absent("extension-filter")
        ui.absent("activity:File transfers")
        ui.absent("open-transfers")
        unchanged_project()

print(
    "Transfers: binary upload/download, folders, conflicts, cancellation, feature gating and independent project navigation passed."
)
