#!/usr/bin/env python3
"""Native editor and theme regression checks on an isolated X display."""

import json
import os
import re
import subprocess
import tempfile
import time
from pathlib import Path

from ui_harness import (
    ARTIFACTS,
    BINARY,
    assert_running,
    background,
    click,
    close_window,
    focus_window,
    key,
    saved_theme,
    screenshot,
    text,
    theme,
    wait_for_window,
)

with tempfile.TemporaryDirectory(prefix="rode-ui-") as directory:
    path = Path(directory) / "main.rs"
    path.write_text("""use std::{thread, time::Duration};

// A small program. A remote workspace. No editor server.
const REMOTE_HOST: &str = "workstation.example.org";

fn main() {
    println!("connected to {REMOTE_HOST}");

    for step in 0..4 {
        run_step(step);
        thread::sleep(Duration::from_millis(20));
    }
}

fn run_step(step: usize) {
    let workers = 32;
    let result: Vec<_> = (0..workers)
        .map(|worker| worker * step)
        .collect();

    println!("step {step}: {result:?}");
}
""")
    (Path(directory) / "Cargo.toml").write_text(
        '[package]\nname = "remote-job"\nversion = "0.1.0"\n'
    )
    (Path(directory) / "data").mkdir()
    (Path(directory) / "scripts").mkdir()
    log = (ARTIFACTS / "app.log").open("w")
    environment = dict(
        os.environ, XDG_CONFIG_HOME=directory + "/config", XDG_DATA_HOME=directory + "/data-home"
    )
    theme_dir = Path(directory) / "config/rode/themes"
    theme_dir.mkdir(parents=True)
    (theme_dir / "smoke.json").write_text(
        json.dumps(
            {
                "name": "Smoke custom",
                "base": "carbon",
                "colors": {"background": "#15121a", "accent": "#bbaaff"},
            }
        )
    )
    # Launch the actual CLI with a file, without a picker that could hide a
    # startup regression. The restart below separately covers directory input.
    process = subprocess.Popen([str(BINARY), str(path)], env=environment, stdout=log, stderr=log)
    try:
        window = wait_for_window(process, ARTIFACTS / "app.log")
        focus_window(window)
        deadline = time.monotonic() + 20
        while True:
            assert_running(process, ARTIFACTS / "app.log")
            attributes = subprocess.check_output(["xwininfo", "-id", hex(window)], text=True)
            if Path(directory).name in attributes and "[Opening project]" not in attributes:
                break
            if time.monotonic() >= deadline:
                screenshot(window, "startup-failure.png")
                raise AssertionError("File startup did not prepare its containing directory")
            time.sleep(0.05)
        time.sleep(0.4)
        screenshot(window, "startup-file.png")
        time.sleep(0.5)
        screenshot(window, "opened.png")
        click(600, 180)
        key("End", ("Control_L",))
        text("\n// native-smoke-saved")
        key("s", ("Control_L",))
        # Formatting and the protected write run asynchronously. A cold
        # formatter on a CI runner need not finish within one frame delay.
        deadline = time.monotonic() + 15
        while "native-smoke-saved" not in path.read_text():
            assert_running(process, ARTIFACTS / "app.log")
            if time.monotonic() >= deadline:
                screenshot(window, "save-failure.png")
                raise AssertionError("Editing + Ctrl+S failed; inspect save-failure.png")
            time.sleep(0.05)
        screenshot(window, "editor.png")
        key("f", ("Control_L",))
        text("run_step")
        key("Return")
        time.sleep(0.2)
        key("Escape")
        saved = path.read_text()
        key("s", ("Control_L",))
        time.sleep(0.3)
        assert path.read_text() == saved, (
            "Find / Enter must select a match without modifying the document"
        )
        key("t", ("Control_L", "Shift_L"))
        time.sleep(0.4)
        text("echo RODE_NATIVE_TERMINAL_OK > terminal.ok\n")
        time.sleep(0.4)
        assert (Path(directory) / "terminal.ok").exists(), "Terminal did not execute input"
        screenshot(window, "terminal.png")
        key("p", ("Control_L", "Shift_L"))
        text("remote: connect")
        key("Return")
        time.sleep(0.3)
        screenshot(window, "connection.png")
        key("Escape")
        time.sleep(0.3)
        click(600, 740)
        text("RODE_THEME_SESSION=kept\n")
        theme("carbon")
        saved_theme(directory, "carbon")
        key("space", ("Control_L",))
        key("e")  # Theme samples use the editor while the shell keeps running.
        background(window, "carbon.png", (16, 16, 16))
        # The status-bar menu contains built-ins plus the custom fixture.
        # Exercise the visible UI control as well as command-palette selection.
        click(1310, 887)
        screenshot(window, "status-theme-selector.png")
        click(1290, 560)  # Phosphor, first menu row
        saved_theme(directory, "phosphor")
        background(window, "phosphor.png", (13, 18, 20))
        for preset, color in [
            ("carbon-edge", (16, 16, 16)),
            ("matrix", (6, 12, 8)),
            ("amber", (16, 12, 7)),
            ("ice", (8, 15, 26)),
            ("neon", (17, 11, 27)),
            ("afterglow", (19, 18, 25)),
            ("knight-rider", (9, 10, 12)),
            ("hologram", (7, 18, 31)),
            ("telemetry", (5, 15, 18)),
            ("studio", (23, 26, 33)),
            ("studio-neutral", (26, 27, 30)),
            ("studio-hacker", (16, 21, 18)),
        ]:
            theme(preset.replace("-", " "))
            saved_theme(directory, preset)
            background(window, preset + ".png", color)
        theme("Code Dark")
        saved_theme(directory, "code-dark")
        background(window, "code-dark.png", (30, 30, 30))
        theme("smoke custom")
        saved_theme(directory, "custom:smoke")
        background(window, "custom.png", (21, 18, 26))
        click(1310, 887)
        click(1290, 635)  # Code Dark, fourth menu row
        saved_theme(directory, "code-dark")
        theme("Studio Neutral")
        saved_theme(directory, "studio-neutral")
        key("s", ("Control_L",))
        time.sleep(0.3)
        assert path.read_text() == saved, "Changing themes must preserve document contents"
        key("space", ("Control_L",))
        key("t")
        click(600, 740)
        text("echo $RODE_THEME_SESSION > session.ok\n")
        time.sleep(0.3)
        assert (Path(directory) / "session.ok").read_text().strip() == "kept", (
            "Theme switch restarted the terminal"
        )

        # Exclude startup and editing work from the idle sample.
        def ticks():
            fields = Path(f"/proc/{process.pid}/stat").read_text().split()
            return int(fields[13]) + int(fields[14])

        before = ticks()
        time.sleep(1.0)
        after = ticks()
        memory = re.search(r"VmRSS:\s+(\d+) kB", Path(f"/proc/{process.pid}/status").read_text())
        print("Native open/edit/save, find, terminal, and connection dialog smoke completed.")
        print(
            f"Idle CPU sample: {after - before} scheduler ticks / 1 s; RSS: {int(memory[1]) / 1024:.1f} MiB"
        )
        print(f"Screenshots: {ARTIFACTS}")
        close_window(window, process)
        # A real native restart exercises eframe's persistence, without --theme.
        process = subprocess.Popen(
            [str(BINARY), directory], env=environment, stdout=log, stderr=log
        )
        window = None
        for _ in range(100):
            tree = subprocess.check_output(["xwininfo", "-root", "-tree"], text=True)
            match = re.search(r'(0x[0-9a-f]+) "RODE', tree)
            if match:
                window = int(match[1], 16)
                break
            assert process.poll() is None
            time.sleep(0.05)
        assert window
        time.sleep(0.7)
        background(window, "theme-restored.png", (26, 27, 30))
        close_window(window, process)
        print(
            "Built-in/custom theme switching, immediate config writes, terminal preservation, and native restart persistence passed."
        )
    finally:
        if process.poll() is None:
            process.terminate()
        try:
            process.wait(timeout=5)
        except subprocess.TimeoutExpired:
            process.kill()
            process.wait()
        log.close()

# A file directly in HOME must not be mistaken for a whole-home sandbox project.
with tempfile.TemporaryDirectory(prefix="rode-file-ui-") as directory:
    home = Path(directory)
    path = home / "individual file.txt"
    path.write_text("standalone contents\n")
    preferences = home / "data-home/rode/app.ron"
    preferences.parent.mkdir(parents=True)
    defaults = {"enabled": True}
    preferences.write_text(json.dumps({"sandbox_defaults": json.dumps(defaults)}))
    environment = dict(
        os.environ,
        HOME=directory,
        XDG_CONFIG_HOME=directory + "/config",
        XDG_DATA_HOME=directory + "/data-home",
    )
    with (ARTIFACTS / "standalone-file.log").open("w") as log:
        process = subprocess.Popen(
            [str(BINARY), path.name], cwd=directory, env=environment, stdout=log, stderr=log
        )
        try:
            window = wait_for_window(process, ARTIFACTS / "standalone-file.log")
            focus_window(window)
            deadline = time.monotonic() + 15
            while True:
                assert_running(process, ARTIFACTS / "standalone-file.log")
                attributes = subprocess.check_output(["xwininfo", "-id", hex(window)], text=True)
                if "[File mode]" in attributes:
                    break
                if time.monotonic() >= deadline:
                    screenshot(window, "standalone-file-failure.png")
                    raise AssertionError("HOME file did not open without a project")
                time.sleep(0.05)
            screenshot(window, "standalone-file.png")
            key("End", ("Control_L",))
            text("saved without a project")
            key("s", ("Control_L",))
            while path.read_text() != "standalone contents\nsaved without a project":
                assert_running(process, ARTIFACTS / "standalone-file.log")
                if time.monotonic() >= deadline:
                    screenshot(window, "standalone-save-failure.png")
                    raise AssertionError("Standalone Ctrl+S did not save to the original file")
                time.sleep(0.05)
            close_window(window, process)
            saved = re.search(
                r'"sandbox_defaults"\s*:\s*("(?:\\.|[^"\\])*")', preferences.read_text()
            )
            assert saved and json.loads(json.loads(saved[1])) == defaults, (
                "Standalone startup must not change project sandbox preferences"
            )
            print(
                "Relative HOME file startup, editing, saving and unchanged sandbox defaults passed."
            )
        finally:
            if process.poll() is None:
                process.terminate()
            try:
                process.wait(timeout=5)
            except subprocess.TimeoutExpired:
                process.kill()
                process.wait()
