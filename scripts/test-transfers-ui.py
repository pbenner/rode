#!/usr/bin/env python3
"""Real native local/SSH transfers, conflict controls and independent navigation."""

import hashlib
import os
from pathlib import Path

os.environ["RODE_UI_ARTIFACTS"] = str(
    Path(__file__).resolve().parent.parent / "target/transfers-ui"
)
from ui_fixture import native_session, project_args
from ui_harness import fixture_directory, fixture_environment, key, screenshot


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
    expected = hashlib.sha256(payload).hexdigest()
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

        def unchanged_project():
            assert ui.layout()["project"]["root"] == str(remote)

        def copied(path, checksum):
            ui.wait(
                lambda: digest(path) == checksum, f"Transfer did not publish {path}", timeout=30
            )
            ui.control("transfer-idle")

        browse(0, local)
        ui.value("transfer-directory:1", str(remote))
        select(source)
        ui.click("transfer-upload")
        copied(remote / source.name, expected)
        ui.control(f"explorer-row:{remote / source.name}")
        assert source.read_bytes() == payload
        unchanged_project()
        screenshot(ui.window, "uploaded.png")

        # Download a whole binary stream through the other direction and channel.
        browse(0, downloads)
        select(remote / source.name)
        ui.click("transfer-download")
        copied(downloads / source.name, expected)
        unchanged_project()

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

        # The feature gate hides all entry points without stopping the connection.
        ui.command("Extensions: Manage installed extensions")
        ui.click("extension-filter")
        ui.fill("extension-filter", "rode.transfers")
        ui.click("extension-toggle:rode.transfers")
        key("Escape")
        ui.absent("extension-filter")
        ui.absent("workspace-tab:Transfers")
        ui.absent("open-transfers")
        unchanged_project()

print(
    "Transfers: binary upload/download, folders, conflicts, cancellation, feature gating and independent project navigation passed."
)
