#!/usr/bin/env python3
"""Native slides, text selection/search and protected source over local/SSH files."""

import io
import os
import re
import shutil
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
    mouse_button,
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


def replace_parts(original, old, new, *, only=None):
    """Change fixture XML without using any presentation application."""
    output = io.BytesIO()
    with zipfile.ZipFile(io.BytesIO(original)) as source, zipfile.ZipFile(output, "w") as target:
        for item in source.infolist():
            contents = source.read(item.filename)
            if item.filename.endswith(".xml") and (only is None or item.filename == only):
                contents = contents.replace(old, new)
            target.writestr(item, contents)
    return output.getvalue()


def run(remote=False):
    """Drive the real reader and preserve an unrelated dirty source through viewing and reload."""
    label = "ssh" if remote else "local"
    with fixture_directory(prefix="rode-pptx-ui-") as base:
        root = base / "project"
        root.mkdir()
        presentation = root / "lecture.PPTX"
        original = replace_parts(
            FIXTURE.read_bytes(), b'typeface="Inter"', b'typeface="JetBrains Mono"'
        )
        presentation.write_bytes(original)
        notes = root / "notes.txt"
        notes.write_text("Keep this note.\n")
        environment = fixture_environment(base)
        # Exercise real desktop discovery with a repository-owned font. The
        # private XDG directory keeps this independent of developer/CI fonts.
        fonts = Path(environment["XDG_DATA_HOME"]) / "fonts"
        fonts.mkdir(parents=True)
        shutil.copyfile(
            FIXTURE.parents[2] / "assets/fonts/JetBrainsMono-Regular.ttf",
            fonts / "JetBrainsMono-Regular.ttf",
        )
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
            ui.absent("document-preview-warning")
            ui.absent("source-editor")
            ui.control("pdf-text-ready:1")
            bounds = next(
                b["rect"] for b in ui.layout()["buttons"] if b["name"] == "pdf-text-bounds:1"
            )
            scale = ui.layout()["pixels_per_point"]
            start = (
                round((bounds[0] + 1) * scale),
                round((bounds[1] + bounds[3]) * 0.5 * scale),
            )
            end = (round((bounds[2] - 1) * scale), start[1])
            mouse_button(*start, 1, True)
            ui.move_pointer(*end)
            ui.value("pdf-selection", "RODE — PowerPoint slide 1")
            mouse_button(*end, 1, False)
            ui.control("image-preview", focused=True)
            key("c", ("Control_L",))
            ui.wait(
                lambda: ui.clipboard() == "RODE — PowerPoint slide 1",
                "PowerPoint text drag/copy failed",
            )
            screenshot(ui.window, label + "-selection.png")
            key("f", ("Control_L",))
            ui.control("pdf-find-input", focused=True)
            ui.fill("pdf-find-input", "powerpoint")
            ui.control("pdf-find-count:3")
            ui.press("pdf-find-input", "Return")
            ui.control("pdf-current-page:2")
            ui.control("pdf-page-ready:2")
            screenshot(ui.window, label + "-search.png")
            ui.press("pdf-find-input", "Return", ("Shift_L",))
            ui.control("pdf-current-page:1")
            ui.click("pdf-find-case")
            ui.control("pdf-find-count:0")
            ui.click("pdf-find-case")
            ui.control("pdf-find-count:3")
            key("Escape")
            ui.absent("pdf-find-input")
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
            ui.control("pdf-text-ready:3")
            key("a", ("Control_L",))
            key("c", ("Control_L",))
            ui.wait(
                lambda: ui.clipboard() == "RODE — PowerPoint slide 3",
                "Fullscreen copied the wrong slide",
            )
            ui.control("presentation-missing-images")
            screenshot(ui.window, label + "-fullscreen.png")
            key("Escape")
            ui.absent("pdf-fullscreen")
            ui.click("pdf-view-settings")
            ui.click("pdf-continuous")
            ui.click("pdf-document")
            key("Home")
            ui.control("pdf-current-page:1")
            ui.control("pdf-page-ready:1")
            ui.control("pdf-text-ready:1")
            key("a", ("Control_L",))
            key("c", ("Control_L",))
            ui.wait(
                lambda: ui.clipboard() == "RODE — PowerPoint slide 1",
                "Continuous view copied the wrong slide",
            )
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
            updated = replace_parts(original, b"E82525", b"20BD55", only="ppt/slides/slide1.xml")
            updated = replace_parts(
                updated, b"slide 1", b"reloaded 1", only="ppt/slides/slide1.xml"
            )
            presentation.write_bytes(updated)
            ui.command("PPTX: Reload preview")
            ui.control("pdf-page-ready:1")
            ui.wait(
                lambda: green_slide(ui, label),
                "Reloaded presentation did not show new slide pixels",
            )
            ui.control("pdf-text-ready:1")
            ui.click("image-preview")
            key("a", ("Control_L",))
            key("c", ("Control_L",))
            ui.wait(
                lambda: ui.clipboard() == "RODE — PowerPoint reloaded 1",
                "Reload kept stale PowerPoint text",
            )
            ui.command("Theme: Matrix")
            ui.control("preview-format:PPTX")
            assert green_slide(ui, label), "The editor theme tinted the slide"
            presentation.write_bytes(
                replace_parts(updated, b"JetBrains Mono", b"RODE Missing Corporate Font 72f4b02b")
            )
            ui.command("PPTX: Reload preview")
            ui.control("document-preview-warning")
            presentation.write_bytes(updated)
            ui.command("PPTX: Reload preview")
            ui.absent("document-preview-warning")
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
            + ": Native PPTX slides, selection/copy, search, diagnostics, navigation, fullscreen, reload and protected buffers passed."
        )


if __name__ == "__main__":
    case = os.environ.get("RODE_PPTX_UI_CASE", "local")
    run(remote=case == "ssh")
