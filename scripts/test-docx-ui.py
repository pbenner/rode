#!/usr/bin/env python3
"""Native Word reading, fullscreen, reload and protected buffers on local/SSH hosts."""

import io
import os
import subprocess
import zipfile
from pathlib import Path

os.environ["RODE_UI_ARTIFACTS"] = str(Path(__file__).resolve().parent.parent / "target/docx-ui")
from ui_fixture import native_session, project_args
from ui_harness import ARTIFACTS, fixture_directory, fixture_environment, key, screenshot, text

FIXTURE = Path(__file__).resolve().parent.parent / "tests/fixtures/word.docx"


def red_heading(ui, label):
    """Acknowledge actual new page pixels, rather than stale page-ready controls."""
    state = ui.layout()
    canvas = next(button for button in state["buttons"] if button["name"] == "image-preview")
    left, top, right, bottom = (round(n * state["pixels_per_point"]) for n in canvas["rect"])
    name = label + "-reload.png"
    screenshot(ui.window, name)
    raw = subprocess.check_output(
        [
            "convert",
            str(ARTIFACTS / name),
            "-crop",
            f"{right - left}x{bottom - top}+{left}+{top}",
            "-depth",
            "8",
            "rgb:-",
        ],
        timeout=5,
    )
    return (
        sum(r > 150 and g < 110 and b < 110 for r, g, b in zip(raw[::3], raw[1::3], raw[2::3]))
        > 100
    )


def recolor(original):
    """Edit a fixture's heading color without Word or any document library."""
    output = io.BytesIO()
    with zipfile.ZipFile(io.BytesIO(original)) as source, zipfile.ZipFile(output, "w") as target:
        for item in source.infolist():
            content = source.read(item)
            if item.filename == "word/document.xml":
                content = content.replace(b'val="2048E8"', b'val="E82525"')
            target.writestr(item, content)
    return output.getvalue()


def run(remote=False):
    """Use the shared reader while keeping both the original Word file and a dirty note intact."""
    label = "ssh" if remote else "local"
    with fixture_directory(prefix="rode-docx-ui-") as base:
        root = base / "project"
        root.mkdir()
        path = root / "report.DOCX"
        original = FIXTURE.read_bytes()
        path.write_bytes(original)
        notes = root / "notes.txt"
        notes.write_text("Keep this note.\n")
        env = fixture_environment(base)
        with native_session(
            project_args(root, remote=remote, sandbox=True), env, root, label
        ) as ui:
            ui.open_file("notes.txt")
            key("End", ("Control_L",))
            text("\nunsaved notes")
            expected = "Keep this note.\n\nunsaved notes"
            ui.source(expected)
            ui.open_file("report.DOCX")
            ui.control("preview-format:DOCX")
            ui.control("pdf-page-ready:1")
            ui.absent("source-editor")
            ui.click("pdf-next")
            ui.control("pdf-page-ready:2")
            ui.click("image-preview")
            ui.control("image-preview", focused=True)
            key("Next")
            ui.control("pdf-page-ready:3")
            key("Prior")
            ui.control("pdf-page-ready:2")
            key("Home")
            ui.control("pdf-page-ready:1")
            key("F11")
            ui.control("pdf-fullscreen")
            ui.absent("menu-file")
            screenshot(ui.window, label + "-fullscreen.png")
            key("Escape")
            ui.absent("pdf-fullscreen")
            ui.click("pdf-view-settings")
            ui.click("pdf-continuous")
            ui.click("image-fit-width")
            screenshot(ui.window, label + "-continuous.png")
            for command in ["Save file", "Save file as"]:
                ui.command(command)
                ui.notice("Word previews are read-only.")
                ui.dismiss_notice()
            assert path.read_bytes() == original
            ui.click("pdf-single")
            ui.click("image-preview")
            key("Home")
            ui.control("pdf-page-ready:1")
            path.write_bytes(recolor(original))
            ui.command("Word: Reload preview")
            ui.wait(lambda: red_heading(ui, label), "Word reload did not display the new heading")
            ui.command("Theme: Matrix")
            assert red_heading(ui, label), "The interface theme tinted the Word page"
            # A previous toast must not acknowledge the asynchronous reload error.
            ui.dismiss_notice()
            ui.absent("notice-dismiss")
            path.write_bytes(b"broken DOCX package")
            ui.command("Word: Reload preview")
            ui.control("notice-dismiss")
            assert red_heading(ui, label), "A failed reload discarded the previous Word page"
            ui.command("Word: Toggle fullscreen")
            ui.control("pdf-fullscreen")
            key("w", ("Control_L",))
            ui.absent("pdf-fullscreen")
            ui.document(notes)
            ui.source(expected)
            assert notes.read_text() == "Keep this note.\n"
            ui.command("Save file")
            ui.wait(lambda: notes.read_text() == expected, "Restored source was not saved")
            assert path.read_bytes() == b"broken DOCX package"
        print(label + ": DOCX pages, navigation, fullscreen, reload and protected buffers passed.")


if __name__ == "__main__":
    run(remote=os.environ.get("RODE_DOCX_UI_CASE", "local") == "ssh")
