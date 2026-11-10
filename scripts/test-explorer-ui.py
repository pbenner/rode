#!/usr/bin/env python3
"""Native explorer menus, project guards and host-backed file operations; no inference."""

import json
import os
import subprocess
import tempfile
import time
from pathlib import Path

os.environ["RODE_UI_ARTIFACTS"] = str(Path(__file__).resolve().parent.parent / "target/explorer-ui")
from ui_harness import (
    ARTIFACTS,
    BINARY,
    click,
    click_button,
    close_window,
    display,
    focus_window,
    key,
    palette_command,
    screenshot,
    text,
    wait_for_button,
    wait_for_project,
    wait_for_ui_state,
    wait_for_window,
    x11,
    xtest,
)


def wait(check, message):
    deadline = time.monotonic() + 12
    while time.monotonic() < deadline:
        assert process.poll() is None, "RODE exited: " + log.name
        if check():
            return
        time.sleep(0.05)
    raise AssertionError(message)


def clipboard_matches(expected):
    """An absent owner is normal until the native Copy action reaches its frame."""
    try:
        result = subprocess.run(
            ["xclip", "-selection", "clipboard", "-o"],
            capture_output=True,
            text=True,
            timeout=1,
        )
    except subprocess.TimeoutExpired:
        return False
    return result.returncode == 0 and result.stdout == expected


with tempfile.TemporaryDirectory(prefix="rode-explorer-ui-") as directory:
    base = Path(directory)
    root = base / "project"
    root.mkdir()
    (root / "another-project").mkdir()
    (root / "another-project/README.md").write_text("another project\n")
    state = base / "ui-state.json"
    environment = dict(
        os.environ,
        XDG_CONFIG_HOME=str(base / "config"),
        XDG_DATA_HOME=str(base / "data"),
        RODE_UI_TEST_STATE=str(state),
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

            def point(name, **guards):
                return wait_for_button(window, process, log.name, state, name, **guards)

            def button(name):
                click_button(window, process, log.name, state, name)

            def idle():
                point("explorer-refresh-expanded")

            def menu(path=None):
                idle()
                location = point("explorer-root" if path is None else "explorer-row:" + str(path))
                xtest.XTestFakeMotionEvent(display, -1, *location, 0)
                xtest.XTestFakeButtonEvent(display, 3, 1, 0)
                xtest.XTestFakeButtonEvent(display, 3, 0, 0)
                x11.XFlush(display)

            def named(value):
                point("explorer-name", focused=True)
                key("a", ("Control_L",))
                text(value)
                button("explorer-apply")

            if remote:
                palette_command(window, process, log.name, state, "Toggle explorer")
            menu()
            screenshot(window, "root-menu.png")
            key("Escape")
            wait(
                lambda: (
                    not any(
                        button["name"] == "explorer-new-file"
                        for button in json.loads(state.read_text())["buttons"]
                    )
                ),
                "Explorer consumed Escape instead of closing its context menu",
            )
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
            click(*point("source-editor"))
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
            click(*point("explorer-row:" + str(moved)))
            menu(saved)
            screenshot(window, "file-menu.png")
            button("explorer-copy-relative")
            wait(
                lambda: clipboard_matches("renamed/child.txt"),
                "Relative path was not copied",
            )
            menu(saved)
            button("explorer-rename")
            named("renamed.txt")
            renamed = moved / "renamed.txt"
            wait(renamed.is_file, "File rename did not finish")
            idle()
            click(*point("source-editor"))
            key("End", ("Control_L",))
            text(" kept after delete")
            menu(renamed)
            button("explorer-delete")
            screenshot(window, "delete-confirmation.png")
            button("explorer-apply")
            wait(lambda: not renamed.exists(), "File deletion did not finish")
            # A retained editor still owns the deleted path. A new file must
            # not reopen that old draft or acquire a duplicate path binding.
            menu(moved)
            button("explorer-new-file")
            point("explorer-name", focused=True)
            key("a", ("Control_L",))
            text("renamed.txt")
            wait(
                lambda: any(
                    button["name"] == "explorer-apply" and not button["enabled"]
                    for button in json.loads(state.read_text())["buttons"]
                ),
                "Creating a new file reused the retained editor's path",
            )
            button("explorer-cancel")
            assert not renamed.exists()
            idle()
            click(*point("source-editor"))
            key("s", ("Control_L", "Shift_L"))
            point("file-dialog-path", focused=True)
            key("a", ("Control_L",))
            text(str(root / "rescued.txt"))
            key("Return")
            wait(
                lambda: (root / "rescued.txt").is_file(),
                "Deleted file's open buffer could not be saved separately",
            )
            assert (root / "rescued.txt").read_text() == "unsaved contents kept after delete"
            assert not renamed.exists()

            menu(duplicate)
            button("explorer-delete")
            point("explorer-apply", focused=True)
            key("Return")
            wait(lambda: not duplicate.exists(), "Recursive folder deletion did not finish")
            idle()
            click(*point("source-editor"))
            key("End", ("Control_L",))
            text(" dirty")
            menu(root / "another-project")
            button("explorer-open-project")
            point("project-dialog-path", focused=True)
            key("a", ("Control_L",))
            key("c", ("Control_L",))
            wait(
                lambda: clipboard_matches(str(root / "another-project")),
                "The project dialog did not prefill the selected folder",
            )
            assert not any(
                item["name"].startswith("guard-")
                for item in json.loads(state.read_text())["buttons"]
            ), "Project switching began before confirming its options"
            screenshot(window, "project-options.png")
            button("project-dialog-cancel")
            wait_for_ui_state(
                process,
                log.name,
                state,
                lambda layout: all(
                    item["name"] != "project-dialog-path" for item in layout["buttons"]
                ),
                "Cancelling did not close the project dialog",
            )
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
