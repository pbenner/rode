#!/usr/bin/env python3
"""Run the independent Stocks provider through the same API as installed panels.

Uses only explicit synthetic market data; no public endpoints are contacted.
Checks keyboard chart interaction, persisted settings and independent windows.
"""

import json
import os
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
os.environ["RODE_UI_ARTIFACTS"] = str(ROOT / "target/extension-charts-ui")
from ui_fixture import native_session
from ui_harness import BINARY, fixture_directory, fixture_environment, key, screenshot

with fixture_directory(prefix="rode-charts-") as base:
    env = fixture_environment(base)
    project = base / "project"
    project.mkdir()
    manifests = base / "config/rode/extensions"
    manifests.mkdir(parents=True)
    provider = BINARY.with_name("rode-stocks")
    assert provider.is_file(), "Build included extensions with cargo build --profile ci --bins"
    for name, program, args, version in [
        ("market", str(provider), ["--stdio", "--demo"], 2),
        ("notes", "python3", [str(ROOT / "tests/fixtures/panel.py"), str(base / "notes.jsonl")], 1),
    ]:
        (manifests / f"test.{name}.json").write_text(
            json.dumps(
                dict(
                    api=1,
                    id=f"test.{name}",
                    name=f"Test {name}",
                    version="1",
                    description="Synthetic native provider",
                    panel=True,
                    panel_version=version,
                    tool=dict(program=program, args=args),
                )
            )
        )

    def marker(ui, chart):
        return next(
            button["name"]
            for button in ui.layout()["buttons"]
            if button["name"].startswith(f"extension-chart-window:{chart}:")
        )

    with native_session(
        [str(BINARY), "--theme", "studio", str(project)], env, project, "charts"
    ) as ui:
        key("x", ("Control_L", "Shift_L"))
        ui.fill("extension-filter", "test.")
        ui.click("extension-toggle:test.market")
        ui.click("extension-toggle:test.notes")
        key("Escape")
        ui.click("extension-panel:test.market")
        chart = "AAPL:1Y"
        ui.control(f"extension-chart:{chart}")
        original = marker(ui, chart)
        ui.click(f"extension-chart:{chart}")
        ui.control(f"extension-chart:{chart}", focused=True)
        key("equal")
        ui.layout(
            lambda state: all(row["name"] != original for row in state["buttons"]),
            "Keyboard zoom did not change the viewport",
        )
        zoomed = marker(ui, chart)
        key("Right")
        ui.layout(
            lambda state: all(row["name"] != zoomed for row in state["buttons"]),
            "Keyboard pan did not change the viewport",
        )
        ui.control(f"extension-chart:{chart}", focused=True)
        key("Home")
        ui.control(original)
        ui.click("extension-field:query")
        ui.fill("extension-field:query", "Siemens")
        key("Return")
        ui.click("extension-action:choose:SIE.DE")
        chart = "SIE.DE:1Y"
        ui.control(f"extension-chart:{chart}")
        ui.click("extension-action:pin")
        ui.control("extension-action:watch:SIE.DE")
        ui.click("extension-action:style:candles")
        ui.value(f"extension-chart-style:{chart}", "true:true")
        ui.click("extension-action:volume")
        ui.value(f"extension-chart-style:{chart}", "true:false")
        screenshot(ui.window, "native-market-chart.png")
        ui.click("extension-panel:test.market", mouse_button=3)
        ui.click("area-detach")
        market = ui.child_window("RODE · Test market")
        market.control(f"extension-chart:{chart}")
        ui.click("extension-panel:test.notes")
        ui.click("extension-panel:test.notes", mouse_button=3)
        ui.click("area-detach")
        notes = ui.child_window("RODE · Test notes")
        notes.control("extension-action:refresh")
        market.control(f"extension-chart:{chart}")
        notes.click("workspace-reattach:Test notes")
        market.click("workspace-reattach:Test market")
        ui.control(f"extension-chart:{chart}")
        ui.resize(900, 650)
        ui.control("extension-field:query")
        ui.control(f"extension-chart:{chart}")
        bounds = next(
            row["rect"]
            for row in ui.layout()["buttons"]
            if row["name"] == f"extension-chart:{chart}"
        )
        assert bounds[3] - bounds[1] >= 160, f"Compact layout hides the chart: {bounds}"
        screenshot(ui.window, "compact-market-chart.png")
    # A restarted provider gets the host's persisted extension settings.
    with native_session(
        [str(BINARY), "--theme", "studio", str(project)], env, project, "restored"
    ) as ui:
        ui.click("extension-panel:test.market")
        ui.value("extension-chart-style:SIE.DE:1Y", "true:false")
        ui.control("extension-action:watch:SIE.DE")
    print(
        "Native chart extensions: search, keyboard zoom/pan, styles, watchlist persistence and independent windows passed."
    )
