#!/usr/bin/env python3
"""Native PTY contrast, palette edits, per-theme persistence and live-shell checks."""

import json
import os
import shlex
import shutil
import subprocess
import time
from collections import Counter
from pathlib import Path

os.environ["RODE_UI_ARTIFACTS"] = str(
    Path(__file__).resolve().parent.parent / "target/terminal-colors-ui"
)
from ui_fixture import project_ui
from ui_harness import (
    ARTIFACTS,
    BINARY,
    ROOT,
    close_window,
    fixture_directory,
    fixture_environment,
    focus_window,
    key,
    screenshot,
    text,
    theme,
    wait_for_window,
)
from ui_harness import (
    read_preferences as preferences,
)


def wait_for(check, message):
    deadline = time.monotonic() + 10
    while time.monotonic() < deadline:
        if check():
            return
        time.sleep(0.05)
    raise AssertionError(message)


def window_for(process, title="RODE"):
    window = wait_for_window(process, ARTIFACTS / "bash-prompt.log")
    focus_window(window)
    return window


def colors():
    ui.command("Terminal: Colors and contrast")
    ui.control("terminal-colors-theme")


def press(name, modifiers=()):
    before = ui.layout()["pass"]
    key(name, modifiers)
    ui.layout(lambda state: state["pass"] > before, "Color control did not process " + name)


def contrast(value, theme_id="phosphor"):
    stored = preferences(saved).get(f"terminal_color.{theme_id}.minimum_contrast", "1")
    current = float(stored) if stored != "theme" else 1.0
    ui.hint("Terminal minimum contrast")
    ui.control("terminal-colors-contrast", focused=True)
    steps = round((float(value) - current) / 0.5)
    for _ in range(abs(steps)):
        press("Right" if steps > 0 else "Left")


def edit_red(label, screenshot_name):
    # Enter egui's native picker through its named swatch. Its pinned native
    # tab order reaches the numeric row (type, copy, R); no popup coordinates
    # depend on the interface font, title height or placement on the display.
    ui.hint(label)
    press("Return")
    for _ in range(4):
        press("Tab")
    screenshot(window, screenshot_name)
    press("a", ("Control_L",))
    text("240")
    press("Return")
    press("Escape")


def close_colors():
    ui.click("terminal-colors-close")
    ui.absent("terminal-colors-theme")


def terminal_crop(*, columns=None, rows=None):
    """Use the observed grid and pixel scale, independently of sidebar/header geometry."""
    ui.control("terminal-first-cell")
    state = ui.layout()
    first = next(b["rect"] for b in state["buttons"] if b["name"] == "terminal-first-cell")
    area = next(b["rect"] for b in state["buttons"] if b["name"].startswith("terminal-input:"))
    left, top = first[:2]
    right = area[2] if columns is None else min(area[2], left + columns * (first[2] - left))
    bottom = area[3] if rows is None else min(area[3], top + rows * (first[3] - top))
    left, top, right, bottom = (
        round(value * state["pixels_per_point"]) for value in (left, top, right, bottom)
    )
    return f"{max(1, right - left)}x{max(1, bottom - top)}{left:+}{top:+}"


def bright_pixels(filename):
    """Count readable neutral glyph pixels in the three deliberately dark lines."""
    rgb = subprocess.check_output(
        [
            "convert",
            str(ARTIFACTS / filename),
            "-crop",
            terminal_crop(columns=12, rows=3),
            "-depth",
            "8",
            "rgb:-",
        ]
    )
    return sum(min(rgb[i : i + 3]) > 90 for i in range(0, len(rgb), 3))


def contrast_pixels(filename, *, visible):
    def painted():
        screenshot(window, filename)
        count = bright_pixels(filename)
        return count > 100 if visible else count < 10

    wait_for(painted, f"{filename}: terminal contrast did not render")


def prompt_colors(window, filename, palette, crop=None, failed=False):
    """Check prompt ink including antialiasing against its terminal background."""

    def painted():
        screenshot(window, filename)
        command = ["convert", str(ARTIFACTS / filename)]
        if crop:
            command += ["-crop", crop]
        rgb = subprocess.check_output(command + ["-depth", "8", "rgb:-"])
        colors = Counter(tuple(rgb[i : i + 3]) for i in range(0, len(rgb), 3))
        background = tuple(
            bytes.fromhex(palette["colors"]["terminal_background"].removeprefix("#"))
        )
        # PS1 changes blue to magenta without clearing bold, so Git is bright too.
        for index in [8, 12, 13, 9 if failed else 10]:
            value = palette["ansi"][index].removeprefix("#")
            expected = tuple(bytes.fromhex(value))
            channel = max(range(3), key=lambda c: abs(expected[c] - background[c]))
            pixels = 0
            for color, count in colors.items():
                alpha = (color[channel] - background[channel]) / (
                    expected[channel] - background[channel]
                )
                if 0.6 <= alpha <= 1.02 and all(
                    abs(color[c] - (background[c] + alpha * (expected[c] - background[c]))) <= 2
                    for c in range(3)
                ):
                    pixels += count
            if pixels <= 3:
                return False
        return True

    wait_for(painted, f"{filename}: missing prompt ANSI colors")


with fixture_directory(prefix="rode-terminal-colors-") as directory:
    root = Path(directory)
    shell = root / "fixture-shell"
    shell.write_text(
        "#!/bin/sh\n"
        "printf '\\033[2J\\033[H\\033[?25l\\033[30mANSI_DARK\\033[0m\\n"
        "\\033[38;5;17mINDEX_DARK\\033[0m\\n"
        "\\033[38;2;9;10;11mRGB_DARK\\033[0m\\n'\n"
        ": > " + shlex.quote(str(root / "dark-ready")) + "\n"
        "exec /bin/sh -i\n"
    )
    shell.chmod(0o700)
    environment = fixture_environment(
        root,
        XDG_CONFIG_HOME=str(root / "config"),
        XDG_DATA_HOME=str(root / "data"),
        WINIT_X11_SCALE_FACTOR="1",
        SHELL=str(shell),
    )
    saved = root / "data/rode/app.ron"
    for run in (
        () if os.environ.get("RODE_TERMINAL_COLORS_UI_PROMPT_ONLY") else ("initial", "restart")
    ):
        (root / "dark-ready").unlink(missing_ok=True)
        with (ARTIFACTS / f"{run}.log").open("w") as log:
            process = subprocess.Popen(
                [str(BINARY), str(root), *(["--theme", "phosphor"] if run == "initial" else [])],
                env=environment,
                stdout=log,
                stderr=log,
            )
            window = None
            try:
                ui = project_ui(process, environment, root, log.name)
                window = ui.window
                key("t", ("Control_L", "Shift_L"))
                ui.terminal_input()
                wait_for(lambda: (root / "dark-ready").exists(), "Dark-text fixture did not start")
                if run == "initial":
                    contrast_pixels("contrast-default.png", visible=False)
                    colors()
                    screenshot(window, "settings.png")
                    contrast(4.5)
                    wait_for(
                        lambda: (
                            preferences(saved).get("terminal_color.phosphor.minimum_contrast")
                            == "4.5"
                        ),
                        "Optional contrast protection was not saved",
                    )
                    # The terminal now fills the workspace. Close the dialog
                    # before measuring its first three lines, which it covers.
                    close_colors()
                    contrast_pixels("contrast-enabled.png", visible=True)
                    colors()
                    contrast(1)
                    wait_for(
                        lambda: (
                            preferences(saved).get("terminal_color.phosphor.minimum_contrast")
                            == "1"
                        ),
                        "Exact-color mode was not saved immediately",
                    )
                    close_colors()
                    contrast_pixels("contrast-off.png", visible=False)
                    colors()
                    edit_red("Terminal Foreground color", "foreground-picker.png")
                    wait_for(
                        lambda: (
                            preferences(saved).get("terminal_color.phosphor.foreground")
                            == "#f0dad8"
                        ),
                        "Foreground edit was not saved",
                    )
                    ui.click("terminal-colors-ansi")
                    ui.control("terminal-colors-ansi-0")
                    edit_red("ANSI Black color", "ansi-picker.png")
                    wait_for(
                        lambda: (
                            preferences(saved).get("terminal_color.phosphor.ansi.0") == "#f02224"
                        ),
                        "ANSI swatch edit was not saved",
                    )
                    screenshot(window, "ansi-edited.png")
                    close_colors()
                    theme("Carbon", ui=ui)
                    colors()
                    contrast(7, "carbon")
                    wait_for(
                        lambda: (
                            preferences(saved).get("terminal_color.carbon.minimum_contrast") == "7"
                        ),
                        "Second theme did not save independently",
                    )
                    close_colors()
                    theme("Phosphor", ui=ui)
                    assert preferences(saved)["terminal_color.phosphor.minimum_contrast"] == "1"
                    ui.navigate("t")
                    ui.terminal_input()
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
                    # Collapse a restored open section before the footer action.
                    if any(b["name"] == "terminal-colors-ansi-0" for b in ui.layout()["buttons"]):
                        ui.click("terminal-colors-ansi")
                        ui.absent("terminal-colors-ansi-0")
                    ui.click("terminal-colors-reset")
                    wait_for(
                        lambda: (
                            preferences(saved).get("terminal_color.phosphor.minimum_contrast")
                            == "theme"
                        ),
                        "Reset did not restore theme defaults",
                    )
                    assert preferences(saved)["terminal_color.phosphor.ansi.0"] == "theme"
                    assert preferences(saved)["terminal_color.carbon.minimum_contrast"] == "7"
                    close_colors()
                    contrast_pixels("reset.png", visible=False)
                    colors()
                    ui.resize(850, 560)
                    ui.click("terminal-colors-ansi")
                    ui.control("terminal-colors-ansi-0")
                    screenshot(window, "minimum-window.png")
                    ui.scroll_to_control("terminal-colors-ansi-15", over="terminal-colors-ansi-0")
                    screenshot(window, "minimum-window-scrolled.png")
                close_window(window, process)
            except Exception:
                if window is not None and process.poll() is None:
                    screenshot(window, f"{run}-failure.png")
                raise
            finally:
                if process.poll() is None:
                    process.terminate()
                    process.wait(timeout=10)

    # Reproduce a real PS1 with status command substitution, Git expansion,
    # readline's nonprinting markers, and bold-black host/prompt plus bold blue
    # path. Keep test startup independent of the user's own shell configuration.
    rcfile = root / "prompt.bashrc"
    rcfile.write_text(
        r"""unset PROMPT_COMMAND
__git_ps1() { printf ' branch'; }
PS1='`if [ $? = 0 ]; then echo "\[\033[01;32m\]✔"; else echo "\[\033[01;31m\]✘"; fi` \[\033[01;30m\]\h\[\033[01;34m\] \w\[\033[35m\]$(__git_ps1 " %s") \[\033[01;30m\]>\[\033[00m\] '
"""
    )
    shell.write_text(
        "#!/bin/sh\nexec /bin/bash --noprofile --rcfile " + shlex.quote(str(rcfile)) + " -i\n"
    )
    prompt_environment = dict(
        environment,
        XDG_CONFIG_HOME=str(root / "prompt-config"),
        XDG_DATA_HOME=str(root / "prompt-data"),
    )
    palette = json.loads((ROOT / "themes/carbon.json").read_text())
    with (ARTIFACTS / "bash-prompt.log").open("w") as log:
        process = subprocess.Popen(
            [str(BINARY), "--theme", "carbon", str(root)],
            env=prompt_environment,
            stdout=log,
            stderr=log,
        )
        reference = None
        try:
            ui = project_ui(process, prompt_environment, root, log.name)
            window = ui.window
            key("t", ("Control_L", "Shift_L"))
            ui.terminal_input()
            text("echo $$ > palette-before.pid\n")
            wait_for(
                lambda: (root / "palette-before.pid").exists(), "No shell before theme switches"
            )
            for name in ["carbon", "amber", "matrix", "ice", "neon", "code-dark"]:
                theme(name.replace("-", " "), ui=ui)
                current = json.loads((ROOT / f"themes/{name}.json").read_text())
                text("true\n")
                prompt_colors(window, name + "-prompt.png", current, terminal_crop())
                text("false\n")
                prompt_colors(
                    window, name + "-prompt-failed.png", current, terminal_crop(), failed=True
                )
            text("echo $$ > palette-after.pid\n")
            wait_for(
                lambda: (root / "palette-after.pid").exists(), "Theme switches lost shell input"
            )
            assert (root / "palette-before.pid").read_text() == (
                root / "palette-after.pid"
            ).read_text()
            theme("carbon", ui=ui)
            # Compare with xterm when installed; it is a test reference, never
            # an editor dependency. Pin its palette to compare ANSI semantics.
            if shutil.which("xterm"):
                command = [
                    "xterm",
                    "-T",
                    "RODE-PROMPT-XTERM",
                    "-geometry",
                    "100x12",
                    "-fa",
                    # Match egui's grayscale antialiasing for RGB pixel checks,
                    # independent of the desktop's LCD/subpixel font settings.
                    "monospace:rgba=none",
                    "-fs",
                    "11",
                    "-tn",
                    "xterm-256color",
                    "-bg",
                    palette["colors"]["terminal_background"],
                    "-fg",
                    palette["colors"]["terminal_foreground"],
                    "-xrm",
                    "*boldColors: true",
                ]
                for index, color in enumerate(palette["ansi"]):
                    command += ["-xrm", f"*color{index}: {color}"]
                command += ["-e", "/bin/bash", "--noprofile", "--rcfile", str(rcfile), "-i"]
                reference = subprocess.Popen(
                    command,
                    cwd=root,
                    env=prompt_environment,
                    stdout=log,
                    stderr=log,
                )
                reference_window = window_for(reference, "RODE-PROMPT-XTERM")
                prompt_colors(reference_window, "xterm-prompt.png", palette)
                text("false\n")
                prompt_colors(reference_window, "xterm-prompt-failed.png", palette, failed=True)
                reference.terminate()
                reference.wait(timeout=10)
            else:
                print("xterm reference check skipped: xterm is not installed")
            focus_window(window)
            close_window(window, process)
        finally:
            for child in (reference, process):
                if child is not None and child.poll() is None:
                    child.terminate()
                    child.wait(timeout=10)
print(f"Terminal color UI checks passed; artifacts: {ARTIFACTS}")
