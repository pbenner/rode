#!/usr/bin/env python3
"""Native PTY contrast, palette edits, per-theme persistence and live-shell checks."""

import ctypes
import json
import os
import re
import subprocess
import tempfile
import time
from pathlib import Path

os.environ["RODE_UI_ARTIFACTS"] = str(
    Path(__file__).resolve().parent.parent / "target/terminal-colors-ui"
)
from ui_harness import (
    ARTIFACTS,
    BINARY,
    click,
    close_window,
    display,
    key,
    screenshot,
    text,
    theme,
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
            x11.XSetInputFocus(display, window, 1, 0)
            x11.XFlush(display)
            return window
        assert process.poll() is None, "RODE exited before opening a window"
        time.sleep(0.05)
    raise AssertionError("No RODE window")


def colors():
    key("p", ("Control_L", "Shift_L"))
    text("Terminal: Colors and contrast")
    key("Return")
    time.sleep(0.2)


def number(x, y, value):
    click(x, y)
    click(x, y)
    key("a", ("Control_L",))
    text(value)
    key("Return")


def bright_pixels(filename):
    """Count readable neutral glyph pixels in the three deliberately dark lines."""
    rgb = subprocess.check_output(
        [
            "convert",
            str(ARTIFACTS / filename),
            "-crop",
            "150x43+315+690",
            "-depth",
            "8",
            "rgb:-",
        ]
    )
    return sum(min(rgb[i : i + 3]) > 90 for i in range(0, len(rgb), 3))


with tempfile.TemporaryDirectory(prefix="rode-terminal-colors-") as directory:
    root = Path(directory)
    shell = root / "fixture-shell"
    shell.write_text(
        "#!/bin/sh\n"
        "printf '\\033[?25l\\033[30mANSI_DARK\\033[0m\\n"
        "\\033[38;5;17mINDEX_DARK\\033[0m\\n"
        "\\033[38;2;9;10;11mRGB_DARK\\033[0m\\n'\n"
        "exec /bin/sh -i\n"
    )
    shell.chmod(0o700)
    environment = dict(
        os.environ,
        XDG_CONFIG_HOME=str(root / "config"),
        XDG_DATA_HOME=str(root / "data"),
        WINIT_X11_SCALE_FACTOR="1",
        SHELL=str(shell),
    )
    saved = root / "data/rode/app.ron"
    for run in ("initial", "restart"):
        with (ARTIFACTS / f"{run}.log").open("w") as log:
            process = subprocess.Popen(
                [str(BINARY), str(root)], env=environment, stdout=log, stderr=log
            )
            window = None
            try:
                window = window_for(process)
                key("grave", ("Control_L",))
                time.sleep(1)
                if run == "initial":
                    screenshot(window, "contrast-default.png")
                    assert bright_pixels("contrast-default.png") > 100
                    colors()
                    screenshot(window, "settings.png")
                    number(470, 260, "1")
                    wait_for(
                        lambda: (
                            preferences(saved).get("terminal_color.phosphor.minimum_contrast")
                            == "1"
                        ),
                        "Exact-color mode was not saved immediately",
                    )
                    screenshot(window, "contrast-off.png")
                    assert bright_pixels("contrast-off.png") < 10
                    click(440, 347)
                    number(507, 373, "240")
                    click(700, 232)
                    wait_for(
                        lambda: (
                            preferences(saved).get("terminal_color.phosphor.foreground")
                            == "#f0dad8"
                        ),
                        "Foreground edit was not saved",
                    )
                    click(375, 401)
                    click(440, 449)
                    number(507, 475, "240")
                    click(700, 232)
                    wait_for(
                        lambda: (
                            preferences(saved).get("terminal_color.phosphor.ansi.0") == "#f02224"
                        ),
                        "ANSI swatch edit was not saved",
                    )
                    screenshot(window, "ansi-edited.png")
                    click(735, 100)
                    theme("Carbon")
                    colors()
                    number(470, 260, "7")
                    wait_for(
                        lambda: (
                            preferences(saved).get("terminal_color.carbon.minimum_contrast") == "7"
                        ),
                        "Second theme did not save independently",
                    )
                    click(735, 100)
                    theme("Phosphor")
                    assert preferences(saved)["terminal_color.phosphor.minimum_contrast"] == "1"
                    click(600, 755)
                    text("printf still-alive > color-proof\n")
                    wait_for(
                        lambda: (root / "color-proof").exists(),
                        "Palette changes disrupted shell input",
                    )
                    assert (root / "color-proof").read_text() == "still-alive"
                else:
                    assert preferences(saved)["terminal_color.phosphor.ansi.0"] == "#f02224"
                    colors()
                    screenshot(window, "restored.png")
                    # Collapse the restored ANSI section to reveal Reset.
                    click(375, 401)
                    click(415, 449)
                    wait_for(
                        lambda: (
                            preferences(saved).get("terminal_color.phosphor.minimum_contrast")
                            == "theme"
                        ),
                        "Reset did not restore theme defaults",
                    )
                    assert preferences(saved)["terminal_color.phosphor.ansi.0"] == "theme"
                    assert preferences(saved)["terminal_color.carbon.minimum_contrast"] == "7"
                    screenshot(window, "reset.png")
                    assert bright_pixels("reset.png") > 100
                    x11.XResizeWindow.argtypes = [
                        ctypes.c_void_p,
                        ctypes.c_ulong,
                        ctypes.c_uint,
                        ctypes.c_uint,
                    ]
                    x11.XResizeWindow(display, window, 850, 560)
                    x11.XFlush(display)
                    time.sleep(0.4)
                    click(375, 401)
                    screenshot(window, "minimum-window.png")
                    xtest.XTestFakeMotionEvent(display, -1, 700, 430, 0)
                    for _ in range(8):
                        xtest.XTestFakeButtonEvent(display, 5, 1, 0)
                        xtest.XTestFakeButtonEvent(display, 5, 0, 0)
                    x11.XFlush(display)
                    screenshot(window, "minimum-window-scrolled.png")
                close_window(window, process)
            finally:
                if process.poll() is None:
                    process.terminate()
                    process.wait(timeout=10)
print(f"Terminal color UI checks passed; artifacts: {ARTIFACTS}")
