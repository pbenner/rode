#!/usr/bin/env python3
"""Native shared-folder UI, both SSH clients, kernel mounts and bookmark settings."""

import contextlib
import hashlib
import os
from pathlib import Path

os.environ["RODE_UI_ARTIFACTS"] = str(
    Path(__file__).resolve().parent.parent / "target/shared-folders-ui"
)
from fixture_support import cleanup_fixture_mounts
from test_capabilities import skip
from ui_fixture import native_session, project_args
from ui_harness import fixture_directory, fixture_environment, key, screenshot, text


def fill(ui, name, value):
    ui.click(name)
    ui.fill(name, str(value))


def pick_folder(ui, field, folder, *, remote=False, create=False, capture=None):
    """Exercise the shared directory rows, keyboard navigation and captured host."""
    ui.click(field + "-browse")
    ui.value("share-picker-host", "remote" if remote else "local")
    ui.control("share-picker-path")
    fill(ui, "share-picker-path", folder.parent)
    ui.click("share-picker-go")
    ui.value("share-picker-directory", folder.parent)
    if create:
        ui.click("share-picker-new")
        fill(ui, "share-picker-name", folder.name)
        ui.click("share-picker-create")
    else:
        fill(ui, "share-picker-filter", folder.name)
        row = "file-picker-row:" + hashlib.sha256(str(folder).encode()).hexdigest()
        ui.click(row)
        key("Return")
    ui.value("share-picker-directory", folder)
    if capture:
        screenshot(ui.window, capture)
    ui.click("share-picker-select")
    ui.absent("share-picker-select")
    ui.value(field + "-value", folder)


def switch_project(ui, directory):
    """Use the ordinary project picker so live shares pass through transition cleanup."""
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
    project_ready(ui, directory)


def project_ready(ui, directory):
    ui.layout(
        lambda state: (
            (state.get("project") or {}).get("ready") and state["project"]["root"] == str(directory)
        ),
        "The selected project did not become ready",
    )


def restored(ui, recipes, fuse, ids):
    """Require every saved recipe to start without a manual Mount click."""
    # A bookmark can become ready in a frame that still reports the old tabs.
    # Acknowledge a fresh native layout before deciding whether to open tools.
    ui.control("workspace-sidebar", hovered=True)
    # The palette command toggles the panel off if it is already selected.
    ui.click("workspace-tab:Shared folders")
    ui.control("share-add")
    state = ui.layout(
        lambda state: all(
            any(
                button["name"] in (f"share-ready:{identity}", f"share-error:{identity}")
                for button in state["buttons"]
            )
            for identity in ids
        ),
        "Bookmark did not automatically mount all saved shared folders",
        timeout=45,
    )
    for identity, (source, destination) in zip(ids, recipes, strict=True):
        failed = any(button["name"] == f"share-error:{identity}" for button in state["buttons"])
        if failed:
            assert not fuse, "Bookmark mount failed despite available FUSE"
            assert not list(destination.iterdir()), "Failed restoration changed its destination"
        else:
            assert (destination / "sample.txt").read_bytes() == (source / "sample.txt").read_bytes()
    ui.dismiss_notice()


for backend in ("russh", "openssh"):
    with fixture_directory(prefix="rode-shares-") as base, contextlib.ExitStack() as cleanup:
        project = base / "project"
        local = base / "local source ' quoted"
        remote = project / "remote source"
        for folder in (project, local, remote):
            folder.mkdir()
        (local / "sample.txt").write_text("local data\n")
        (remote / "sample.txt").write_text("remote data\n")
        env = fixture_environment(base)
        args = project_args(project, remote=True) + ["--ssh-backend", backend]
        try:
            fd = os.open("/dev/fuse", os.O_RDWR)
            os.close(fd)
            fuse = True
        except OSError:
            fuse = False
        bookmark = None
        skipped_mounts = False
        recipes = []
        cleanup.callback(
            lambda root=base, saved=recipes: cleanup_fixture_mounts(
                root, [path for _, path in saved]
            )
        )
        with native_session(args, env, project, backend) as ui:
            ui.command("Project: Shared folders")
            for index, (source, reverse, writable) in enumerate(
                (
                    (local, False, False),
                    (remote, True, False),
                    (local, False, True),
                    (remote, True, True),
                ),
                1,
            ):
                destination = (base if reverse else project) / f"mount-{index}"
                recipes.append((source, destination))
                if index != 1:
                    destination.mkdir()
                ui.click("share-add")
                if reverse:
                    ui.click("share-direction")
                    ui.click("share-remote-to-local")
                if index == 1:
                    ui.click("share-source-browse")
                    ui.control("share-picker-path")
                    ui.click("share-picker-cancel")
                    ui.absent("share-picker-select")
                    ui.value("share-source-value", "")
                pick_folder(ui, "share-source", source, remote=reverse)
                pick_folder(
                    ui,
                    "share-destination",
                    destination,
                    remote=not reverse,
                    create=index == 1,
                    capture=backend + "-mount-picker.png" if index == 1 else None,
                )
                assert destination.is_dir(), "Picker did not create the mount folder"
                if index == 1:
                    screenshot(ui.window, backend + "-share-form.png")
                if writable:
                    ui.click("share-read-only")
                ui.click("share-save")
                ui.absent("share-source")
                ui.click(f"share-mount:{index}")
                outcome = ui.layout(
                    lambda state: any(
                        b["name"] in (f"share-ready:{index}", f"share-error:{index}")
                        for b in state["buttons"]
                    ),
                    "Share did not mount or report a startup error",
                    timeout=45,
                )
                failed = any(b["name"] == f"share-error:{index}" for b in outcome["buttons"])
                if failed:
                    assert not fuse, "Shared-folder mount failed despite available FUSE"
                    skipped_mounts = True
                    assert not list(destination.iterdir()), "Failed mount changed its destination"
                    continue
                ui.control(f"share-ready:{index}", timeout=45)
                assert (destination / "sample.txt").read_bytes() == (
                    source / "sample.txt"
                ).read_bytes()
                if writable:
                    (destination / f"created-{index}").write_text("written through mount\n")
                    assert (source / f"created-{index}").read_text() == "written through mount\n"
                    (destination / f"created-{index}").rename(destination / f"renamed-{index}")
                    assert (source / f"renamed-{index}").exists()
                    (destination / f"renamed-{index}").unlink()
                else:
                    try:
                        (destination / "denied").write_text("must not be written")
                    except OSError:
                        pass
                    else:
                        raise AssertionError("Read-only share allowed a write")
                    assert not (source / "denied").exists()
                # A normal Unmount must leave an in-use share available.
                with (destination / "sample.txt").open() as held:
                    ui.click(f"share-unmount:{index}")
                    ui.control(f"share-unmount-error:{index}")
                    assert held.read()
                ui.click(f"share-unmount:{index}")
                ui.value(f"share-status:{index}", "Not mounted", timeout=30)
                assert not list(destination.iterdir())
            screenshot(ui.window, backend + "-shares.png")
            ui.click("activity:Connections")
            ui.click("bookmark-project")
            state = ui.layout(
                lambda s: any(b["name"].startswith("saved-project:") for b in s["buttons"]),
                "Bookmark did not appear",
            )
            bookmark = next(
                b["name"] for b in state["buttons"] if b["name"].startswith("saved-project:")
            )
            identity = bookmark.removeprefix("saved-project:")
            ui.click(bookmark, mouse_button=3)
            ui.control("bookmark-shares-update:" + identity)
            key("Escape")
        # Loading preferences is inert; explicitly reopening a bookmark restores its mounts.
        other_project = base / "another-project"
        other_project.mkdir()
        with native_session(
            project_args(other_project), env, other_project, backend + "-restored"
        ) as ui:
            ui.command("Project: Shared folders")
            ui.control("workbench-panel:SHARED FOLDERS")
            assert not any(
                button["name"].startswith(("share-mount:", "share-unmount:"))
                for button in ui.layout()["buttons"]
            ), "Loading the saved-project list started a share"
            assert all(not list(destination.iterdir()) for _, destination in recipes)
            ui.click("activity:Connections")
            ui.click(bookmark)
            project_ready(ui, project)
            restored(ui, recipes, fuse, range(1, 5))
            # Selecting an already visible tab must also work on hosts without FUSE.
            restored(ui, recipes, fuse, range(1, 5))
            if fuse:
                # Current-project reopening must retain the same live rows, without duplicates.
                ui.click(bookmark)
                restored(ui, recipes, fuse, range(1, 5))
            switch_project(ui, other_project)
            assert all(not list(destination.iterdir()) for _, destination in recipes), (
                "Changing projects left old shares mounted"
            )
            ui.click(bookmark)
            project_ready(ui, project)
            restored(ui, recipes, fuse, range(5, 9))
            screenshot(ui.window, backend + "-restored.png")
        assert all(not list(destination.iterdir()) for _, destination in recipes), (
            "Closing RODE left shared folders mounted"
        )
        if skipped_mounts:
            skip(
                "fuse",
                "This host denies /dev/fuse; mount failures and bookmark restoration passed",
                test=f"shared-folders-{backend}",
            )
        coverage = (
            "mount-failure handling"
            if skipped_mounts
            else "kernel mounts, busy unmounts and read/write access"
        )
        print(
            f"Shared folders ({backend}): UI, {coverage}, and bookmark restoration passed",
            flush=True,
        )
