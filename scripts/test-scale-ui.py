#!/usr/bin/env python3
"""Native interface scaling at 1x/2x DPI, independent text size and safe restart.

Run with scripts/ui_display.py --screen 3200x2200x24. The 2x case needs
room for the doubled window; controls use acknowledged native geometry.
"""

import json
import os
import subprocess
import time
from pathlib import Path

os.environ["RODE_UI_ARTIFACTS"] = str(Path(__file__).resolve().parent.parent / "target/scale-ui")
from ui_fixture import native_session
from ui_harness import (
    ARTIFACTS,
    BINARY,
    display,
    fixture_directory,
    fixture_environment,
    key,
    screenshot,
    text,
    x11,
    xtest,
)


def preferences(path):
    """Read the actual atomic RON string map without changing application state."""
    if not path.exists():
        return {}
    contents = path.read_text().rstrip()
    return json.loads(contents[:-1].rstrip().removesuffix(",") + "}")


def scale_menu(ui, scale):
    """Open the real View submenu at the current monitor/interface scale."""
    ui.click("menu-view")
    ui.click(f"submenu:Interface scale: {round(scale * 100)}%")


def header_height(filename, dpi):
    """Measure the native menu boundary outside text, proving whole-UI scaling."""
    pixels = subprocess.check_output(
        [
            "convert",
            str(ARTIFACTS / filename),
            "-crop",
            f"1x250+{3 * dpi}+0",
            "-depth",
            "8",
            "rgb:-",
        ]
    )
    first = pixels[:3]
    for row in range(3 * dpi, len(pixels) // 3):
        if pixels[3 * row : 3 * row + 3] != first:
            return row
    raise AssertionError("Menu boundary not found")


with fixture_directory(prefix="rode-scale-ui-") as directory:
    for dpi in (1, 2):
        base = Path(directory) / str(dpi)
        root = base / "project"
        root.mkdir(parents=True)
        source = root / "main.txt"
        source.write_text("Original text\n")
        state_path = base / "ui-state.json"
        environment = fixture_environment(
            base,
            WINIT_UNIX_BACKEND="x11",
            WINIT_X11_SCALE_FACTOR=str(dpi),
            RODE_UI_TEST_STATE=str(state_path),
        )
        saved = base / "data/rode/app.ron"
        args = [str(BINARY), str(root)]
        with native_session(args, environment, root, f"dpi-{dpi}") as ui:
            ui.open_file("main.txt")
            ui.click("source-editor")
            key("End", ("Control_L",))
            text("Unsaved text survives scaling\n")
            ui.source("Original text\nUnsaved text survives scaling\n")
            screenshot(ui.window, f"dpi-{dpi}-original.png")
            original = header_height(f"dpi-{dpi}-original.png", dpi)
            key("equal", ("Control_L",))
            ui.wait(
                lambda: preferences(saved).get("font_size") == "15",
                "Editor font did not change",
            )
            screenshot(ui.window, f"dpi-{dpi}-font.png")
            assert header_height(f"dpi-{dpi}-font.png", dpi) == original
            scale_menu(ui, 1)
            screenshot(ui.window, f"dpi-{dpi}-scale-presets.png")
            # A held pointer must not resize the GUI. Apply the preset only
            # after release, so its target never moves during the click.
            point = ui.control("interface-scale-75", hovered=True)
            xtest.XTestFakeMotionEvent(display, -1, *point, 0)
            xtest.XTestFakeButtonEvent(display, 1, 1, 0)
            x11.XFlush(display)
            time.sleep(0.3)
            assert preferences(saved).get("ui_scale", "1") == "1"
            xtest.XTestFakeButtonEvent(display, 1, 0, 0)
            x11.XFlush(display)
            ui.wait(
                lambda: preferences(saved).get("ui_scale") == "0.75",
                "Percentage preset did not apply on release",
            )
            scale_menu(ui, 0.75)
            ui.click("interface-larger")
            ui.wait(
                lambda: preferences(saved).get("ui_scale") == "0.8",
                "Larger action did not apply a 5% step",
            )
            scale_menu(ui, 0.8)
            ui.click("interface-smaller")
            ui.wait(
                lambda: preferences(saved).get("ui_scale") == "0.75",
                "Smaller action did not apply a 5% step",
            )
            key("0", ("Control_L",))
            ui.wait(
                lambda: preferences(saved).get("ui_scale") == "1",
                "Could not reset after using the scale menu",
            )
            for _ in range(5):
                ui.command("Interface: Zoom out")
            ui.wait(
                lambda: preferences(saved).get("ui_scale") == "0.75",
                "Interface scale was not saved",
            )
            assert preferences(saved)["font_size"] == "15"
            assert source.read_text() == "Original text\n", "Scaling saved a dirty buffer"
            screenshot(ui.window, f"dpi-{dpi}-75.png")
            assert abs(header_height(f"dpi-{dpi}-75.png", dpi) - original * 0.75) <= 2
            ui.click("menu-view")
            screenshot(ui.window, f"dpi-{dpi}-view-menu.png")
            key("Escape")
            key("0", ("Control_L",))
            ui.wait(
                lambda: preferences(saved).get("ui_scale") == "1",
                "Ctrl+0 did not reset interface scale",
            )
            screenshot(ui.window, f"dpi-{dpi}-reset.png")
            assert header_height(f"dpi-{dpi}-reset.png", dpi) == original
            ui.click("source-editor")
            key("s", ("Control_L",))
            ui.wait(
                lambda: "Unsaved text survives scaling" in source.read_text(),
                "Scaling lost the dirty buffer",
            )
            for _ in range(5):
                ui.command("Interface: Zoom out")
            ui.wait(
                lambda: preferences(saved).get("ui_scale") == "0.75",
                "Scale was not saved before exit",
            )
        with native_session(args, environment, root, f"dpi-{dpi}-restart") as ui:
            ui.layout(
                lambda state: abs(state["pixels_per_point"] - dpi * 0.75) < 0.01,
                "Saved scale was not rendered",
            )
            screenshot(ui.window, f"dpi-{dpi}-restart.png")
            assert abs(header_height(f"dpi-{dpi}-restart.png", dpi) - original * 0.75) <= 2
            assert preferences(saved)["font_size"] == "15"
        print(
            f"Native {dpi}x DPI: font size, interface scale, reset, dirty buffer and restart passed."
        )
