#!/usr/bin/env python3
"""Native allocation form, pending/active indicators, environment and reattachment."""

import json
import os
import shutil
import subprocess
from functools import partial
from pathlib import Path

os.environ["RODE_UI_ARTIFACTS"] = str(Path(__file__).resolve().parent.parent / "target/slurm-ui")
from ui_fixture import project_ui
from ui_harness import (
    ARTIFACTS,
    BINARY,
    ROOT,
    close_window,
    fixture_directory,
    fixture_environment,
    key,
    read_preferences,
    screenshot,
    text,
    theme,
    wait_for_effect,
)

wait_for = partial(wait_for_effect, timeout=18)


def filled(name, expected):
    ui.button_fill("terminal-allocation", "terminal-new-menu", name, filled=expected)


with fixture_directory(prefix="rode-slurm-ui-") as base:
    root = base / "project"
    tools = root / "tools"
    tools.mkdir(parents=True)
    for name in ["salloc", "srun", "squeue", "scancel"]:
        shutil.copy2(ROOT / "tests/fixtures/slurm.py", tools / name)
        (tools / name).chmod(0o755)
    shell = tools / "shell"
    shell.write_text("#!/bin/sh\nexec /bin/bash --noprofile --norc\n")
    shell.chmod(0o755)
    environment = fixture_environment(
        base,
        PATH=str(tools) + os.pathsep + os.environ["PATH"],
        SHELL=str(shell),
        SALLOC_NO_SHELL="1",
        SRUN_EXPORT_ENV="NONE",
        SLURM_STEP_GPUS="stale",
        XDG_CONFIG_HOME=str(base / "config"),
        XDG_DATA_HOME=str(base / "data"),
        RODE_SESSION_DIR=str(base / "runtime"),
    )

    def rpc(action, **values):
        result = json.loads(
            subprocess.check_output(
                [str(BINARY), "--session-service", "request"],
                input=json.dumps(dict(action=action, **values)).encode(),
                env=environment,
            )
        )
        assert result["kind"] != "error", result
        return result.get("data")

    process = None
    window = None
    with (ARTIFACTS / "native.log").open("w") as log:

        def launch():
            global process, window, ui, command
            process = subprocess.Popen(
                [str(BINARY), "--theme", "carbon", str(root)],
                env=environment,
                stdout=log,
                stderr=log,
            )
            ui = project_ui(process, environment, root, log.name)
            window = ui.window
            command = ui.command
            ui.resize(1380, 900)

        try:
            launch()
            command("Terminal: New terminal")
            wait_for(lambda: rpc("list"), "No initial terminal")
            original = rpc("list")[0]
            ui.navigate("t")
            ui.terminal_input()
            text("TOKEN=untouched\n")
            ui.click("terminal-allocation")
            ui.control("slurm-mode-sandbox", enabled=False)
            ui.value("slurm-execution-mode", "outside_sandbox")
            ui.resize(850, 560)
            for name in ("slurm-allocate", "slurm-reserve", "slurm-close"):
                ui.control(name)
            screenshot(window, "compact-form.png")
            ui.resize(1380, 900)
            screenshot(window, "form.png")
            # Request two GPUs through the real number field and form action.
            # Focusing DragValue already opens its numeric text editor.
            ui.hint("GPUs (total)")
            key("a", ("Control_L",))
            text("2")
            key("Return")
            ui.hint("Allocate in new terminal")
            key("Return")
            wait_for(
                lambda: any(info.get("allocation") for info in rpc("list")),
                "Allocation form did not create a terminal",
            )
            info = next(info for info in rpc("list") if info.get("allocation"))
            assert not info["persistent"]
            assert info["mode"] == "NORMAL MODE", info
            assert any(i["id"] == original["id"] and i["alive"] for i in rpc("list"))
            state = root / "slurm-state"
            wait_for(lambda: (state / "job.json").exists(), "No pending allocation")
            assert json.loads((state / "job.json").read_text())["state"] == "PENDING"
            args = json.loads((state / "job.json").read_text())["args"]
            assert "--gpus=2" in args and "--time=2:00:00" in args and "--mem=80G" in args, args
            filled("pending.png", False)
            (state / "grant").touch()
            wait_for(
                lambda: json.loads((state / "job.json").read_text())["state"] == "RUNNING",
                "Allocation did not start",
            )
            filled("active.png", True)
            theme("phosphor", ui=ui)
            filled("active-phosphor.png", True)
            theme("carbon", ui=ui)
            (state / "queue-error").touch()
            filled("query-error.png", False)
            (state / "queue-error").unlink()
            filled("query-recovered.png", True)
            ui.navigate("t")
            ui.terminal_input()
            text(
                'printf \'%s:%s:%s:%s\' "$SLURM_JOB_ID" "$SLURM_STEP_ID" "$SLURM_STEP_GPUS" "$CUDA_VISIBLE_DEVICES" > allocation.env\n'
            )
            wait_for(lambda: (root / "allocation.env").exists(), "No interactive allocation shell")
            assert (root / "allocation.env").read_text() == "4242:0:0,1:0,1"
            ui.click("terminal-persist:off")
            wait_for(
                lambda: any(i["id"] == info["id"] and i["persistent"] for i in rpc("list")),
                "Could not persist the allocation",
            )
            ui.resize(850, 560)
            filled("compact-active.png", True)
            close_window(window, process)
            assert any(i["id"] == info["id"] for i in rpc("list"))
            preferences = read_preferences(base / "data/rode/app.ron")
            projects = json.loads(preferences["project_preferences"])
            saved = projects[json.dumps([None, str(root)], separators=(",", ":"))]
            # Shared storage omits unchanged defaults when merging preferences.
            assert saved.get("slurm_sandbox", False) is False, saved
            assert saved.get("terminal_sandbox", True) is True, saved
            launch()
            command("Terminal: Sessions and Slurm allocations")
            screenshot(window, "reattach-list.png")
            # The original nonpersistent shell ended on exit, leaving this one row.
            ui.click("session-attach:" + info["id"])
            ui.terminal_input()
            screenshot(window, "reattached.png")
            ui.navigate("t")
            ui.terminal_input()
            text("printf '%s' \"$SLURM_JOB_ID\" > reattached.env\n")
            wait_for(
                lambda: (root / "reattached.env").exists(), "Could not reattach the allocation"
            )
            assert (root / "reattached.env").read_text() == "4242"
            ui.click("terminal-allocation")
            screenshot(window, "release.png")
            assert not (state / "cancelled").exists()
            ui.hint("Release allocation")
            key("Return")
            ui.control("session-confirm")
            screenshot(window, "release-confirmation.png")
            assert not (state / "cancelled").exists(), "Clicking Release skipped confirmation"
            key("Return", ("Alt_L",))
            wait_for(
                lambda: (state / "cancelled").exists(), "Confirmed allocation release did not run"
            )
            key("Escape")
            filled("released.png", False)
            close_window(window, process)
            print(
                "Slurm form, pending/active fill, scheduler environment, compact header, persistent reattachment and confirmed release passed."
            )
        except Exception:
            if process and process.poll() is None and window:
                screenshot(window, "failure.png")
            raise
        finally:
            if process and process.poll() is None:
                process.terminate()
                process.wait(timeout=5)
            for info in rpc("list") or []:
                rpc("end", id=info["id"], policy=info["policy"])
