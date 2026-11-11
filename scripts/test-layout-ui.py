#!/usr/bin/env python3
"""Native tab and responsive-layout regression checks on an isolated X display."""

import hashlib
import os
import subprocess
import tempfile
import time
from pathlib import Path

os.environ["RODE_UI_ARTIFACTS"] = str(Path(__file__).resolve().parent.parent / "target/layout-ui")
from ui_harness import (
    ARTIFACTS,
    BINARY,
    NativeUI,
    click,
    close_window,
    display,
    focus_window,
    key,
    screenshot,
    stop_process,
    text,
    wait_for_project,
    wait_for_window,
    x11,
    xtest,
)


def wait_for(check, message):
    deadline = time.monotonic() + 8
    while time.monotonic() < deadline:
        if check():
            return
        time.sleep(0.05)
    raise AssertionError(message)


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


def append_save(ui, path, marker, *, unsaved=""):
    # Verify the shortcut restored the intended editor; never fix focus by clicking it.
    identity = hashlib.sha256(str(path).encode()).hexdigest()
    ui.control("active-document:" + identity)
    ui.control("source-editor", focused=True)
    before = ui.layout()["pass"]
    key("End", ("Control_L",))
    ui.control("source-editor", focused=True, after_pass=before)
    contents = path.read_text() + unsaved + "\n# " + marker
    before = ui.layout()["pass"]
    text("\n# " + marker)
    ui.source(contents, changed_after=before)
    key("s", ("Control_L",))
    wait_for(lambda: marker in path.read_text(), "Editing focus/selection is wrong: " + marker)


def terminal_focus(ui):
    ui.layout(
        lambda state: any(
            button["name"].startswith("terminal-input:") and button["enabled"] and button["focused"]
            for button in state["buttons"]
        ),
        "The terminal shortcut did not focus its input",
    )


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
    environment["RODE_UI_TEST_STATE"] = str(base / "ui-state.json")
    log_path = ARTIFACTS / "layout.log"
    with log_path.open("w") as log:
        process = subprocess.Popen(
            [str(BINARY), str(root), "--theme", "carbon"], env=environment, stdout=log, stderr=log
        )
        window = None
        try:
            window = wait_for_window(process, log_path)
            wait_for_project(window, process, log_path, root)
            focus_window(window)
            ui = NativeUI(window, process, log_path, base / "ui-state.json", root)
            for name in names:
                ui.open_file(name)
            append_save(ui, root / "notes.txt", "active_tab_focus")
            ui.open_file("notes.txt")
            append_save(ui, root / "notes.txt", "same_file_refocus")
            ui.command("Save file as")
            renamed = root / "notes_saved_with_a_much_longer_filename_for_layout_checks.txt"
            ui.fill_path(renamed.name)
            key("Return")
            wait_for(renamed.exists, "Save As did not finish")
            screenshot(window, "tabs-carbon.png")
            active_line("tabs-carbon.png", 1380, 900, (220, 163, 37), minimum=240)
            click(1356, 66)  # All open editors
            screenshot(window, "open-editors.png")
            click(1130, 101)  # README, first item
            append_save(ui, root / "README.md", "selected_from_editors_list")
            button(505, 66, 2)  # Close inactive main.rs with the middle button.
            key("Next", ("Control_L",))
            append_save(ui, root / "api/config.toml", "api_tab")
            key("Next", ("Control_L",))
            append_save(ui, root / "worker/config.toml", "worker_tab")
            screenshot(window, "duplicate-filenames.png")
            click(1304, 66)  # Scroll to the first tab without changing selection.
            click(1304, 66)
            drag((355, 66), (610, 66))  # README after api/config.toml.
            key("Prior", ("Control_L",))
            append_save(ui, root / "README.md", "selection_survives_reorder")
            key("End", ("Control_L",))
            text("\n# unsaved_layout_change")
            key("w", ("Control_L",))
            screenshot(window, "unsaved-tab-close.png")
            key("Escape")
            append_save(
                ui, root / "README.md", "focus_after_cancel", unsaved="\n# unsaved_layout_change"
            )
            assert "unsaved_layout_change" in (root / "README.md").read_text()
            ui.command("System: Monitor")
            screenshot(window, "monitor-full-workspace.png")
            key("t", ("Control_L", "Shift_L"))
            ui.resize(850, 560)
            terminal_focus(ui)
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
            append_save(ui, root / "README.md", "focus_after_find")
            append_save(ui, root / "README.md", "focus_after_terminal")
            ui.command("Git: Source control")
            time.sleep(0.5)
            screenshot(window, "minimum-git.png")
            ui.command("Git: Source control")  # Hide panel.
            ui.resize(1380, 900)
            ui.command("Theme: Phosphor")
            screenshot(window, "tabs-phosphor.png")
            active_line("tabs-phosphor.png", 1380, 900, (125, 235, 164), left=240)
            key("t", ("Control_L", "Shift_L"))
            terminal_focus(ui)
            screenshot(window, "terminal-full-height.png")
            text("stty size > terminal.size\n")
            wait_for(
                lambda: (root / "terminal.size").exists(),
                "Terminal input stopped working after expanding to full height",
            )
            rows, _ = map(int, (root / "terminal.size").read_text().split())
            assert rows >= 40, f"Terminal height is still capped: {rows} rows"
            key("t", ("Control_L", "Shift_L"))
            append_save(ui, root / "README.md", "focus_after_full_terminal")
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
            stop_process(process)
