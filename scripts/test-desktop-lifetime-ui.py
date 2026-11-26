#!/usr/bin/env python3
"""Kill only a fixture-owned X server; verify agent policy and crash draft recovery."""

import contextlib
import json
import os
import secrets
import select
import shutil
import subprocess
import tempfile
import time
from pathlib import Path

from fixture_support import fixture_directory, fixture_environment


def wait(check, message, timeout=20):
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        if result := check():
            return result
        time.sleep(0.05)
    raise AssertionError(message)


@contextlib.contextmanager
def owned_display(log):
    """Allocate an authenticated private display; optional TCP supports restricted /tmp hosts."""
    temporary = tempfile.TemporaryDirectory(prefix="display-loss-auth-")
    auth = Path(temporary.name) / "auth"
    cookie = secrets.token_hex(16)
    subprocess.run(
        ["xauth", "-f", str(auth), "add", "127.0.0.1:199", "MIT-MAGIC-COOKIE-1", cookie],
        check=True,
        capture_output=True,
        timeout=5,
    )
    auth.chmod(0o600)
    tcp = os.environ.get("RODE_TEST_XVFB_TCP") == "1"
    transport = ["-nolisten", "unix", "-listen", "tcp"] if tcp else ["-nolisten", "tcp"]
    server = subprocess.Popen(
        [
            "Xvfb",
            "-displayfd",
            "1",
            "-screen",
            "0",
            "1600x1100x24",
            *transport,
            "-auth",
            str(auth),
            "-noreset",
        ],
        stdout=subprocess.PIPE,
        stderr=log,
    )
    try:
        assert select.select([server.stdout], [], [], 15)[0], "Private X server did not start"
        number = server.stdout.readline().decode().strip()
        assert number.isdecimal(), "Invalid private X server display"
        environment = dict(os.environ)
        for name in ("WAYLAND_DISPLAY", "WAYLAND_SOCKET", "XAUTHORITY"):
            environment.pop(name, None)
        environment.update(
            DISPLAY=("127.0.0.1:" if tcp else ":") + number,
            XAUTHORITY=str(auth),
            WINIT_UNIX_BACKEND="x11",
            LIBGL_ALWAYS_SOFTWARE="1",
        )
        subprocess.run(
            ["xauth", "-f", str(auth), "add", environment["DISPLAY"], "MIT-MAGIC-COOKIE-1", cookie],
            check=True,
            capture_output=True,
            timeout=5,
        )
        yield (
            server,
            {
                name: environment[name]
                for name in ("DISPLAY", "XAUTHORITY", "WINIT_UNIX_BACKEND", "LIBGL_ALWAYS_SOFTWARE")
            },
        )
    finally:
        if server.poll() is None:
            server.terminate()
        server.wait(timeout=5)
        server.stdout.close()
        temporary.cleanup()


def run():
    os.environ["RODE_UI_ARTIFACTS"] = str(
        Path(__file__).resolve().parent.parent / "target/desktop-lifetime-ui"
    )
    # Importing ui_harness opens Xlib; establish our own display first.
    from fixture_support import fixture_artifacts

    artifacts = fixture_artifacts(
        Path(__file__).resolve().parent.parent,
        os.environ["RODE_UI_ARTIFACTS"],
        os.environ.get("RODE_CI_RESULTS_DIR"),
    )
    artifacts.mkdir(parents=True, exist_ok=True)
    with (
        fixture_directory(prefix="rode-desktop-loss-") as base,
        (artifacts / "display.log").open("w") as log,
    ):
        processes, cases, windows = [], [], []
        try:
            with owned_display(log) as (server, display_environment):
                os.environ.update(display_environment)
                for name in ("WAYLAND_DISPLAY", "WAYLAND_SOCKET"):
                    os.environ.pop(name, None)
                import ui_harness as harness
                from ui_fixture import project_ui

                def rpc(environment, request):
                    output = subprocess.check_output(
                        [str(harness.BINARY), "--session-service", "request"],
                        input=json.dumps(request).encode(),
                        env=environment,
                        timeout=15,
                    )
                    response = json.loads(output)
                    assert response["kind"] != "error", response
                    return response.get("data")

                for label, mode, finish in [
                    ("off", "off", False),
                    ("auto-working", "auto", False),
                    ("on", "on", True),
                    ("auto-idle", "auto", True),
                ]:
                    case = base / label
                    case.mkdir()
                    root = case / "project"
                    root.mkdir()
                    shutil.copy2(
                        harness.ROOT / "tests/fixtures/persistent_agent.py", root / "agent.py"
                    )
                    (root / "agent.py").chmod(0o755)
                    (root / "notes.txt").write_text("Saved original 日本語\n")
                    if finish:
                        (root / "continue").touch()
                    environment = fixture_environment(case)
                    environment.update(display_environment)
                    environment.update(
                        RODE_SESSION_DIR=str(case / "runtime"),
                        RODE_UI_TEST_STATE=str(case / "layout.json"),
                        XDG_DATA_HOME=str(case / "data"),
                        XDG_CONFIG_HOME=str(case / "config"),
                    )
                    log_path = harness.ARTIFACTS / f"{label}.log"
                    with log_path.open("w") as output:
                        process = subprocess.Popen(
                            [str(harness.BINARY), str(root)],
                            env=environment,
                            stdout=output,
                            stderr=output,
                        )
                    processes.append(process)
                    ui = project_ui(process, environment, root, log_path)
                    windows.append(ui.window)
                    harness.key("a", ("Control_L", "Shift_L"))
                    ui.control("agent-composer")
                    ui.click("agent-settings")
                    ui.edit("agent-codex-executable", "./agent.py")
                    ui.click("agent-settings")
                    ui.absent("agent-codex-executable")
                    if mode != "auto":
                        ui.click("agent-persistence")
                        ui.click("agent-persist-" + mode)
                    ui.edit("agent-composer", "work")
                    ui.press("agent-composer", "Return")
                    if finish:
                        ui.control("agent-idle")
                    else:
                        wait(
                            lambda: (root / "work-started").exists(), "Agent did not begin its task"
                        )
                    info = wait(
                        lambda: next(
                            (
                                i
                                for i in rpc(environment, {"action": "agents"})
                                if i["attached"]
                                and i.get("controller")
                                and (
                                    not finish or (not i["busy"] and (root / "work-done").exists())
                                )
                            ),
                            None,
                        ),
                        "Agent attachment was not published",
                    )
                    assert (
                        info["attached"]
                        and info["controller"]["owner"]["display"] == environment["DISPLAY"]
                    )
                    if label == "on":
                        ui.click("workspace-sidebar")
                        ui.open_file("notes.txt")
                        ui.press("source-editor", "End", ("Control_L",))
                        harness.text("Crash recovery keeps unsaved edits.\n")
                        ui.value(
                            "source-editor-value",
                            "Saved original 日本語\nCrash recovery keeps unsaved edits.\n",
                        )
                        wait(
                            lambda: any(
                                "Crash recovery" in p.read_text()
                                for p in (case / "data").rglob("recovery/*.json")
                            ),
                            "Draft checkpoint not durable",
                        )
                    # An unmapped/idle window is still a connected desktop.
                    harness.x11.XUnmapWindow.argtypes = [harness.c.c_void_p, harness.c.c_ulong]
                    harness.x11.XUnmapWindow(harness.display, ui.window)
                    harness.x11.XFlush(harness.display)
                    cases.append((label, root, environment, info))
                until = time.monotonic() + 11  # Cross one heartbeat interval while unmapped.
                while time.monotonic() < until:
                    for _, _, environment, info in cases:
                        current = rpc(environment, {"action": "agents"})
                        assert any(i["id"] == info["id"] and i["attached"] for i in current), (
                            "A healthy idle desktop lost its attachment"
                        )
                    assert all(p.poll() is None for p in processes), (
                        "A healthy unmapped window exited"
                    )
                    time.sleep(0.2)
                harness.x11.XMapWindow.argtypes = [harness.c.c_void_p, harness.c.c_ulong]
                for window, (_, _, environment, _) in zip(windows, cases, strict=True):
                    layout = Path(environment["RODE_UI_TEST_STATE"])
                    layout.with_suffix(".pause").touch()
                    harness.x11.XMapWindow(harness.display, window)
                    harness.x11.XFlush(harness.display)
                    wait(
                        lambda: layout.with_suffix(".paused").exists(),
                        "UI stall was not acknowledged",
                    )
                server.kill()
                server.wait(timeout=5)
                for process in processes:
                    process.wait(timeout=8)
                    assert process.returncode == 70, (
                        "Blocked UI did not exit through the independent display monitor"
                    )
                for label, root, environment, info in cases:
                    layout = Path(environment["RODE_UI_TEST_STATE"])
                    layout.with_suffix(".pause").unlink()
                    layout.with_suffix(".paused").unlink()
                    if label in ("off", "auto-idle"):
                        wait(
                            lambda: not rpc(environment, {"action": "agents"}),
                            label + " survived display loss",
                        )
                    else:
                        current = wait(
                            lambda: next(
                                (
                                    i
                                    for i in rpc(environment, {"action": "agents"})
                                    if i["id"] == info["id"] and not i["attached"]
                                ),
                                None,
                            ),
                            label + " retained its controlling lease",
                        )
                        assert current["alive"], label + " was killed instead of detached"
                        if label == "auto-working":
                            assert not (root / "work-done").exists()
                            (root / "continue").touch()
                            wait(
                                lambda: not rpc(environment, {"action": "agents"}),
                                "Auto did not finish and exit",
                            )
            with owned_display(log) as (_, display_environment):
                os.environ.update(display_environment)
                # The old Xlib connection belongs to the killed fixture server.
                # Reopen it without calling into the dead display.
                harness.display = harness.x11.XOpenDisplay(None)
                assert harness.display
                _, root, environment, info = next(c for c in cases if c[0] == "on")
                environment.update(display_environment)
                with (harness.ARTIFACTS / "recovery.log").open("w") as output:
                    process = subprocess.Popen(
                        [str(harness.BINARY), str(root)],
                        env=environment,
                        stdout=output,
                        stderr=output,
                    )
                processes.append(process)
                ui = project_ui(process, environment, root, harness.ARTIFACTS / "recovery.log")
                ui.click("recovery-restore")
                ui.value(
                    "source-editor-value",
                    "Saved original 日本語\nCrash recovery keeps unsaved edits.\n",
                )
                assert (root / "notes.txt").read_text() == "Saved original 日本語\n"
                ui.command("Agent: Running sessions")
                ui.click("agent-running-attach:" + info["id"])
                ui.control("agent-idle")
                original = next(
                    i for i in rpc(environment, {"action": "agents"}) if i["id"] == info["id"]
                )
                other_environment = dict(
                    environment, RODE_UI_TEST_STATE=str(base / "other-layout.json")
                )
                other_log = harness.ARTIFACTS / "takeover.log"
                with other_log.open("w") as output:
                    other = subprocess.Popen(
                        [str(harness.BINARY), str(root)],
                        env=other_environment,
                        stdout=output,
                        stderr=output,
                    )
                processes.append(other)
                other_ui = project_ui(other, other_environment, root, other_log)
                other_ui.command("Agent: Running sessions")
                other_ui.click("agent-running-attach:" + info["id"])
                other_ui.click("agent-running-confirm-takeover")
                other_ui.control("agent-idle")
                successor = next(
                    i for i in rpc(environment, {"action": "agents"}) if i["id"] == info["id"]
                )
                assert successor["controller"]["id"] != original["controller"]["id"]
                assert successor["alive"] and successor["attached"] and process.poll() is None
                print(
                    "Display loss: Off/Auto/On, healthy unmapped windows, stalled UI threads, draft recovery and confirmed takeover passed."
                )
        finally:
            for process in processes:
                if process.poll() is None:
                    process.kill()
                process.wait(timeout=5)
            for _, _, environment, info in cases:
                with contextlib.suppress(Exception):
                    rpc(
                        environment,
                        {"action": "end_agent", "id": info["id"], "policy": info["policy"]},
                    )


if __name__ == "__main__":
    run()
