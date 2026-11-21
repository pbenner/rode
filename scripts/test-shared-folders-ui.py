#!/usr/bin/env python3
"""Native shared-folder UI, both SSH clients, kernel mounts and bookmark settings."""

import os
from pathlib import Path

os.environ["RODE_UI_ARTIFACTS"] = str(
    Path(__file__).resolve().parent.parent / "target/shared-folders-ui"
)
from test_capabilities import skip
from ui_fixture import native_session, project_args
from ui_harness import fixture_directory, fixture_environment, key, screenshot


def fill(ui, name, value):
    ui.click(name)
    ui.fill(name, str(value))


for backend in ("russh", "openssh"):
    with fixture_directory(prefix="rode-shares-") as base:
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
        with native_session(args, env, project, backend) as ui:
            ui.command("Remote: File transfers")
            ui.click("shared-folders-toggle")
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
                destination.mkdir()
                ui.click("share-add")
                if reverse:
                    ui.click("share-direction")
                    ui.click("share-remote-to-local")
                fill(ui, "share-source", source)
                fill(ui, "share-destination", destination)
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
            ui.click("bookmark-shares:" + identity)
            key("Escape")
        # Settings persist but must never start mounts merely by loading a bookmark.
        with native_session(args, env, project, backend + "-restored") as ui:
            ui.click("activity:Connections")
            ui.click(bookmark)
            ui.command("Remote: File transfers")
            ui.control("share-add")
            for index in range(1, 5):
                ui.control(f"share-mount:{index}")
                ui.value(f"share-status:{index}", "Not mounted")
            screenshot(ui.window, backend + "-restored.png")
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
