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
    background,
    click,
    close_window,
    display,
    key,
    saved_theme,
    screenshot,
    text,
    theme,
    x11,
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
    process = subprocess.Popen([str(BINARY), directory], env=environment, stdout=log, stderr=log)
    try:
        window = None
        for _ in range(100):
            tree = subprocess.check_output(["xwininfo", "-root", "-tree"], text=True)
            match = re.search(r'(0x[0-9a-f]+) "RODE', tree)
            if match:
                window = int(match[1], 16)
                break
            assert process.poll() is None, (ARTIFACTS / "app.log").read_text()
            time.sleep(0.05)
        assert window, "RODE window did not appear"
        time.sleep(0.5)  # winit creates the window before mapping its first frame
        x11.XSetInputFocus(display, window, 1, 0)
        x11.XFlush(display)
        time.sleep(0.4)
        screenshot(window, "welcome.png")
        key("p", ("Control_L",))
        text("main.rs")
        key("Return")
        time.sleep(0.5)
        screenshot(window, "opened.png")
        click(600, 180)
        key("End", ("Control_L",))
        text("\n// native-smoke-saved")
        key("s", ("Control_L",))
        time.sleep(0.6)
        assert "native-smoke-saved" in path.read_text(), (
            "Editing + Ctrl+S failed; inspect opened.png"
        )
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
        key("grave", ("Control_L",))
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
        # The status-bar menu has seven built-ins plus the custom fixture.
        # Exercise the visible UI control as well as command-palette selection.
        click(1310, 887)
        screenshot(window, "status-theme-selector.png")
        click(1290, 649)  # Phosphor, first menu row
        saved_theme(directory, "phosphor")
        background(window, "phosphor.png", (13, 18, 20))
        for preset, color in [
            ("matrix", (6, 12, 8)),
            ("amber", (16, 12, 7)),
            ("ice", (8, 15, 26)),
            ("neon", (17, 11, 27)),
        ]:
            theme(preset)
            saved_theme(directory, preset)
            background(window, preset + ".png", color)
        theme("Code Dark")
        saved_theme(directory, "code-dark")
        background(window, "code-dark.png", (30, 30, 30))
        theme("smoke custom")
        saved_theme(directory, "custom:smoke")
        background(window, "custom.png", (21, 18, 26))
        click(1310, 887)
        click(1290, 699)  # Code Dark, third menu row
        saved_theme(directory, "code-dark")
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
        background(window, "theme-restored.png", (30, 30, 30))
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
