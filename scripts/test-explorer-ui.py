#!/usr/bin/env python3
"""Native explorer menus, project guards and host-backed file operations; no inference."""

import json
import os
import subprocess
from pathlib import Path

os.environ["RODE_UI_ARTIFACTS"] = str(Path(__file__).resolve().parent.parent / "target/explorer-ui")
from ui_harness import (
    ARTIFACTS,
    BINARY,
    NativeUI,
    close_window,
    fixture_directory,
    fixture_environment,
    focus_window,
    key,
    palette_command,
    screenshot,
    text,
    wait_for_project,
    wait_for_window,
)

with fixture_directory(prefix="rode-explorer-ui-") as base:
    root = base / "project"
    root.mkdir()
    (root / "another-project").mkdir()
    (root / "another-project/README.md").write_text("another project\n")
    state = base / "ui-state.json"
    environment = fixture_environment(
        base,
        XDG_CONFIG_HOME=str(base / "config"),
        XDG_DATA_HOME=str(base / "data"),
        RODE_UI_TEST_STATE=str(state),
    )
    backend = os.environ.get("RODE_TEST_SANDBOX_BACKEND")
    if backend:
        config = base / "data/rode"
        config.mkdir(parents=True)
        (config / "app.ron").write_text(
            json.dumps({"sandbox_defaults": json.dumps({"enabled": True, "backend": backend})})
        )
    with (ARTIFACTS / "explorer.log").open("w") as log:
        remote = os.environ.get("RODE_EXPLORER_UI_REMOTE") == "1"
        arguments = [str(BINARY), str(root), "--theme", "studio-neutral"]
        if remote:
            arguments = [
                str(BINARY),
                "--ssh",
                "rode-test",
                "--ssh-config",
                os.environ["RODE_TEST_SSH_CONFIG"],
                "--remote-path",
                str(root),
                "--theme",
                "studio-neutral",
            ]
        if backend:
            arguments.append("--sandbox")
        process = subprocess.Popen(
            arguments,
            env=environment,
            stdout=log,
            stderr=log,
        )
        window = None
        try:
            window = wait_for_window(process, log.name)
            wait_for_project(window, process, log.name, root)
            focus_window(window)
            ui = NativeUI(window, process, log.name, state)
            point, button, wait = ui.control, ui.click, ui.wait
            if (
                os.environ.get("RODE_TEST_REQUIRE_SANDBOX") == "1"
                or os.environ.get("RODE_TEST_REQUIRE_LANDLOCK") == "1"
            ):
                title = subprocess.check_output(["xwininfo", "-id", hex(window)], text=True)
                assert "[Sandbox]" in title, title

            def idle():
                point("explorer-refresh-expanded")

            def menu(path=None):
                idle()
                target = root if path is None else path
                button(
                    "explorer-root" if path is None else "explorer-row:" + str(path),
                    mouse_button=3,
                )
                point("explorer-menu:" + str(target))

            def named(value):
                ui.edit("explorer-name", value)
                button("explorer-apply")
                ui.absent("explorer-apply")

            if remote:
                palette_command(window, process, log.name, state, "Toggle explorer")
            menu()
            screenshot(window, "root-menu.png")
            key("Escape")
            ui.absent("explorer-new-file")
            menu()
            button("explorer-new-folder")
            named("created")
            folder = root / "created"
            wait(folder.is_dir, "New folder did not reach the project host")
            menu(folder)
            button("explorer-new-file")
            named("child.txt")
            child = folder / "child.txt"
            wait(child.is_file, "New file did not reach the project host")
            point("source-editor", focused=True)
            text("unsaved contents")

            menu(folder)
            button("explorer-rename")
            named("renamed")
            moved = root / "renamed"
            wait(moved.is_dir, "Folder rename did not finish")
            idle()
            button("source-editor")
            key("s", ("Control_L",))
            saved = moved / "child.txt"
            wait(
                lambda: saved.read_text() == "unsaved contents",
                "Rename lost the open buffer or saved to its old path",
            )
            assert not folder.exists()

            menu(moved)
            screenshot(window, "folder-menu.png")
            button("explorer-duplicate")
            named("duplicate")
            duplicate = root / "duplicate"
            wait(lambda: (duplicate / "child.txt").is_file(), "Directory duplicate did not finish")
            assert (duplicate / "child.txt").read_text() == "unsaved contents"

            idle()
            button("explorer-row:" + str(moved))
            menu(saved)
            screenshot(window, "file-menu.png")
            button("explorer-copy-relative")
            wait(
                lambda: ui.clipboard() == "renamed/child.txt",
                "Relative path was not copied",
            )
            menu(saved)
            button("explorer-rename")
            named("renamed.txt")
            renamed = moved / "renamed.txt"
            wait(renamed.is_file, "File rename did not finish")
            idle()
            button("source-editor")
            key("End", ("Control_L",))
            text(" kept after delete")
            menu(renamed)
            button("explorer-delete")
            screenshot(window, "delete-confirmation.png")
            button("explorer-confirm-delete:" + str(renamed))
            ui.absent("explorer-apply")
            wait(lambda: not renamed.exists(), "File deletion did not finish")
            # A retained editor still owns the deleted path. A new file must
            # not reopen that old draft or acquire a duplicate path binding.
            menu(moved)
            button("explorer-new-file")
            ui.edit("explorer-name", "renamed.txt")
            ui.layout(
                lambda layout: any(
                    button["name"] == "explorer-apply" and not button["enabled"]
                    for button in layout["buttons"]
                ),
                "Creating a new file reused the retained editor's path",
            )
            button("explorer-cancel")
            ui.absent("explorer-apply")
            assert not renamed.exists()
            idle()
            button("source-editor")
            key("s", ("Control_L", "Shift_L"))
            ui.edit("file-dialog-path", str(root / "rescued.txt"))
            ui.press("file-dialog-path", "Return")
            ui.absent("file-dialog-path")
            wait(
                lambda: (root / "rescued.txt").is_file(),
                "Deleted file's open buffer could not be saved separately",
            )
            assert (root / "rescued.txt").read_text() == "unsaved contents kept after delete"
            assert not renamed.exists()

            menu(duplicate)
            button("explorer-delete")
            ui.press("explorer-confirm-delete:" + str(duplicate), "Return")
            ui.absent("explorer-apply")
            wait(lambda: not duplicate.exists(), "Recursive folder deletion did not finish")
            idle()
            button("source-editor")
            key("End", ("Control_L",))
            text(" dirty")
            menu(root / "another-project")
            button("explorer-open-project")
            point("project-dialog-path", focused=True)
            key("a", ("Control_L",))
            key("c", ("Control_L",))
            wait(
                lambda: ui.clipboard() == str(root / "another-project"),
                "The project dialog did not prefill the selected folder",
            )
            ui.layout(
                lambda layout: (
                    not any(item["name"].startswith("guard-") for item in layout["buttons"])
                ),
                "Project switching began before confirming its options",
            )
            screenshot(window, "project-options.png")
            button("project-dialog-cancel")
            ui.absent("project-dialog-path")
            wait_for_project(window, process, log.name, root)
            menu(root / "another-project")
            button("explorer-open-project")
            button("project-dialog-open")
            button("guard-keep-editing")
            wait_for_project(window, process, log.name, root)
            menu(root / "another-project")
            button("explorer-open-project")
            button("project-dialog-open")
            button("guard-discard")
            wait_for_project(window, process, log.name, root / "another-project")
            idle()
            if remote:
                assert "SSH" in subprocess.check_output(
                    ["xwininfo", "-id", hex(window)], text=True
                ), "Opening a folder switched away from the SSH host"
            screenshot(window, "opened-child-project.png")
            close_window(window, process)
            print(
                "Explorer: native context menus, file operations, prefilled project options, cancellation and guarded local/SSH project opening passed."
            )
        except Exception:
            if state.exists():
                (ARTIFACTS / "failure-state.json").write_bytes(state.read_bytes())
            if window is not None and process.poll() is None:
                screenshot(window, "failure.png")
            raise
        finally:
            if process.poll() is None:
                process.terminate()
                process.wait(timeout=5)
