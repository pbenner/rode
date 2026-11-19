#!/usr/bin/env python3
"""Native PowerPoint viewing, diagnostics, fullscreen and protected source over local/SSH files."""

import io
import os
import re
import subprocess
import zipfile
from pathlib import Path

os.environ["RODE_UI_ARTIFACTS"] = str(Path(__file__).resolve().parent.parent / "target/pptx-ui")
from ui_fixture import native_session, project_args
from ui_harness import (
    ARTIFACTS,
    fixture_directory,
    fixture_environment,
    key,
    screenshot,
    text,
)

FIXTURE = Path(__file__).resolve().parent.parent / "tests/fixtures/slides.pptx"


def green_slide(ui, label):
    """Observe rendered pixels so cached controls cannot falsely acknowledge a reload."""
    state = ui.layout()
    canvas = next(button for button in state["buttons"] if button["name"] == "image-preview")
    left, top, right, bottom = canvas["rect"]
    scale = state["pixels_per_point"]
    x, y = round((left + right) * scale / 2), round((top + bottom) * scale / 2)
    filename = label + "-reload.png"
    screenshot(ui.window, filename)
    pixel = subprocess.check_output(
        ["convert", str(ARTIFACTS / filename), "-format", f"%[pixel:p{{{x},{y}}}]", "info:"],
        text=True,
        timeout=3,
    )
    red, green, blue = map(int, re.findall(r"\d+", pixel)[:3])
    return green > 150 and red < 80 and blue < 120


def recolor(original):
    """Replace one slide's fill without using any presentation application."""
    output = io.BytesIO()
    with zipfile.ZipFile(io.BytesIO(original)) as source, zipfile.ZipFile(output, "w") as target:
        for item in source.infolist():
            contents = source.read(item.filename)
            if item.filename == "ppt/slides/slide1.xml":
                contents = contents.replace(b"E82525", b"20BD55")
            target.writestr(item, contents)
    return output.getvalue()


def run(remote=False):
    """Drive the real reader and preserve an unrelated dirty source through viewing and reload."""
    label = "ssh" if remote else "local"
    with fixture_directory(prefix="rode-pptx-ui-") as base:
        root = base / "project"
        root.mkdir()
        presentation = root / "lecture.PPTX"
        original = FIXTURE.read_bytes()
        presentation.write_bytes(original)
        notes = root / "notes.txt"
        notes.write_text("Keep this note.\n")
        environment = fixture_environment(base)
        environment["RODE_UI_TEST_STATE"] = str(base / "state.json")
        with native_session(
            project_args(root, remote=remote, sandbox=True), environment, root, label
        ) as ui:
            ui.open_file("notes.txt")
            key("End", ("Control_L",))
            text("\nunsaved notes")
            expected = "Keep this note.\n\nunsaved notes"
            ui.source(expected)
            ui.open_file("lecture.PPTX")
            ui.control("preview-format:PPTX")
            ui.control("pdf-current-page:1")
            ui.control("pdf-page-ready:1")
            ui.absent("source-editor")
            ui.click("image-preview")
            ui.control("image-preview", focused=True)
            key("Right")
            ui.control("pdf-current-page:2")
            ui.control("pdf-page-ready:2")
            key("Left")
            ui.control("pdf-current-page:1")
            key("F11")
            ui.control("pdf-fullscreen")
            ui.absent("menu-file")
            ui.click("image-preview")
            key("End")
            ui.control("pdf-current-page:3")
            ui.control("pdf-page-ready:3")
            ui.control("presentation-missing-images")
            screenshot(ui.window, label + "-fullscreen.png")
            key("Escape")
            ui.absent("pdf-fullscreen")
            ui.click("pdf-continuous")
            ui.click("pdf-document")
            key("Home")
            ui.control("pdf-current-page:1")
            ui.control("pdf-page-ready:1")
            ui.click("image-fit-width")
            screenshot(ui.window, label + "-continuous.png")
            ui.command("Save file")
            ui.notice("PowerPoint previews are read-only.")
            ui.dismiss_notice()
            ui.command("Save file as")
            ui.notice("PowerPoint previews are read-only.")
            ui.dismiss_notice()
            assert presentation.read_bytes() == original
            ui.click("pdf-single")
            ui.click("image-preview")
            key("Home")
            ui.control("pdf-current-page:1")
            updated = recolor(original)
            presentation.write_bytes(updated)
            ui.command("PPTX: Reload preview")
            ui.control("pdf-page-ready:1")
            ui.wait(
                lambda: green_slide(ui, label),
                "Reloaded presentation did not show new slide pixels",
            )
            ui.command("Theme: Matrix")
            ui.control("preview-format:PPTX")
            assert green_slide(ui, label), "The editor theme tinted the slide"
            presentation.write_bytes(b"broken PPTX package")
            ui.command("PPTX: Reload preview")
            ui.control("notice-dismiss")
            ui.control("pdf-page-ready:1")
            assert green_slide(ui, label), "A failed reload discarded the previous slide"
            ui.command("PPTX: Toggle fullscreen")
            ui.control("pdf-fullscreen")
            key("w", ("Control_L",))
            ui.absent("pdf-fullscreen")
            ui.document(notes)
            ui.source(expected)
            assert notes.read_text() == "Keep this note.\n"
            ui.command("Save file")
            ui.wait(lambda: notes.read_text() == expected, "Restored source was not saved")
            assert presentation.read_bytes() == b"broken PPTX package"
        print(
            label
            + ": Native PPTX slides, images, diagnostics, navigation, fullscreen, reload and protected buffers passed."
        )


if __name__ == "__main__":
    case = os.environ.get("RODE_PPTX_UI_CASE", "local")
    run(remote=case == "ssh")
