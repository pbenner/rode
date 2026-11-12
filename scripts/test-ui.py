#!/usr/bin/env python3
"""Native editor and theme regression checks on an isolated X display."""

import json
import re
import subprocess
import time
from pathlib import Path

from ui_harness import (
    ARTIFACTS,
    BINARY,
    NativeUI,
    assert_running,
    background,
    click_button,
    close_window,
    decode_ron_string,
    fixture_directory,
    fixture_environment,
    focus_window,
    key,
    saved_theme,
    screenshot,
    text,
    theme,
    wait_for_project,
    wait_for_window,
)

with fixture_directory(prefix="rode-ui-") as base:
    directory = str(base)
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
    (Path(directory) / "data").mkdir(mode=0o700)
    (Path(directory) / "scripts").mkdir()
    log = (ARTIFACTS / "app.log").open("w")
    environment = fixture_environment(
        base,
        XDG_CONFIG_HOME=directory + "/config",
        XDG_DATA_HOME=directory + "/data-home",
        RODE_UI_TEST_STATE=directory + "/ui-state.json",
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
        wait_for_project(window, process, ARTIFACTS / "app.log", base)
        ui = NativeUI(
            window, process, ARTIFACTS / "app.log", Path(environment["RODE_UI_TEST_STATE"]), base
        )
        ui.click("source-editor")
        ui.document(path)
        screenshot(window, "startup-file.png")
        screenshot(window, "opened.png")
        before = ui.layout()["pass"]
        key("End", ("Control_L",))
        original = path.read_text()
        text("\n// native-smoke-saved")
        ui.source(original + "\n// native-smoke-saved", changed_after=before)
        key("s", ("Control_L",))
        ui.wait(lambda: "native-smoke-saved" in path.read_text(), "Editing + Ctrl+S failed")
        ui.control("source-editor-saved")
        screenshot(window, "editor.png")
        saved = path.read_text()
        key("f", ("Control_L",))
        ui.control("source-find", focused=True)
        ui.fill("source-find", "run_step")
        before = ui.layout()["pass"]
        key("Return")
        ui.control("source-find", after_pass=before)
        key("Escape")
        ui.absent("source-find")
        ui.control("source-editor", focused=True)
        before = ui.layout()["pass"]
        key("s", ("Control_L",))
        ui.control("source-editor-saved", after_pass=before)
        assert path.read_text() == saved, "Find / Enter changed the document"
        key("t", ("Control_L", "Shift_L"))
        ui.terminal_input()
        text("echo RODE_NATIVE_TERMINAL_OK > terminal.ok\n")
        ui.wait(lambda: (base / "terminal.ok").exists(), "Terminal did not execute input")
        assert (base / "terminal.ok").read_text() == "RODE_NATIVE_TERMINAL_OK\n"
        screenshot(window, "terminal.png")
        ui.command("Remote: Connect via SSH")
        ui.control("ssh-field-Host / alias")
        screenshot(window, "connection.png")
        key("Escape")
        ui.absent("ssh-field-Host / alias")
        ui.terminal_input()
        text("RODE_THEME_SESSION=kept\n")
        theme("carbon", ui=ui)
        saved_theme(directory, "carbon")
        ui.navigate("e")
        ui.document(path)
        background(window, "carbon.png", (16, 16, 16))
        # The status-bar menu contains built-ins plus the custom fixture.
        # Exercise the visible UI control as well as command-palette selection.
        click_button(
            window,
            process,
            ARTIFACTS / "app.log",
            environment["RODE_UI_TEST_STATE"],
            "appearance-menu",
        )
        screenshot(window, "status-theme-selector.png")
        click_button(
            window,
            process,
            ARTIFACTS / "app.log",
            environment["RODE_UI_TEST_STATE"],
            "interface-theme:phosphor",
        )
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
            theme(preset.replace("-", " "), ui=ui)
            saved_theme(directory, preset)
            background(window, preset + ".png", color)
        theme("Code Dark", ui=ui)
        saved_theme(directory, "code-dark")
        background(window, "code-dark.png", (30, 30, 30))
        theme("smoke custom", ui=ui)
        saved_theme(directory, "custom:smoke")
        background(window, "custom.png", (21, 18, 26))
        click_button(
            window,
            process,
            ARTIFACTS / "app.log",
            environment["RODE_UI_TEST_STATE"],
            "appearance-menu",
        )
        click_button(
            window,
            process,
            ARTIFACTS / "app.log",
            environment["RODE_UI_TEST_STATE"],
            "interface-theme:code-dark",
        )
        saved_theme(directory, "code-dark")
        theme("Studio Neutral", ui=ui)
        saved_theme(directory, "studio-neutral")
        ui.source(saved)
        before = ui.layout()["pass"]
        key("s", ("Control_L",))
        ui.control("source-editor-saved", after_pass=before)
        assert path.read_text() == saved, "Changing themes must preserve document contents"
        ui.navigate("t")
        ui.terminal_input()
        text("echo $RODE_THEME_SESSION > session.ok\n")
        ui.wait(
            lambda: (base / "session.ok").exists(), "Terminal did not finish after theme changes"
        )
        assert (base / "session.ok").read_text().strip() == "kept", (
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
        window = wait_for_window(process, ARTIFACTS / "app.log")
        wait_for_project(window, process, ARTIFACTS / "app.log", base)
        focus_window(window)
        ui = NativeUI(
            window, process, ARTIFACTS / "app.log", Path(environment["RODE_UI_TEST_STATE"]), base
        )
        ui.control("appearance-menu")
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
with fixture_directory(prefix="rode-file-ui-") as home:
    directory = str(home)
    path = home / "individual file.txt"
    path.write_text("standalone contents\n")
    preferences = home / "data-home/rode/app.ron"
    preferences.parent.parent.mkdir(mode=0o700)
    preferences.parent.mkdir(parents=True, mode=0o700)
    defaults = {"enabled": True}
    preferences.write_text(json.dumps({"sandbox_defaults": json.dumps(defaults)}))
    environment = fixture_environment(
        home,
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
            ui = NativeUI(
                window,
                process,
                ARTIFACTS / "standalone-file.log",
                Path(environment["RODE_UI_TEST_STATE"]),
            )
            ui.click("source-editor")
            ui.document(path)
            key("End", ("Control_L",))
            text("saved without a project")
            ui.source("standalone contents\nsaved without a project")
            key("s", ("Control_L",))
            ui.wait(
                lambda: path.read_text() == "standalone contents\nsaved without a project",
                "Standalone Ctrl+S did not save to the original file",
            )
            ui.control("source-editor-saved")
            close_window(window, process)
            saved = re.search(
                r'"sandbox_defaults"\s*:\s*("(?:\\.|[^"\\])*")', preferences.read_text()
            )
            assert saved and json.loads(decode_ron_string(saved[1])) == defaults, (
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
