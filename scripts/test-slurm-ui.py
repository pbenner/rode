#!/usr/bin/env python3
"""Native allocation form, pending/active indicators, environment and reattachment."""

import ctypes
import json
import os
import shutil
import subprocess
from functools import partial
from pathlib import Path

os.environ["RODE_UI_ARTIFACTS"] = str(Path(__file__).resolve().parent.parent / "target/slurm-ui")
from ui_harness import (
    ARTIFACTS,
    BINARY,
    ROOT,
    click,
    close_window,
    display,
    fixture_directory,
    fixture_environment,
    key,
    screenshot,
    text,
    theme,
    wait_for_effect,
    x11,
    xtest,
)

x11.XResizeWindow.argtypes = [ctypes.c_void_p, ctypes.c_ulong, ctypes.c_uint, ctypes.c_uint]

from ui_fixture import project_ui

wait_for = partial(wait_for_effect, timeout=18)


def filled(window, name, expected, width=1380, height=900, accent=(220, 163, 37)):
    def matches():
        xtest.XTestFakeMotionEvent(display, -1, 600, 200, 0)
        x11.XFlush(display)
        screenshot(window, name)
        raw = subprocess.check_output(["convert", str(ARTIFACTS / name), "-depth", "8", "rgb:-"])
        # Allocation is immediately left of the rightmost Persist toggle.
        count = sum(
            raw[(y * width + x) * 3 : (y * width + x) * 3 + 3] == bytes(accent)
            for y in (range(98, 122) if width >= 1000 else range(124, 148))
            for x in range(width - 215, width - 108)
        )
        return (count > 500) if expected else (count < 100)

    wait_for(matches, f"Allocation indicator did not reach expected state: {name}")


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
            click(600, 755)
            text("TOKEN=untouched\n")
            click(1210, 108)
            screenshot(window, "form.png")
            # Request two GPUs through the real number field and form action.
            click(592, 394)
            click(592, 394)
            key("a", ("Control_L",))
            text("2")
            key("Return")
            click(500, 662)  # Allocate in new terminal below the Resources card.
            wait_for(
                lambda: any(info.get("allocation") for info in rpc("list")),
                "Allocation form did not create a terminal",
            )
            info = next(info for info in rpc("list") if info.get("allocation"))
            assert not info["persistent"]
            assert any(i["id"] == original["id"] and i["alive"] for i in rpc("list"))
            state = root / "slurm-state"
            wait_for(lambda: (state / "job.json").exists(), "No pending allocation")
            assert json.loads((state / "job.json").read_text())["state"] == "PENDING"
            args = json.loads((state / "job.json").read_text())["args"]
            assert "--gpus=2" in args and "--time=2:00:00" in args and "--mem=80G" in args
            filled(window, "pending.png", False)
            (state / "grant").touch()
            wait_for(
                lambda: json.loads((state / "job.json").read_text())["state"] == "RUNNING",
                "Allocation did not start",
            )
            filled(window, "active.png", True)
            theme("phosphor", ui=ui)
            filled(window, "active-phosphor.png", True, accent=(125, 235, 164))
            theme("carbon", ui=ui)
            (state / "queue-error").touch()
            filled(window, "query-error.png", False)
            (state / "queue-error").unlink()
            filled(window, "query-recovered.png", True)
            click(600, 755)
            text(
                'printf \'%s:%s:%s:%s\' "$SLURM_JOB_ID" "$SLURM_STEP_ID" "$SLURM_STEP_GPUS" "$CUDA_VISIBLE_DEVICES" > allocation.env\n'
            )
            wait_for(lambda: (root / "allocation.env").exists(), "No interactive allocation shell")
            assert (root / "allocation.env").read_text() == "4242:0:0,1:0,1"
            click(1326, 108)
            wait_for(
                lambda: any(i["id"] == info["id"] and i["persistent"] for i in rpc("list")),
                "Could not persist the allocation",
            )
            x11.XResizeWindow(display, window, 850, 560)
            x11.XFlush(display)
            filled(window, "compact-active.png", True, 850, 560)
            close_window(window, process)
            assert any(i["id"] == info["id"] for i in rpc("list"))
            launch()
            command("Terminal: Sessions and Slurm allocations")
            screenshot(window, "reattach-list.png")
            # The original nonpersistent shell ended on exit, leaving this one row.
            click(647, 294)  # Attach the retained Slurm session.
            ui.terminal_input()
            screenshot(window, "reattached.png")
            click(600, 755)
            text("printf '%s' \"$SLURM_JOB_ID\" > reattached.env\n")
            wait_for(
                lambda: (root / "reattached.env").exists(), "Could not reattach the allocation"
            )
            assert (root / "reattached.env").read_text() == "4242"
            click(1210, 108)
            screenshot(window, "release.png")
            assert not (state / "cancelled").exists()
            click(682, 296)  # Release allocation in the current-job toolbar.
            screenshot(window, "release-confirmation.png")
            assert not (state / "cancelled").exists(), "Clicking Release skipped confirmation"
            key("Return", ("Alt_L",))
            wait_for(
                lambda: (state / "cancelled").exists(), "Confirmed allocation release did not run"
            )
            key("Escape")
            filled(window, "released.png", False)
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
