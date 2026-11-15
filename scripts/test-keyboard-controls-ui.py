#!/usr/bin/env python3
"""Native hint/search workflows using keyboard input and observable effects only."""

import hashlib
import os
from pathlib import Path

os.environ["RODE_UI_ARTIFACTS"] = str(
    Path(__file__).resolve().parent.parent / "target/keyboard-controls-ui"
)
from ui_fixture import native_session
from ui_harness import BINARY, fixture_directory, fixture_environment, key, text


def hint(ui, label):
    """Select an observed code; geometry is never used to activate a control."""
    ui.navigate("a")
    state = ui.layout(
        lambda state: state.get("keyboard", {}).get("mode") == "hints",
        "Keyboard hints did not appear",
    )
    choices = [entry for entry in state["keyboard"]["hints"] if entry["label"] == label]
    assert len(choices) == 1, (label, choices, state["keyboard"])
    text(choices[0]["code"])
    ui.layout(lambda state: state["keyboard"]["mode"] == "idle", "Focus handoff stalled")


def run():
    with fixture_directory(prefix="rode-keyboard-controls-") as base:
        root = base / "project"
        root.mkdir()
        source = root / "main.txt"
        source.write_text("original\n")
        other = root / "other.txt"
        other.write_text("second\n")
        environment = fixture_environment(base)
        with native_session([str(BINARY), str(source)], environment, root, "controls") as ui:
            hint(ui, "File menu")
            ui.control("menu-file", focused=True)
            # Selecting the hint focuses; only Enter opens the existing menu.
            assert not any(b["name"] == "menu-item:New file" for b in ui.layout()["buttons"])
            key("Return")
            ui.control("menu-item:New file")
            key("Escape")
            # A command palette creates its own modal. Search must rediscover
            # the originating controls after that modal has closed.
            ui.command("Find visible control", keyboard=True)
            ui.layout(
                lambda state: state["keyboard"]["mode"] == "search",
                "Palette control search did not open",
            )
            text("source editor")
            key("Return")
            ui.control("source-editor", focused=True)
            ui.command("Open file by path", keyboard=True)
            ui.control("file-dialog-path", focused=True)
            hint(ui, "Cancel")
            key("Return")
            ui.document(source)
            ui.navigate("p")
            hint(ui, "Code")
            key("Right")
            ui.control("workspace-tab:Chat", focused=True)
            assert not any(b["name"] == "agent-composer" for b in ui.layout()["buttons"])
            hint(ui, "Terminal")
            ui.control("workspace-tab:Terminal", focused=True)
            hint(ui, "Chat")
            ui.control("workspace-tab:Chat", focused=True)
            assert not any(b["name"] == "agent-composer" for b in ui.layout()["buttons"])
            key("Return")
            ui.control("agent-composer", focused=True)
            ui.navigate("e")
            ui.navigate("g")
            ui.layout(
                lambda state: state["keyboard"]["mode"] == "search", "Control search did not open"
            )
            text("source editor")
            key("Return")
            ui.control("source-editor", focused=True)
            ui.navigate("g")
            ui.layout(
                lambda state: state["keyboard"]["mode"] == "search", "Control search did not reopen"
            )
            text("cancel this query")
            key("Escape")
            ui.control("source-editor", focused=True)
            key("End", ("Control_L",))
            text("ABC")
            ui.source("original\nABC")
            key("s", ("Control_L",))
            ui.wait(
                lambda: source.read_text() == "original\nABC",
                "Hint/search input reached the document",
            )
            ui.command("Open file by path", keyboard=True)
            ui.fill_path(other)
            key("Return")
            ui.document(other)
            hint(ui, "Editor tab: main.txt")
            key("F10", ("Shift_L",))
            ui.control("editor-tab-detach")
            hint(ui, "Move tab right")
            key("Return")
            expected = [
                "editor-tab:" + hashlib.sha256(str(path).encode()).hexdigest()
                for path in [other, source]
            ]
            ui.layout(
                lambda state: (
                    [
                        button["name"]
                        for button in state["buttons"]
                        if button["name"].startswith("editor-tab:")
                    ]
                    == expected
                ),
                "Keyboard tab reordering did not change the tab strip",
            )
            print(
                "Native keyboard hints, modal controls, control search, focus-only selection, contextual tab reordering and source input passed."
            )


if __name__ == "__main__":
    run()
