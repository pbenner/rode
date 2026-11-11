#!/usr/bin/env python3
"""Reserve through native controls, select a Slurm notebook target and change it."""

import json
import os
import shutil
import subprocess
from functools import partial
from pathlib import Path

os.environ["RODE_UI_ARTIFACTS"] = str(
    Path(__file__).resolve().parent.parent / "target/slurm-notebook-ui"
)
from ui_fixture import project_ui
from ui_harness import (
    ARTIFACTS,
    BINARY,
    ROOT,
    click,
    close_window,
    fixture_directory,
    fixture_environment,
    key,
    screenshot,
    wait_for_effect,
)

wait_for = partial(wait_for_effect, timeout=30)


def save_completed(ui, source):
    state = ui.layout(
        lambda state: state.get("notebook") is not None and not state["notebook"]["busy"],
        "Kernel run did not finish",
    )
    expected = state["notebook"]["cells"][0]["execution_count"]
    key("s", ("Control_L",))
    ui.layout(
        lambda state: (state.get("notebook") or {}).get("saved") is True,
        "Notebook save was not acknowledged",
    )
    ui.wait(
        lambda: json.loads(source.read_text())["cells"][0]["execution_count"] == expected,
        "Completed notebook output did not save",
    )


with fixture_directory(prefix="rode-slurm-notebook-ui-") as base:
    root = base / "project"
    tools = root / "tools"
    tools.mkdir(parents=True)
    python = Path(os.environ["RODE_TEST_JUPYTER"]).absolute()
    (root / ".venv").symlink_to(python.parent.parent)
    for name in ["salloc", "srun", "squeue", "scancel"]:
        shutil.copy2(ROOT / "tests/fixtures/slurm.py", tools / name)
        (tools / name).chmod(0o755)
    state = root / "slurm-state"
    state.mkdir()
    (state / "grant").touch()
    book = json.loads((ROOT / "tests/fixtures/notebook.ipynb").read_text())
    book["cells"] = [
        dict(
            cell_type="code",
            metadata={},
            outputs=[],
            execution_count=None,
            source="import json, os\nfrom pathlib import Path\n"
            "counter = globals().get('counter', 0) + 1\n"
            "Path('executed.json').write_text(json.dumps(dict(job=os.environ.get('SLURM_JOB_ID'), "
            "gpu=os.environ.get('CUDA_VISIBLE_DEVICES'), counter=counter)))\nprint(counter)",
        )
    ]
    (root / "demo.ipynb").write_text(json.dumps(book))
    env = fixture_environment(
        base,
        PATH=str(tools) + os.pathsep + os.environ["PATH"],
        XDG_CONFIG_HOME=str(base / "config"),
        XDG_DATA_HOME=str(base / "data"),
        RODE_SESSION_DIR=str(base / "runtime"),
    )

    def rpc(action, **values):
        result = json.loads(
            subprocess.check_output(
                [str(BINARY), "--session-service", "request"],
                input=json.dumps(dict(action=action, **values)).encode(),
                env=env,
            )
        )
        assert result["kind"] != "error", result
        return result.get("data")

    owner = None
    process, window = None, None
    with (ARTIFACTS / "native.log").open("w") as log:
        try:
            process = subprocess.Popen(
                [str(BINARY), str(root), "--theme", "carbon"], env=env, stdout=log, stderr=log
            )

            ui = project_ui(process, env, root, log.name)
            window = ui.window
            command = ui.command
            command("Terminal: Allocate Slurm resources")
            screenshot(window, "reserve-form.png")
            click(660, 662)  # Reserve for notebooks below the Resources card.
            wait_for(lambda: (state / "job.json").exists(), "Reserve button did not submit")
            wait_for(
                lambda: json.loads((state / "job.json").read_text())["state"] == "RUNNING",
                "Reservation was not granted",
            )
            owner = next(info for info in rpc("list") if info.get("allocation"))
            assert owner["persistent"]
            screenshot(window, "reserved.png")
            ui.open_file("demo.ipynb")
            screenshot(window, "notebook.png")
            ui.click("notebook-settings")
            ui.click("notebook-refresh-targets")
            ui.control("notebook-targets-ready")
            ui.click("notebook-target")
            screenshot(window, "targets.png")
            ui.click("notebook-target:4242")
            ui.value("notebook-target-value", "4242")
            command("Notebook: Run all cells")
            marker = root / "executed.json"
            wait_for(marker.exists, "Notebook did not execute on selected target")
            result = json.loads(marker.read_text())
            assert result == dict(job="4242", gpu="0,1", counter=1), result
            screenshot(window, "compute-kernel.png")
            # Toggle persistence on the existing kernel, then destroy the entire desktop.
            ui.click("notebook-persistence")
            wait_for(
                lambda: any(i["persistent"] for i in rpc("runtimes")),
                "Kernel persistence was not acknowledged",
            )
            kernel = next(i for i in rpc("runtimes") if i["persistent"])
            save_completed(ui, root / "demo.ipynb")
            close_window(window, process)
            wait_for(
                lambda: any(i["id"] == kernel["id"] and not i["attached"] for i in rpc("runtimes")),
                "Kernel did not detach",
            )
            # Reattach with a different UI interpreter selection. The retained
            # kernel must keep its captured interpreter and Python variables.
            preferences = base / "data/rode/app.ron"
            content = preferences.read_text().rstrip()
            storage = json.loads(content[:-1].rstrip().removesuffix(",") + "}")
            saved = json.loads(storage.get("project_preferences", "{}"))
            profile = json.dumps([None, str(root)], separators=(",", ":"))
            saved.setdefault(profile, {})["python"] = str(python)
            storage["project_preferences"] = json.dumps(saved)
            preferences.write_text(json.dumps(storage))
            process = subprocess.Popen(
                [str(BINARY), str(root), "--theme", "carbon"], env=env, stdout=log, stderr=log
            )
            window = None
            ui = project_ui(process, env, root, log.name)
            window = ui.window
            command = ui.command
            ui.open_file("demo.ipynb")
            screenshot(window, "before-reattach.png")
            # The expanded settings state survives the desktop restart.
            ui.click("notebook-reattach")
            wait_for(
                lambda: any(i["id"] == kernel["id"] and i["attached"] for i in rpc("runtimes")),
                "Kernel did not reattach",
            )
            ui.layout(
                lambda state: state.get("notebook") is not None and not state["notebook"]["busy"],
                "Reattached kernel did not become ready",
            )
            marker.unlink()
            command("Notebook: Run all cells")
            wait_for(marker.exists, "Reattached kernel did not execute")
            result = json.loads(marker.read_text())
            assert result == dict(job="4242", gpu="0,1", counter=2), result
            screenshot(window, "reattached-kernel.png")
            # Changing to the host stops the old kernel and resets its live variables.
            marker.unlink()
            ui.click("notebook-target")
            ui.click("notebook-target:host")
            ui.value("notebook-target-value", "host")
            command("Notebook: Run all cells")
            wait_for(marker.exists, "Host kernel did not start after changing target")
            result = json.loads(marker.read_text())
            assert result["job"] is None and result["counter"] == 1, result
            host_kernel = next(i for i in rpc("runtimes") if i["alive"] and i["target"] is None)
            assert host_kernel["python"] == str(python), host_kernel
            assert (state / "job.json").exists(), "Changing kernels released the reservation"
            assert any(info["id"] == owner["id"] for info in rpc("list"))
            screenshot(window, "host-kernel.png")
            save_completed(ui, root / "demo.ipynb")
            close_window(window, process)
        except Exception:
            if process and process.poll() is None and window:
                screenshot(window, "failure.png")
            raise
        finally:
            if process and process.poll() is None:
                process.terminate()
                process.wait(timeout=5)
            for info in rpc("runtimes") or []:
                rpc("end_runtime", id=info["id"], policy=info["policy"])
            for info in rpc("list") or []:
                rpc("end", id=info["id"], policy=info["policy"])
            wait_for(
                lambda: not rpc("runtimes") and not rpc("list"),
                "Notebook fixture services did not stop",
            )
            if owner is not None:
                wait_for(
                    (state / "ended").exists, "Allocation fixture did not finish shutting down"
                )
print(
    "Notebook reservation, compute execution, desktop restart, kernel reattachment and target restart passed"
)
