#!/usr/bin/env python3
"""Independent syntax palettes, dirty buffers, menus and saved choices in native UI."""

import os
import re
import subprocess
from functools import partial
from pathlib import Path

os.environ["RODE_UI_ARTIFACTS"] = str(
    Path(__file__).resolve().parent.parent / "target/syntax-themes-ui"
)
from ui_fixture import project_ui
from ui_harness import (
    ARTIFACTS,
    BINARY,
    background,
    click,
    close_window,
    fixture_directory,
    fixture_environment,
    key,
    screenshot,
    text,
    wait_for_effect,
)

wait_for = partial(wait_for_effect, timeout=8)


def run():
    with fixture_directory(prefix="rode-syntax-ui-") as base:
        project = base / "project"
        project.mkdir()
        source = project / "main.rs"
        original = """//! Scoped syntax colors; UI and terminal colors stay independent.
use std::collections::HashMap;

#[derive(Debug, Clone)]
struct Packet<'a> {
    label: &'a str,
    retries: u32,
}

fn dispatch<'a>(packet: &Packet<'a>) -> Option<String> {
    let timeout_ms = 1_000;
    let escaped = "hello\\nworld";
    let message = format!("{}: {:?}", packet.label, timeout_ms);
    // A comment with unicode: 🦀
    if packet.retries > 3 { return None; }
    Some(message)
}
"""
        source.write_text(original)
        environment = fixture_environment(base)
        custom = base / "config/rode/syntax-themes"
        custom.mkdir(parents=True)
        (custom / "custom.json").write_text(
            '{"name":"Custom Scope","base":"dracula","rules":['
            '{"scope":"source.rust comment","foreground":"#9988aa","font_style":"italic"}]}'
        )
        prefs = base / "data/rode/app.ron"

        def saved(key_name, expected):
            if not prefs.exists():
                return False
            value = re.search(r'"' + key_name + r'"\s*:\s*"([^"]+)"', prefs.read_text())
            return value and value[1] == expected

        for restart in [False, True]:
            args = [str(BINARY), str(project)]
            if not restart:
                args += ["--theme", "carbon"]
            with (ARTIFACTS / ("restart.log" if restart else "native.log")).open("w") as log:
                process = subprocess.Popen(args, env=environment, stdout=log, stderr=log)
                window = None
                try:
                    ui = project_ui(process, environment, project, log.name)
                    window = ui.window
                    command = ui.command
                    ui.open_file("main.rs")
                    if restart:
                        assert saved("syntax_theme", "nord") and saved("color_theme", "carbon")
                        background(window, "restored-nord.png", (16, 16, 16))
                        close_window(window, process)
                        continue
                    click(550, 200)
                    key("End", ("Control_L",))
                    text("\n// dirty_syntax_marker")
                    frames = set()
                    for name, identity in [
                        ("Dracula", "dracula"),
                        ("Catppuccin Mocha", "catppuccin-mocha"),
                        ("Nord", "nord"),
                        ("Monokai", "monokai"),
                        ("One Half Dark", "one-half-dark"),
                        ("Custom Scope", "custom:custom"),
                        ("Follow UI theme", "ui"),
                    ]:
                        command("Syntax theme: " + name)
                        wait_for(
                            lambda: saved("syntax_theme", identity),
                            "Syntax choice not saved immediately",
                        )
                        assert saved("color_theme", "carbon"), (
                            "Syntax selection changed the UI preference"
                        )
                        assert source.read_text() == original, (
                            "Switching palettes saved a dirty buffer"
                        )
                        filename = identity.replace(":", "-") + ".png"
                        background(window, filename, (16, 16, 16))
                        pixels = subprocess.check_output(
                            [
                                "convert",
                                str(ARTIFACTS / filename),
                                "-crop",
                                "950x350+365+130",
                                "rgb:-",
                            ]
                        )
                        frames.add(pixels)
                    assert len(frames) == 7, "Unchanged source retained stale syntax colors"
                    command("Syntax theme: Nord")
                    command("Save file")
                    wait_for(
                        lambda: "dirty_syntax_marker" in source.read_text(), "Dirty text was lost"
                    )
                    assert (
                        "fn dispatch" in source.read_text()
                        and "let timeout_ms" in source.read_text()
                    )
                    click(200, 24)
                    screenshot(window, "view-menu.png")
                    key("Escape")
                    click(1310, 886)
                    screenshot(window, "status-menu.png")
                    key("Escape")
                    close_window(window, process)
                except Exception:
                    if window and process.poll() is None:
                        screenshot(window, "failure.png")
                    raise
                finally:
                    if process.poll() is None:
                        process.terminate()
                        process.wait(timeout=5)
        print(
            "Native independent syntax colors, custom scopes, dirty buffers and restart persistence passed."
        )


if __name__ == "__main__":
    run()
