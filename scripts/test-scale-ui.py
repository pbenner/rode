#!/usr/bin/env python3
"""Native interface scaling at 1x/2x DPI, independent text size and safe restart."""

import json
import os
import re
import subprocess
import tempfile
import time
from pathlib import Path

os.environ["RODE_UI_ARTIFACTS"] = str(Path(__file__).resolve().parent.parent / "target/scale-ui")
from ui_harness import (
    ARTIFACTS,
    BINARY,
    click,
    close_window,
    display,
    focus_window,
    key,
    screenshot,
    text,
    x11,
    xtest,
)


def wait_for(check, message):
    deadline = time.monotonic() + 10
    while time.monotonic() < deadline:
        if check():
            return
        time.sleep(0.05)
    raise AssertionError(message)


def preferences(path):
    """Read the actual atomic RON string map without changing application state."""
    if not path.exists():
        return {}
    contents = path.read_text().rstrip()
    return json.loads(contents[:-1].rstrip().removesuffix(",") + "}")


def window_for(process):
    for _ in range(150):
        tree = subprocess.check_output(["xwininfo", "-root", "-tree"], text=True)
        match = re.search(r'(0x[0-9a-f]+) "RODE', tree)
        if match:
            window = int(match[1], 16)
            time.sleep(0.8)
            focus_window(window)
            return window
        assert process.poll() is None, "RODE exited before opening a window"
        time.sleep(0.05)
    raise AssertionError("No RODE window")


def command(title):
    key("p", ("Control_L", "Shift_L"))
    text(title)
    key("Return")
    time.sleep(0.15)


def scale_menu(dpi, scale):
    """Open the real View submenu at the current monitor/interface scale."""
    factor = dpi * scale
    click(round(195 * factor), round(24 * factor))
    click(round(290 * factor), round(104 * factor))


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


with tempfile.TemporaryDirectory(prefix="rode-scale-ui-") as directory:
    for dpi in (1, 2):
        root = Path(directory) / str(dpi)
        root.mkdir()
        source = root / "main.txt"
        source.write_text("Original text\n")
        environment = dict(
            os.environ,
            WINIT_UNIX_BACKEND="x11",
            WINIT_X11_SCALE_FACTOR=str(dpi),
            XDG_CONFIG_HOME=str(root / "config"),
            XDG_DATA_HOME=str(root / "data"),
        )
        saved = root / "data/rode/app.ron"
        with (ARTIFACTS / f"dpi-{dpi}.log").open("w") as log:
            process = subprocess.Popen(
                [str(BINARY), str(root)], env=environment, stdout=log, stderr=log
            )
            window = None
            try:
                window = window_for(process)
                key("p", ("Control_L",))
                text("main.txt")
                key("Return")
                time.sleep(0.6)
                click(430 * dpi, 138 * dpi)
                key("End", ("Control_L",))
                text("Unsaved text survives scaling\n")
                screenshot(window, f"dpi-{dpi}-original.png")
                original = header_height(f"dpi-{dpi}-original.png", dpi)
                key("equal", ("Control_L",))
                wait_for(
                    lambda: preferences(saved).get("font_size") == "15",
                    "Editor font did not change",
                )
                screenshot(window, f"dpi-{dpi}-font.png")
                assert header_height(f"dpi-{dpi}-font.png", dpi) == original
                scale_menu(dpi, 1)
                screenshot(window, f"dpi-{dpi}-scale-presets.png")
                # A held pointer must not resize the GUI. Apply the preset only
                # after release, so its target never moves during the click.
                xtest.XTestFakeMotionEvent(display, -1, 480 * dpi, 210 * dpi, 0)
                xtest.XTestFakeButtonEvent(display, 1, 1, 0)
                x11.XFlush(display)
                time.sleep(0.3)
                assert preferences(saved).get("ui_scale", "1") == "1"
                xtest.XTestFakeButtonEvent(display, 1, 0, 0)
                x11.XFlush(display)
                wait_for(
                    lambda: preferences(saved).get("ui_scale") == "0.75",
                    "Percentage preset did not apply on release",
                )
                scale_menu(dpi, 0.75)
                click(round(480 * dpi * 0.75), round(146 * dpi * 0.75))
                wait_for(
                    lambda: preferences(saved).get("ui_scale") == "0.8",
                    "Larger action did not apply a 5% step",
                )
                scale_menu(dpi, 0.8)
                click(round(480 * dpi * 0.8), round(121 * dpi * 0.8))
                wait_for(
                    lambda: preferences(saved).get("ui_scale") == "0.75",
                    "Smaller action did not apply a 5% step",
                )
                key("0", ("Control_L",))
                wait_for(
                    lambda: preferences(saved).get("ui_scale") == "1",
                    "Could not reset after using the scale menu",
                )
                for _ in range(5):
                    command("Interface: Zoom out")
                wait_for(
                    lambda: preferences(saved).get("ui_scale") == "0.75",
                    "Interface scale was not saved",
                )
                assert preferences(saved)["font_size"] == "15"
                assert source.read_text() == "Original text\n", "Scaling saved a dirty buffer"
                screenshot(window, f"dpi-{dpi}-75.png")
                assert abs(header_height(f"dpi-{dpi}-75.png", dpi) - original * 0.75) <= 2
                click(round(205 * dpi * 0.75), round(24 * dpi * 0.75))
                screenshot(window, f"dpi-{dpi}-view-menu.png")
                key("Escape")
                key("0", ("Control_L",))
                wait_for(
                    lambda: preferences(saved).get("ui_scale") == "1",
                    "Ctrl+0 did not reset interface scale",
                )
                screenshot(window, f"dpi-{dpi}-reset.png")
                assert header_height(f"dpi-{dpi}-reset.png", dpi) == original
                click(430 * dpi, 138 * dpi)
                key("s", ("Control_L",))
                wait_for(
                    lambda: "Unsaved text survives scaling" in source.read_text(),
                    "Scaling lost the dirty buffer",
                )
                for _ in range(5):
                    command("Interface: Zoom out")
                wait_for(
                    lambda: preferences(saved).get("ui_scale") == "0.75",
                    "Scale was not saved before exit",
                )
                close_window(window, process)
                process = subprocess.Popen(
                    [str(BINARY), str(root)], env=environment, stdout=log, stderr=log
                )
                window = window_for(process)
                screenshot(window, f"dpi-{dpi}-restart.png")
                assert abs(header_height(f"dpi-{dpi}-restart.png", dpi) - original * 0.75) <= 2
                assert preferences(saved)["font_size"] == "15"
                close_window(window, process)
                print(
                    f"Native {dpi}x DPI: font size, interface scale, reset, dirty buffer and restart passed."
                )
            except Exception:
                if window is not None and process.poll() is None:
                    screenshot(window, f"dpi-{dpi}-failure.png")
                raise
            finally:
                if process.poll() is None:
                    process.terminate()
                    process.wait(timeout=5)
