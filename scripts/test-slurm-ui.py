#!/usr/bin/env python3
"""Native allocation form, pending/active indicators, environment and reattachment."""

import ctypes
import json
import os
import re
import shutil
import subprocess
import tempfile
import time
from pathlib import Path

os.environ["RODE_UI_ARTIFACTS"] = str(Path(__file__).resolve().parent.parent / "target/slurm-ui")
from ui_harness import (
    ARTIFACTS,
    BINARY,
    ROOT,
    click,
    close_window,
    display,
    key,
    screenshot,
    text,
    theme,
    x11,
    xtest,
)

x11.XResizeWindow.argtypes = [ctypes.c_void_p, ctypes.c_ulong, ctypes.c_uint, ctypes.c_uint]


def wait_for(check, message):
    end = time.monotonic() + 18
    while time.monotonic() < end:
        if check():
            return
        time.sleep(0.1)
    raise AssertionError(message)


def command(value):
    key("p", ("Control_L", "Shift_L"))
    text(value)
    key("Return")
    time.sleep(0.4)


def filled(window, name, expected, width=1380, height=900, accent=(220, 163, 37)):
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
    assert (count > 500) if expected else (count < 100), (name, count)


with tempfile.TemporaryDirectory(prefix="rode-slurm-ui-") as directory:
    base = Path(directory)
    root = base / "project"
    tools = root / "tools"
    tools.mkdir(parents=True)
    for name in ["salloc", "srun", "squeue", "scancel"]:
        shutil.copy2(ROOT / "tests/fixtures/slurm.py", tools / name)
        (tools / name).chmod(0o755)
    shell = tools / "shell"
    shell.write_text("#!/bin/sh\nexec /bin/bash --noprofile --norc\n")
    shell.chmod(0o755)
    environment = dict(
        os.environ,
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
            global process, window
            process = subprocess.Popen(
                [str(BINARY), "--theme", "carbon", str(root)],
                env=environment,
                stdout=log,
                stderr=log,
            )
            found = []

            def mapped():
                match = re.search(
                    r'(0x[0-9a-f]+) "RODE',
                    subprocess.check_output(["xwininfo", "-root", "-tree"], text=True),
                )
                if match:
                    found.append(int(match[1], 16))
                    return True
                assert process.poll() is None
                return False

            wait_for(mapped, "No native RODE window")
            window = found[-1]
            time.sleep(0.7)
            x11.XSetInputFocus(display, window, 1, 0)
            x11.XResizeWindow(display, window, 1380, 900)
            x11.XFlush(display)
            time.sleep(0.4)

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
            click(592, 400)
            click(592, 400)
            key("a", ("Control_L",))
            text("2")
            key("Return")
            click(498, 593)
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
            time.sleep(5.5)
            filled(window, "active.png", True)
            theme("phosphor")
            filled(window, "active-phosphor.png", True, accent=(125, 235, 164))
            theme("carbon")
            (state / "queue-error").touch()
            time.sleep(6)
            filled(window, "query-error.png", False)
            (state / "queue-error").unlink()
            time.sleep(6)
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
            time.sleep(0.4)
            filled(window, "compact-active.png", True, 850, 560)
            close_window(window, process)
            assert any(i["id"] == info["id"] for i in rpc("list"))
            launch()
            command("Terminal: Sessions and Slurm allocations")
            screenshot(window, "reattach-list.png")
            # The original nonpersistent shell ended on exit, leaving this one row.
            for _ in range(5):
                key("Tab")
            key("Return")
            time.sleep(1)
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
            click(682, 343)
            screenshot(window, "release-confirmation.png")
            assert not (state / "cancelled").exists(), "Clicking Release skipped confirmation"
            key("Return", ("Alt_L",))
            wait_for(
                lambda: (state / "cancelled").exists(), "Confirmed allocation release did not run"
            )
            key("Escape")
            time.sleep(0.5)
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
