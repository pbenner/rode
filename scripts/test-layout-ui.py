#!/usr/bin/env python3
"""Native tab and responsive-layout regression checks on an isolated X display."""

import ctypes
import os
import re
import subprocess
import tempfile
import time
from pathlib import Path

os.environ["RODE_UI_ARTIFACTS"] = str(Path(__file__).resolve().parent.parent / "target/layout-ui")
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

x11.XResizeWindow.argtypes = [ctypes.c_void_p, ctypes.c_ulong, ctypes.c_uint, ctypes.c_uint]


def wait_for(check, message):
    deadline = time.monotonic() + 8
    while time.monotonic() < deadline:
        if check():
            return
        time.sleep(0.05)
    raise AssertionError(message)


def command(name):
    key("p", ("Control_L", "Shift_L"))
    text(name)
    key("Return")
    time.sleep(0.3)


def button(x, y, number=1):
    xtest.XTestFakeMotionEvent(display, -1, x, y, 0)
    xtest.XTestFakeButtonEvent(display, number, 1, 0)
    xtest.XTestFakeButtonEvent(display, number, 0, 0)
    x11.XFlush(display)
    time.sleep(0.25)


def drag(start, end):
    xtest.XTestFakeMotionEvent(display, -1, *start, 0)
    xtest.XTestFakeButtonEvent(display, 1, 1, 0)
    x11.XFlush(display)
    time.sleep(0.1)
    for step in range(1, 15):
        x = round(start[0] + (end[0] - start[0]) * step / 14)
        y = round(start[1] + (end[1] - start[1]) * step / 14)
        xtest.XTestFakeMotionEvent(display, -1, x, y, 0)
        x11.XFlush(display)
        time.sleep(0.035)
    xtest.XTestFakeButtonEvent(display, 1, 0, 0)
    x11.XFlush(display)
    time.sleep(0.4)


def append_save(path, marker):
    key("End", ("Control_L",))
    text("\n# " + marker)
    key("s", ("Control_L",))
    wait_for(lambda: marker in path.read_text(), "Editing focus/selection is wrong: " + marker)


def active_line(filename, width, height, accent, left=302, right=None, minimum=80):
    data = subprocess.check_output(["convert", str(ARTIFACTS / filename), "-depth", "8", "rgb:-"])
    assert len(data) == width * height * 3
    pixels = [
        tuple(data[(48 * width + x) * 3 : (48 * width + x) * 3 + 3])
        for x in range(left, right or width - 90)
    ]
    assert pixels.count(accent) >= minimum, (
        "Active tab is off-screen or its selection marker is missing"
    )


with tempfile.TemporaryDirectory(prefix="rode-layout-ui-") as directory:
    base = Path(directory)
    root = base / "project-with-a-long-descriptive-name"
    names = [
        "README.md",
        "src/main.rs",
        "api/config.toml",
        "worker/config.toml",
        "processing_configuration_with_a_very_long_filename.toml",
        "analysis.jl",
        "notes.txt",
    ]
    for name in names:
        path = root / name
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text("// main\nfn main() {}\n" if path.suffix == ".rs" else "# " + name + "\n")
    environment = dict(
        os.environ,
        XDG_CONFIG_HOME=str(base / "config"),
        XDG_DATA_HOME=str(base / "data"),
        GIT_CONFIG_NOSYSTEM="1",
        GIT_CONFIG_GLOBAL="/dev/null",
    )

    def git(*args):
        return subprocess.check_output(
            ["git", "-C", str(root), *args], env=environment, text=True, stderr=subprocess.DEVNULL
        )

    git("init", "-b", "main")
    git("config", "user.name", "RODE layout test")
    git("config", "user.email", "rode@example.invalid")
    git("add", ".")
    git("commit", "-m", "Layout fixture")
    with (ARTIFACTS / "layout.log").open("w") as log:
        process = subprocess.Popen(
            [str(BINARY), str(root), "--theme", "carbon"], env=environment, stdout=log, stderr=log
        )
        window = None
        try:
            for _ in range(100):
                found = re.search(
                    r'(0x[0-9a-f]+) "RODE',
                    subprocess.check_output(["xwininfo", "-root", "-tree"], text=True),
                )
                if found:
                    window = int(found[1], 16)
                    break
                assert process.poll() is None
                time.sleep(0.05)
            assert window
            time.sleep(0.8)
            x11.XSetInputFocus(display, window, 1, 0)
            x11.XFlush(display)
            for name in names:
                key("p", ("Control_L",))
                text(name)
                key("Return")
                time.sleep(0.2)
            append_save(root / "notes.txt", "active_tab_focus")
            key("p", ("Control_L",))
            text("notes.txt")
            key("Return")
            time.sleep(0.2)
            append_save(root / "notes.txt", "same_file_refocus")
            key("s", ("Control_L", "Shift_L"))
            time.sleep(0.2)
            key("a", ("Control_L",))
            renamed = root / "notes_saved_with_a_much_longer_filename_for_layout_checks.txt"
            text(renamed.name)
            key("Return")
            wait_for(renamed.exists, "Save As did not finish")
            screenshot(window, "tabs-carbon.png")
            active_line("tabs-carbon.png", 1380, 900, (220, 163, 37), minimum=240)
            click(1356, 66)  # All open editors
            screenshot(window, "open-editors.png")
            click(1130, 101)  # README, first item
            append_save(root / "README.md", "selected_from_editors_list")
            button(505, 66, 2)  # Close inactive main.rs with the middle button.
            key("Next", ("Control_L",))
            append_save(root / "api/config.toml", "api_tab")
            key("Next", ("Control_L",))
            append_save(root / "worker/config.toml", "worker_tab")
            screenshot(window, "duplicate-filenames.png")
            click(1304, 66)  # Scroll to the first tab without changing selection.
            click(1304, 66)
            drag((355, 66), (610, 66))  # README after api/config.toml.
            key("Prior", ("Control_L",))
            append_save(root / "README.md", "selection_survives_reorder")
            key("End", ("Control_L",))
            text("\n# unsaved_layout_change")
            key("w", ("Control_L",))
            screenshot(window, "unsaved-tab-close.png")
            key("Escape")
            append_save(root / "README.md", "focus_after_cancel")
            assert "unsaved_layout_change" in (root / "README.md").read_text()
            command("System: Monitor")
            screenshot(window, "monitor-full-workspace.png")
            key("t", ("Control_L", "Shift_L"))
            x11.XResizeWindow(display, window, 850, 560)
            x11.XFlush(display)
            time.sleep(0.6)
            screenshot(window, "minimum-terminal.png")
            text("echo layout_terminal_ok > terminal.ok\n")
            wait_for(
                lambda: (root / "terminal.ok").exists(),
                "Terminal controls/content were lost in the narrow layout",
            )
            key("t", ("Control_L", "Shift_L"))  # Return to full-width code, not a split pane.
            click(410, 215)
            key("f", ("Control_L",))
            text("layout")
            screenshot(window, "minimum-window.png")
            active_line(
                "minimum-window.png", 850, 560, (220, 163, 37), left=250, right=750, minimum=140
            )
            key("Escape")
            append_save(root / "README.md", "focus_after_find")
            append_save(root / "README.md", "focus_after_terminal")
            command("Git: Source control")
            time.sleep(0.5)
            screenshot(window, "minimum-git.png")
            command("Git: Source control")  # Hide panel.
            x11.XResizeWindow(display, window, 1380, 900)
            x11.XFlush(display)
            time.sleep(0.4)
            theme("phosphor")
            screenshot(window, "tabs-phosphor.png")
            active_line("tabs-phosphor.png", 1380, 900, (125, 235, 164), left=240)
            key("t", ("Control_L", "Shift_L"))
            time.sleep(0.3)
            screenshot(window, "terminal-full-height.png")
            click(600, 400)
            text("stty size > terminal.size\n")
            wait_for(
                lambda: (root / "terminal.size").exists(),
                "Terminal input stopped working after expanding to full height",
            )
            rows, _ = map(int, (root / "terminal.size").read_text().split())
            assert rows >= 40, f"Terminal height is still capped: {rows} rows"
            key("t", ("Control_L", "Shift_L"))
            append_save(root / "README.md", "focus_after_full_terminal")
            screenshot(window, "terminal-restored-editor.png")
            close_window(window, process)
            print(
                "Native tabs: overflow/reveal, editor list, duplicate names, middle-close, drag ordering, focus, and unsaved protection passed."
            )
            print(
                "Native layout: both themes, 850×560 with exclusive workspace views, find, full-width Git, and terminal/PTY resizing passed."
            )
        except Exception:
            if window is not None and process.poll() is None:
                screenshot(window, "layout-failure.png")
            raise
        finally:
            if process.poll() is None:
                process.terminate()
                process.wait(timeout=5)
