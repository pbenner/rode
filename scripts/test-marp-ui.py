#!/usr/bin/env python3
"""Native Marp preview/export and renderer shutdown, locally and over SSH."""

import json
import os
import re
import shutil
import subprocess
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
os.environ["RODE_UI_ARTIFACTS"] = str(ROOT / "target/marp-ui")
from ui_fixture import native_session, project_args
from ui_harness import (
    ARTIFACTS,
    close_window,
    fixture_directory,
    fixture_environment,
    key,
    screenshot,
    text,
    wait_for_effect,
)


def preview_and_export(ui, project, source, original, label, real):
    ui.open_file("deck.marp.md")
    key("space", ("Control_L",))
    key("e")  # Select Code explicitly after opening the document.
    ui.control("source-editor", focused=True)
    key("End", ("Control_L",))
    text("\nUNSAVED_SNAPSHOT\n")
    ui.source(original + "\nUNSAVED_SNAPSHOT\n")
    ui.command("Marp: Toggle slide preview")

    def slide(number):
        filename = label + f"-slide-{number}.png"
        screenshot(ui.window, filename)
        if not real:
            sample = subprocess.check_output(
                [
                    "convert",
                    str(ARTIFACTS / filename),
                    "-format",
                    "%[pixel:p{840,600}]",
                    "info:",
                ],
                text=True,
            )
            red, green, blue = map(int, re.findall(r"\d+", sample)[:3])
            return (
                (red > 180 and green < 100 and blue < 100)
                if number == 1
                else (blue > 180 and red < 100)
            )
        pixels = subprocess.check_output(
            ["convert", str(ARTIFACTS / filename), "-depth", "8", "rgb:-"]
        )
        wanted = ((28, 35, 49), (22, 59, 44))[number - 1]
        return pixels.count(bytes(wanted)) > 15000

    ui.wait(lambda: slide(1), "Marp first slide was not rendered", timeout=35)
    assert source.read_text() == original, "Preview saved or changed the source"

    def focus_preview():
        ui.control("marp-ready")
        ui.click("image-preview")
        ui.control("image-preview", focused=True)

    focus_preview()
    key("Next")
    ui.wait(lambda: slide(2), "PageDown did not navigate slides", timeout=35)
    key("s", ("Control_L",))
    ui.wait(
        lambda: "UNSAVED_SNAPSHOT" in source.read_text(),
        "Saving from Slides lost the source",
    )
    key("f", ("Control_L",))  # Find must return to source editing.
    key("Escape")
    ui.control("source-editor", focused=True)
    key("End", ("Control_L",))
    text("\nEXPORTED_UNSAVED\n")
    ui.source(original + "\nUNSAVED_SNAPSHOT\n\nEXPORTED_UNSAVED\n")
    ui.command("Marp: Export HTML")
    ui.fill("marp-export-path", "deck.html")
    ui.click("marp-export-submit")
    ui.absent("marp-export-path")
    output = project / "deck.html"
    ui.wait(output.exists, "HTML export did not finish", timeout=35)
    assert "EXPORTED_UNSAVED" in output.read_text()
    assert "EXPORTED_UNSAVED" not in source.read_text(), "Export silently saved Markdown"
    # Replacing a previous export is explicit in the export dialog.
    key("End", ("Control_L",))
    text("\nREPLACED_EXPORT\n")
    ui.source(original + "\nUNSAVED_SNAPSHOT\n\nEXPORTED_UNSAVED\n\nREPLACED_EXPORT\n")
    ui.command("Marp: Export HTML")
    ui.fill("marp-export-path", "deck.html")
    ui.click("marp-export-overwrite")
    ui.click("marp-export-submit")
    ui.absent("marp-export-path")
    ui.wait(
        lambda: "REPLACED_EXPORT" in output.read_text(),
        "Explicit export replacement failed",
    )
    screenshot(ui.window, label + "-exported.png")
    key("s", ("Control_L",))
    ui.wait(
        lambda: "REPLACED_EXPORT" in source.read_text(),
        "Source focus was lost after export",
    )
    ui.command("Marp: New presentation")
    ui.control("source-editor", focused=True)
    key("w", ("Control_L",))
    ui.control("guard-keep-editing")
    screenshot(ui.window, label + "-new-dirty-guard.png")
    ui.click("guard-keep-editing")
    key("s", ("Control_L",))
    ui.fill_path("new.marp.md")
    key("Return")
    ui.wait(
        lambda: (project / "new.marp.md").exists(),
        "New Marp presentation Save As failed",
    )
    assert "marp: true" in (project / "new.marp.md").read_text()
    ui.open_file("deck.marp.md")
    ui.command("Marp: Toggle slide preview")
    focus_preview()
    key("Home")
    ui.wait(lambda: slide(1), "Returning to Slides failed")
    ui.resize(850, 560)
    screenshot(ui.window, label + "-compact.png")


def run(remote=False):
    label = "ssh" if remote else "local"
    with fixture_directory(prefix="rode-marp-ui-") as base:
        project = base / "project"
        project.mkdir()
        source = project / "deck.marp.md"
        original = "---\nmarp: true\ntheme: default\npaginate: true\nbackgroundColor: '#1c2331'\ncolor: '#ffffff'\n---\n# Marp in RODE\n\n---\n\n<!-- _backgroundColor: '#163b2c' -->\n# Second slide\n"
        source.write_text(original)
        fake = project / "marp.py"
        shutil.copy2(ROOT / "tests/fixtures/marp.py", fake)
        fake.chmod(0o755)
        shutil.copy2(ROOT / "tests/fixtures/pages.pdf", project / "pages.pdf")
        real = bool(os.environ.get("RODE_TEST_MARP")) and not remote
        program = os.environ["RODE_TEST_MARP"] if real else str(fake)
        browser = os.environ.get("RODE_TEST_MARP_BROWSER", "") if real else ""
        settings = {
            "program": program,
            "browser": browser,
            "browser_kind": "auto",
            "allow_local_files": False,
            "theme_css": "",
        }
        state_path = base / "ui-state.json"
        environment = fixture_environment(
            base,
            XDG_CONFIG_HOME=str(base / "config"),
            XDG_DATA_HOME=str(base / "data"),
            RODE_UI_TEST_STATE=str(state_path),
        )
        host = ["rode-test", "", "", os.environ["RODE_TEST_SSH_CONFIG"], ""] if remote else None
        context_key = json.dumps([host, str(project)], separators=(",", ":"))
        policy = {"enabled": remote, "share_network": False}
        preferences = {
            context_key: {
                "marp": settings,
                "sandbox": policy,
                "format_on_save": False,
                "check_on_save": False,
                "live_checks": False,
            }
        }
        config = base / "data/rode"
        config.mkdir(parents=True)
        (config / "app.ron").write_text(
            json.dumps({"project_preferences": json.dumps(preferences)})
        )
        args = project_args(project, remote=remote, sandbox=remote)
        with native_session(args, environment, project, label) as ui:
            preview_and_export(ui, project, source, original, label, real)

        # A second owned process checks shutdown without inheriting preview state.
        settings["program"] = str(fake)
        settings["browser"] = ""
        if remote:
            policy["enabled"] = False
        preferences[context_key]["marp"] = settings
        (config / "app.ron").write_text(
            json.dumps({"project_preferences": json.dumps(preferences)})
        )
        (project / "cancel.marp.md").write_text("---\nmarp: true\n---\nRODE_TEST_SLEEP\n")
        with native_session(
            project_args(project, remote=remote), environment, project, label + "-shutdown"
        ) as ui:
            ui.open_file("cancel.marp.md")
            ui.command("Marp: Toggle slide preview")
            ui.wait(lambda: (project / "marp-pids.json").exists(), "Shutdown fixture did not start")
            close_window(ui.window, ui.process)
            # Process exit is expected here, so observe the external cleanup marker.

            wait_for_effect(
                lambda: (project / "marp-stopped").exists(),
                "Closing RODE orphaned the renderer's browser",
            )
    print(label + ": Marp preview/export, protected source and renderer shutdown passed")


if __name__ == "__main__":
    if not os.environ.get("RODE_MARP_UI_REMOTE_ONLY"):
        run()
    if os.environ.get("RODE_TEST_SSH_CONFIG"):
        run(True)
