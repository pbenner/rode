#!/usr/bin/env python3
"""Git change inspection and undoable hunk reverts, including unsaved text and host policy."""

import json
import os
import shutil
import subprocess
from pathlib import Path

remote = os.environ.get("RODE_GIT_GUTTER_UI_REMOTE") == "1"
variant = ("ssh" if remote else "local") + (
    "-" + os.environ["RODE_TEST_SANDBOX_BACKEND"]
    if os.environ.get("RODE_TEST_SANDBOX_BACKEND")
    else ""
)
os.environ["RODE_UI_ARTIFACTS"] = str(
    Path(__file__).resolve().parent.parent / "target/git-gutter-ui" / variant
)
from ui_harness import (
    ARTIFACTS,
    BINARY,
    NativeUI,
    close_window,
    fixture_directory,
    fixture_environment,
    focus_window,
    key,
    screenshot,
    text,
    wait_for_project,
    wait_for_window,
)

with fixture_directory(prefix="rode-git-gutter-ui-") as base:
    root = base / "project"
    root.mkdir()
    state = base / "ui-state.json"
    environment = fixture_environment(
        base,
        RODE_UI_TEST_STATE=str(state),
        GIT_CONFIG_NOSYSTEM="1",
        GIT_CONFIG_GLOBAL=str(base / "empty-gitconfig"),
    )

    def git(*args):
        return subprocess.check_output(
            ["git", "-C", str(root), *args], env=environment, stderr=subprocess.STDOUT, timeout=10
        )

    git("init", "-q")
    git("config", "user.name", "Gutter fixture")
    git("config", "user.email", "fixture@example.invalid")
    git("config", "core.autocrlf", "input")
    (root / "empty.txt").write_text("removed line\n")
    (root / "crlf.txt").write_bytes(b"one\r\ntwo\r\n")
    path = root / "source changes.txt"
    path.write_text("one\nold\nkeep\ndelete\nlast\n")
    (root / ".gitignore").write_text("ignored.txt\n")
    long_source = [f"let line_{n} = {n};\n" for n in range(1, 241)]
    (root / "long.rs").write_text("".join(long_source))
    git("add", ".")
    git("commit", "-qm", "Initial")
    (root / "empty.txt").write_text("")
    for line in [0, 35, 64, 199]:
        long_source[line] = f"let line_{line + 1} = 999;\n"
    (root / "long.rs").write_text("".join(long_source))
    staged = "one\nchanged\nkeep\nlast\nextra\n"
    path.write_text(staged)
    git("add", "--", path.name)
    (root / "new.txt").write_text("untracked\n")
    (root / "ignored.txt").write_text("ignored\n")
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
    backend = os.environ.get("RODE_TEST_SANDBOX_BACKEND")
    if backend:
        preferences = base / "data/rode/app.ron"
        preferences.parent.mkdir(parents=True)
        preferences.write_text(
            json.dumps(
                {
                    "sandbox_defaults": json.dumps({"enabled": True, "backend": backend}),
                }
            )
        )
        arguments.append("--sandbox")
    with (ARTIFACTS / "gutter.log").open("w") as log:
        process = subprocess.Popen(arguments, env=environment, stdout=log, stderr=log)
        window = None
        try:
            window = wait_for_window(process, log.name)
            wait_for_project(window, process, log.name, root)
            if (
                os.environ.get("RODE_TEST_REQUIRE_SANDBOX") == "1"
                or os.environ.get("RODE_TEST_REQUIRE_LANDLOCK") == "1"
            ):
                title = subprocess.check_output(["xwininfo", "-id", hex(window)], text=True)
                assert "[Sandbox]" in title, title
            focus_window(window)
            ui = NativeUI(window, process, log.name, state)
            ui.open_file(path)

            def markers(expected):
                def matches(layout):
                    names = {button["name"] for button in layout["buttons"]}
                    return "source-git-changes-ready" in names and {
                        name for name in names if name.startswith("git-change-")
                    } == set(expected)

                return ui.layout(matches, f"Git gutter did not become {expected}", timeout=20)

            staged_layout = markers(
                ["git-change-modified:2", "git-change-deleted:4", "git-change-added:5"]
            )
            screenshot(window, "staged-changes.png")
            modified_patch = "@@ -2,1 +2,1 @@\n-old\n+changed\n"
            deleted_patch = "@@ -4,1 +3,0 @@\n-delete\n"
            added_patch = "@@ -5,0 +5,1 @@\n+extra\n"
            staged_index = git("diff", "--cached", "--binary")
            ui.click("git-change-modified:2")
            ui.value("git-peek-patch", modified_patch)
            screenshot(window, "change-preview.png")
            ui.click("git-peek-diff")
            ui.copy("git-peek-diff", modified_patch)
            text("READ_ONLY")
            ui.value("git-peek-patch", modified_patch)
            ui.click("source-editor")
            ui.source(staged)
            assert path.read_text() == staged
            assert git("diff", "--cached", "--binary") == staged_index
            ui.resize(850, 560)
            ui.control("git-peek-close")
            ui.control("git-peek-revert")
            ui.value("git-peek-patch", modified_patch)
            screenshot(window, "change-preview-compact.png")
            ui.resize(1380, 900)
            ui.click("git-peek-close")

            def revert_block(marker, restored, remaining):
                ui.click(marker)
                ui.click("git-peek-revert")
                ui.source(restored)
                ui.control("source-editor", focused=True)
                ui.control("source-editor-modified")
                ui.absent("git-peek-diff")
                markers(remaining)
                assert path.read_text() == staged, "Revert wrote the file without Save"
                assert git("diff", "--cached", "--binary") == staged_index
                key("z", ("Control_L",))
                ui.source(staged)
                ui.control("source-editor-saved")
                key("y", ("Control_L",))
                ui.source(restored)
                key("z", ("Control_L",))
                ui.source(staged)
                markers(["git-change-modified:2", "git-change-deleted:4", "git-change-added:5"])

            revert_block(
                "git-change-modified:2",
                "one\nold\nkeep\nlast\nextra\n",
                ["git-change-deleted:4", "git-change-added:5"],
            )
            revert_block(
                "git-change-deleted:4",
                "one\nchanged\nkeep\ndelete\nlast\nextra\n",
                ["git-change-modified:2", "git-change-added:6"],
            )
            revert_block(
                "git-change-added:5",
                "one\nchanged\nkeep\nlast\n",
                ["git-change-modified:2", "git-change-deleted:4"],
            )
            ui.click("git-change-modified:2")
            ui.value("git-peek-patch", modified_patch)
            ui.click("git-peek-next")
            ui.value("git-peek-patch", deleted_patch)
            ui.control("source-editor-cursor:4:1", focused=True)
            ui.click("git-peek-next")
            ui.value("git-peek-patch", added_patch)
            ui.control("source-editor-cursor:5:1", focused=True)
            ui.click("git-peek-previous")
            ui.value("git-peek-patch", deleted_patch)
            key("Escape")
            ui.absent("git-peek-diff")
            ui.control("source-editor", focused=True)
            ui.click("git-change-deleted:4")
            ui.value("git-peek-patch", deleted_patch)
            ui.click("git-change-deleted:4")
            ui.absent("git-peek-diff")
            ui.click("git-change-added:5")
            ui.value("git-peek-patch", added_patch)
            ui.click("source-editor")
            ui.fill("source-editor", "one\nchanged\nkeep\nlast\nextra\nunsaved\n")
            ui.absent("git-peek-diff")
            unsaved_layout = markers(
                ["git-change-modified:2", "git-change-deleted:4", "git-change-added:5"]
            )

            def added_height(layout):
                rect = next(
                    b["rect"] for b in layout["buttons"] if b["name"] == "git-change-added:5"
                )
                return rect[3] - rect[1]

            assert added_height(unsaved_layout) > added_height(staged_layout), (
                "Unsaved line was not decorated"
            )
            assert path.read_text() == staged, "Gutter computation wrote the unsaved buffer"
            ui.click("git-change-added:5")
            ui.value("git-peek-patch", "@@ -5,0 +5,2 @@\n+extra\n+unsaved\n")
            ui.click("git-peek-revert")
            ui.source("one\nchanged\nkeep\nlast\n")
            ui.control("source-editor", focused=True)
            assert path.read_text() == staged
            assert git("diff", "--cached", "--binary") == staged_index
            key("z", ("Control_L",))
            ui.source("one\nchanged\nkeep\nlast\nextra\nunsaved\n")
            markers(["git-change-modified:2", "git-change-deleted:4", "git-change-added:5"])
            ui.click("git-change-added:5")
            ui.click("git-peek-close")
            ui.control("source-editor", focused=True)
            key("z", ("Control_L",))
            ui.source(staged)
            markers(["git-change-modified:2", "git-change-deleted:4", "git-change-added:5"])
            ui.click("git-change-modified:2")
            ui.value("git-peek-patch", modified_patch)
            git("commit", "-qm", "Staged changes")
            markers([])
            ui.absent("git-peek-diff")
            ui.open_file(root / "crlf.txt")
            markers([])
            ui.open_file(root / "empty.txt")
            markers(["git-change-deleted:1"])
            ui.click("git-change-deleted:1")
            ui.value("git-peek-patch", "@@ -1,1 +0,0 @@\n-removed line\n")
            ui.click("git-peek-revert")
            ui.source("removed line\n")
            ui.control("source-editor", focused=True)
            assert (root / "empty.txt").read_text() == ""
            key("z", ("Control_L",))
            ui.source("")
            ui.open_file(root / "long.rs")
            ui.absent("git-peek-diff")
            markers(["git-change-modified:1", "git-change-modified:36"])
            ui.click("git-change-modified:36")
            ui.value(
                "git-peek-patch", "@@ -36,1 +36,1 @@\n-let line_36 = 36;\n+let line_36 = 999;\n"
            )
            # Opening the preview shrinks the source viewport. The selected
            # change must remain visible after that layout and source jump.
            ui.control("source-editor-cursor:36:1", focused=True)
            ui.control("git-change-modified:36")
            screenshot(window, "long-source-peek.png")
            ui.click("git-peek-close")
            ui.control("source-editor", focused=True)
            ui.click("source-editor")
            key("End", ("Control_L",))
            markers(["git-change-modified:200"])
            screenshot(window, "long-source-scrolled.png")
            key("Home", ("Control_L",))
            markers(["git-change-modified:1", "git-change-modified:36"])
            ui.open_file(root / "new.txt")
            markers(["git-change-added:1"])
            ui.click("git-change-added:1")
            ui.value("git-peek-patch", "@@ -0,0 +1,1 @@\n+untracked\n")
            ui.click("git-peek-revert")
            ui.source("")
            ui.control("source-editor", focused=True)
            assert (root / "new.txt").read_text() == "untracked\n"
            key("z", ("Control_L",))
            ui.source("untracked\n")
            ui.open_file(root / "ignored.txt")
            ui.absent("git-peek-diff")
            ui.control("source-git-unavailable")
            ui.layout(
                lambda layout: all(
                    not b["name"].startswith("git-change-") for b in layout["buttons"]
                ),
                "Ignored file retained a previous document's markers",
            )
            ui.open_file(path)
            markers([])
            (root / ".git").rename(root / "repository-metadata")
            ui.control("source-git-unavailable")
            close_window(window, process)
            window = None
            print(
                "Git gutter: change previews, isolated hunk reverts, native undo/redo, unchanged files/index, staged/unsaved edits, commit, untracked/ignored and missing repository passed."
            )
        except Exception:
            if window is not None and process.poll() is None:
                screenshot(window, "failure.png")
            if state.exists():
                shutil.copyfile(state, ARTIFACTS / "failure-layout.json")
            raise
        finally:
            if process.poll() is None:
                process.kill()
                process.wait(timeout=5)
