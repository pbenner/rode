#!/usr/bin/env python3
"""Native tab and responsive-layout regression checks on an isolated X display."""

import hashlib
import os
import subprocess
import time
from pathlib import Path

os.environ["RODE_UI_ARTIFACTS"] = str(Path(__file__).resolve().parent.parent / "target/layout-ui")
from ui_harness import (
    ARTIFACTS,
    BINARY,
    NativeUI,
    assert_pane_outline,
    close_window,
    display,
    fixture_directory,
    fixture_environment,
    focus_window,
    key,
    mouse_button,
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


def drag(start, end, check=None):
    ui.move_pointer(*start)
    mouse_button(*start, 1, True)
    try:
        for step in range(1, 15):
            x = round(start[0] + (end[0] - start[0]) * step / 14)
            y = round(start[1] + (end[1] - start[1]) * step / 14)
            ui.move_pointer(x, y)
        if check is not None:
            check()
    finally:
        mouse_button(*end, 1, False)


def sidebar_divider(state, pane="Explorer"):
    controls = {button["name"]: button["rect"] for button in state["buttons"]}
    sidebar, editor = controls["pane-" + pane], controls["pane-Editor"]
    scale = state["pixels_per_point"]
    # Both panes use the same symmetric padding, so their midpoint is the
    # real divider without assuming a sidebar width or a theme's margin.
    return round((sidebar[2] + editor[0]) / 2 * scale), round((sidebar[1] + sidebar[3]) / 2 * scale)


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


def active_line(ui, filename, width, height, accent, minimum=80):
    def painted():
        # Follow the rendered tab instead of assuming a theme's top margin.
        layout = ui.layout()
        controls = {button["name"]: button["rect"] for button in layout["buttons"]}
        active = next(
            name.split(":", 1)[1] for name in controls if name.startswith("active-document:")
        )
        tab = controls["editor-tab:" + active]
        pane = controls["pane-Editor"]
        scale = layout["pixels_per_point"]
        top = max(0, round(tab[1] * scale))
        bottom = min(height, round(tab[3] * scale))
        left = max(0, round(max(pane[0], tab[0]) * scale))
        right = min(width, round(min(pane[2], tab[2]) * scale))
        screenshot(ui.window, filename)
        data = subprocess.check_output(
            ["convert", str(ARTIFACTS / filename), "-depth", "8", "rgb:-"]
        )
        if len(data) != width * height * 3:
            return False
        # A theme may put the accent at either edge or inside a rounded tab.
        # Require a continuous-width marker inside this active tab, rather
        # than counting unrelated accents across the whole editor header.
        required = min(minimum * scale, max(0, right - left - 32 * scale))
        return required > 0 and any(
            sum(
                tuple(data[(y * width + x) * 3 : (y * width + x) * 3 + 3]) == accent
                for x in range(left, right)
            )
            >= required
            for y in range(top, bottom)
        )

    wait_for(painted, "Active tab is off-screen or its selection marker is missing")


with fixture_directory(prefix="rode-layout-ui-") as base:
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
    environment = fixture_environment(
        base,
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
            ui.navigate("f")
            assert_pane_outline(ui, "Explorer", "explorer-focus.png")
            start = sidebar_divider(ui.layout())
            xtest.XTestFakeMotionEvent(display, -1, *start, 0)
            x11.XFlush(display)
            assert_pane_outline(ui, "Explorer", "explorer-focus-resize-hover.png")
            scale = ui.layout()["pixels_per_point"]
            end = (round(start[0] + 48 * scale), start[1])

            def resizing():
                ui.layout(
                    lambda state: sidebar_divider(state)[0] > start[0] + 30 * scale,
                    "Explorer did not resize",
                )
                assert_pane_outline(ui, "Explorer", "explorer-focus-resize-active.png")

            drag(start, end, check=resizing)
            ui.click("activity:Connections")
            ui.control("pane-Connections")
            ui.layout(
                lambda state: abs(sidebar_divider(state, "Connections")[0] - end[0]) <= 1,
                "Switching to Connections changed the sidebar width",
            )
            connection_start = sidebar_divider(ui.layout(), "Connections")
            connection_end = (round(connection_start[0] + 32 * scale), connection_start[1])
            drag(connection_start, connection_end)
            ui.layout(
                lambda state: (
                    abs(sidebar_divider(state, "Connections")[0] - connection_end[0]) <= 1
                ),
                "Connections did not resize",
            )
            ui.click("activity:Explorer")
            ui.control("pane-Explorer")
            ui.layout(
                lambda state: abs(sidebar_divider(state)[0] - connection_end[0]) <= 1,
                "Switching back to Explorer changed the sidebar width",
            )
            ui.navigate("f")
            drag(sidebar_divider(ui.layout()), start)
            ui.layout(
                lambda state: abs(sidebar_divider(state)[0] - start[0]) <= 1,
                "Explorer width was not restored",
            )
            assert_pane_outline(ui, "Explorer", "explorer-focus-after-resize.png")
            ui.navigate("e")
            append_save(ui, root / "notes.txt", "active_tab_focus")
            ui.open_file("notes.txt")
            append_save(ui, root / "notes.txt", "same_file_refocus")
            ui.command("Save file as")
            renamed = root / "notes_saved_with_a_much_longer_filename_for_layout_checks.txt"
            ui.fill_path(renamed.name)
            key("Return")
            wait_for(renamed.exists, "Save As did not finish")
            screenshot(window, "tabs-carbon.png")
            active_line(ui, "tabs-carbon.png", 1380, 900, (220, 163, 37), minimum=240)
            ui.hint("All open editors")
            key("Return")
            screenshot(window, "open-editors.png")
            ui.hint("README.md")
            key("Return")
            append_save(ui, root / "README.md", "selected_from_editors_list")
            ui.click(
                "editor-tab:" + hashlib.sha256(str(root / "src/main.rs").encode()).hexdigest(),
                mouse_button=2,
            )  # Close inactive main.rs with the middle button.
            ui.absent(
                "editor-tab:" + hashlib.sha256(str(root / "src/main.rs").encode()).hexdigest()
            )
            key("Next", ("Control_L",))
            append_save(ui, root / "api/config.toml", "api_tab")
            key("Next", ("Control_L",))
            append_save(ui, root / "worker/config.toml", "worker_tab")
            screenshot(window, "duplicate-filenames.png")
            before_tab = (
                "editor-tab:" + hashlib.sha256(str(root / "api/config.toml").encode()).hexdigest()
            )
            moved_tab = "editor-tab:" + hashlib.sha256(str(root / "README.md").encode()).hexdigest()
            # Follow actual tab rectangles. The project toolbar, host banner and
            # font scale determine their position; fixed screen coordinates do not.
            for _ in range(len(names)):
                if any(b["name"] == moved_tab for b in ui.layout()["buttons"]):
                    break
                ui.hint("Scroll tabs left")
                key("Return")
            else:
                raise AssertionError("The first tab did not become visible after scrolling")
            start = ui.control(moved_tab, hovered=True)
            target = ui.control(before_tab)
            state = ui.layout()
            rect = next(b["rect"] for b in state["buttons"] if b["name"] == before_tab)
            end = (
                target[0] + round((rect[2] - rect[0]) * state["pixels_per_point"] / 4),
                target[1],
            )
            drag(start, end)
            ui.layout(
                lambda state: (
                    any(b["name"] == before_tab for b in state["buttons"])
                    and any(b["name"] == moved_tab for b in state["buttons"])
                    and next(i for i, b in enumerate(state["buttons"]) if b["name"] == before_tab)
                    < next(i for i, b in enumerate(state["buttons"]) if b["name"] == moved_tab)
                ),
                "Dragged tab did not move after api/config.toml",
            )
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
            # The shortcut's focus was checked above. Notifications are allowed
            # to cover panes; dismiss any before measuring an unobscured border.
            if any(button["name"] == "notice-dismiss" for button in ui.layout()["buttons"]):
                ui.click("notice-dismiss")
                ui.absent("notice-dismiss")
                ui.navigate("t")
                terminal_focus(ui)
            assert_pane_outline(ui, "Terminal", "terminal-focus-compact.png")
            screenshot(window, "minimum-terminal.png")
            compact_terminal = next(
                button["rect"]
                for button in ui.layout()["buttons"]
                if button["name"].startswith("terminal-input:")
            )
            text("echo layout_terminal_ok > terminal.ok\n")
            wait_for(
                lambda: (root / "terminal.ok").exists(),
                "Terminal controls/content were lost in the narrow layout",
            )
            key("t", ("Control_L", "Shift_L"))  # Return to full-width code, not a split pane.
            ui.navigate("f")
            assert_pane_outline(ui, "Explorer", "explorer-focus-compact.png")
            ui.navigate("e")
            ui.control("source-editor", focused=True)
            key("f", ("Control_L",))
            text("layout")
            screenshot(window, "minimum-window.png")
            active_line(
                ui,
                "minimum-window.png",
                850,
                560,
                (220, 163, 37),
                minimum=140,
            )
            key("Escape")
            append_save(ui, root / "README.md", "focus_after_find")
            append_save(ui, root / "README.md", "focus_after_terminal")
            ui.command("Git: Source control")
            ui.control("git-tab-changes")
            screenshot(window, "minimum-git.png")
            ui.command("Git: Source control")  # Hide panel.
            ui.resize(1380, 900)
            ui.command("Theme: Phosphor")
            screenshot(window, "tabs-phosphor.png")
            active_line(ui, "tabs-phosphor.png", 1380, 900, (125, 235, 164))
            key("t", ("Control_L", "Shift_L"))
            terminal_focus(ui)
            screenshot(window, "terminal-full-height.png")
            text("stty size > terminal.size\n")
            wait_for(
                lambda: (root / "terminal.size").exists(),
                "Terminal input stopped working after expanding to full height",
            )
            rows, _ = map(int, (root / "terminal.size").read_text().split())
            controls = {button["name"]: button["rect"] for button in ui.layout()["buttons"]}
            terminal = next(
                rect for name, rect in controls.items() if name.startswith("terminal-input:")
            )
            cell = controls["terminal-first-cell"]
            height = terminal[3] - terminal[1]
            row_height = cell[3] - cell[1]
            # A different font can fit fewer rows without losing usable space.
            # Check viewport growth and the PTY's agreement with painted rows,
            # rather than assuming a particular font's 40-row capacity.
            assert height > (compact_terminal[3] - compact_terminal[1]) * 1.5, (
                "Terminal height is still capped",
                compact_terminal,
                terminal,
            )
            assert abs(terminal[3] - controls["pane-Terminal"][3]) <= 1, controls
            assert height - 2 * row_height <= rows * row_height <= height, (
                "PTY rows disagree with the visible terminal",
                rows,
                cell,
                terminal,
            )
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
