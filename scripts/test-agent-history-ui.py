#!/usr/bin/env python3
"""Delete saved chats through native controls using only isolated backend fixtures."""

import ctypes
import json
import os
import re
import shutil
import subprocess
import time
from pathlib import Path

os.environ["RODE_UI_ARTIFACTS"] = str(
    Path(__file__).resolve().parent.parent / "target/agent-history-ui"
)
from ui_harness import (
    ARTIFACTS,
    BINARY,
    ROOT,
    assert_running,
    click_button,
    close_window,
    decode_ron_string,
    display,
    fixture_directory,
    focus_window,
    key,
    read_json_lines,
    screenshot,
    text,
    wait_for_button,
    wait_for_project,
    wait_for_ui_state,
    wait_for_window,
    x11,
)

x11.XResizeWindow.argtypes = [ctypes.c_void_p, ctypes.c_ulong, ctypes.c_uint, ctypes.c_uint]


def resize(window, width, height):
    before = rendered_pass()
    x11.XResizeWindow(display, window, width, height)
    x11.XFlush(display)
    wait_layout(
        lambda state: (
            state["pass"] > before
            and any(button["name"] == "agent-history-filter" for button in state["buttons"])
            and all(
                abs(logical * state["pixels_per_point"] - physical) <= 2
                for logical, physical in zip(state["size"], (width, height))
            )
        ),
        f"The {width}x{height} history layout did not finish rendering",
    )


def wait(check, message):
    deadline = time.monotonic() + 15
    while time.monotonic() < deadline:
        assert_running(process, log_path)
        if result := check():
            return result
        time.sleep(0.05)
    raise AssertionError(message)


def control(name, **guards):
    return wait_for_button(window, process, log_path, state_path, name, timeout=15, **guards)


def click_control(name):
    """Activate once, after the native hit test acknowledges the named control."""
    click_button(window, process, log_path, state_path, name, timeout=15)


def rendered_pass():
    return json.loads(state_path.read_text())["pass"]


def wait_layout(check, message):
    return wait_for_ui_state(process, log_path, state_path, check, message, timeout=15)


def absent(name):
    wait_layout(
        lambda state: all(button["name"] != name for button in state["buttons"]),
        name + " did not close",
    )


def replace_control(name, value):
    click_control(name)
    control(name, focused=True)
    before = rendered_pass()
    key("a", ("Control_L",))
    if value:
        text(value)
    else:
        key("BackSpace")
    control(name, focused=True, changed_after=before)


def preferences():
    path = base / "data/rode/app.ron"
    if not path.exists():
        return {}
    match = re.search(r'"agent_preferences"\s*:\s*("(?:\\.|[^"\\])*")', path.read_text())
    return next(iter(json.loads(decode_ron_string(match[1])).values()), {}) if match else {}


def refresh_history(button="agent-history-refresh"):
    before = sum(c.get("method") == "thread/list" for c in calls())
    click_control(button)
    wait(
        lambda: sum(c.get("method") == "thread/list" for c in calls()) > before,
        "History did not request the conversation list",
    )
    control("agent-history-ready")


with fixture_directory(prefix="rode-agent-history-ui-") as base:
    root = base / "project"
    root.mkdir()
    shutil.copy2(ROOT / "tests/fixtures/agent.py", root / "agent.py")
    (root / "agent.py").chmod(0o755)
    history = dict(
        archived=True,
        delete_error="saved-0",
        delete_error_message="thread saved-0 already has an active writer",
        startup_stderr="Codex could not find bubblewrap on PATH. Codex will use the bundled bubblewrap in the meantime.",
        count=4,
        active=["saved-3"],
        titles={
            "saved-0": "Can we discuss the implemented Slurm support in rode?",
            "saved-1": "I get this message on another machine, although I have clicked on install codex button\n\nI couldn’t access the repository: the command runner fails because this file is missing.",
            "saved-2": "Notebook kernels and debugger reattachment",
            "saved-3": "Training run on the GPU cluster",
        },
        previews={
            "saved-0": "Persistent notebook kernels, compute targets and debugger reattachment.",
            "saved-1": "I get this message on another machine, although I have clicked on install codex button\n\nI couldn’t access the repository: the command runner fails because this file is missing:\n`/home/test/.local/share/rode/codex/0.166.0/x86_64/codex-code-node-host`\nNo files were changed.",
            "saved-2": "Variables and breakpoints survive closing the desktop and reconnecting over SSH.",
            "saved-3": "Checking the running allocation and reviewing the latest logs.",
        },
        updated={
            "saved-0": int(time.time()) - 420,
            "saved-1": int(time.time()) - 39600,
            "saved-2": int(time.time()) - 86400,
            "saved-3": int(time.time()) - 120,
        },
    )

    def save_history():
        """Replace control data atomically while the backend may be reading it."""
        pending = root / "history-fixture.tmp"
        pending.write_text(json.dumps(history))
        pending.replace(root / "history-fixture.json")

    save_history()
    state_path = base / "ui-state.json"
    env = dict(
        os.environ,
        XDG_CONFIG_HOME=str(base / "config"),
        XDG_DATA_HOME=str(base / "data"),
        RODE_SESSION_DIR=str(base / "runtime"),
        RODE_UI_TEST_STATE=str(state_path),
    )

    def calls():
        return read_json_lines(root / "agent-rpc.jsonl")

    def deleted():
        path = root / "history-deleted.json"
        return json.loads(path.read_text()) if path.exists() else []

    def running_agents():
        output = subprocess.check_output(
            [str(BINARY), "--session-service", "request"],
            input=b'{"action":"agents"}',
            env=env,
        )
        response = json.loads(output)
        assert response["kind"] == "agents", response
        return [agent for agent in response["data"] if not agent.get("history_browser", False)]

    log_path = ARTIFACTS / "history.log"
    window = None
    with log_path.open("w") as log:
        process = subprocess.Popen(
            [str(BINARY), "--theme", "carbon", "--sandbox", str(root)],
            env=env,
            stdout=log,
            stderr=log,
        )
        try:
            window = wait_for_window(process, log_path)
            wait_for_project(window, process, log_path, root)
            focus_window(window)
            state = wait_layout(lambda _: True, "No native layout")
            if any(button["name"] == "notice-dismiss" for button in state["buttons"]):
                click_control("notice-dismiss")
                absent("notice-dismiss")
            key("a", ("Control_L", "Shift_L"))
            click_control("agent-settings")
            replace_control("agent-codex-executable", "./agent.py")
            wait(
                lambda: preferences().get("codex") == "./agent.py",
                "The complete fixture executable was not saved",
            )
            click_control("agent-settings")
            absent("agent-codex-executable")
            assert not calls(), "Settings started a backend before opening History"
            refresh_history("agent-history")
            assert not running_agents(), "History browsing appeared as a running chat"
            screenshot(window, "list.png")
            # Long multiline backend titles must stay compact, including in a
            # narrow tool panel where the toolbar moves onto its own line.
            resize(window, 760, 900)
            screenshot(window, "compact.png")
            resize(window, 1380, 900)
            click_control("agent-history-delete:saved-0")
            control("agent-history-confirm-delete")
            screenshot(window, "confirm-one.png")
            assert not deleted(), "Opening deletion confirmation removed a chat"
            click_control("agent-history-cancel-delete")
            absent("agent-history-confirm-delete")
            assert not deleted(), "Cancelling deletion removed a chat"
            click_control("agent-history-delete:saved-0")
            click_control("agent-history-confirm-delete")
            wait(
                lambda: any(c.get("method") == "thread/delete" for c in calls()),
                "Writer conflict was not requested",
            )
            control("agent-history-ready")
            control("agent-history-error-details")
            screenshot(window, "writer-conflict.png")
            assert not deleted(), "Writer conflict removed an unacknowledged chat"
            click_control("agent-history-error-details")
            control("agent-history-error-expanded")
            screenshot(window, "writer-details.png")
            click_control("agent-history-error-details")
            control("agent-history-error-collapsed")
            click_control("agent-history-backend-output")
            control("agent-history-output-expanded")
            screenshot(window, "writer-startup-warnings.png")
            click_control("agent-history-backend-output")
            control("agent-history-output-collapsed")
            # Ending another process is left to the user; clearing the fixture
            # lock simulates that action before explicitly refreshing and retrying.
            del history["delete_error"]
            save_history()
            refresh_history()
            click_control("agent-history-delete:saved-0")
            click_control("agent-history-confirm-delete")
            wait(lambda: deleted() == ["saved-0"], "Individual chat deletion was not acknowledged")
            control("agent-history-ready")
            absent("agent-history-delete:saved-0")
            screenshot(window, "deleted-one.png")
            # A filter does not turn Delete all into Delete visible rows. Grow
            # the backend snapshot past one page after the initial four-row view.
            history["count"] = 40
            save_history()
            replace_control("agent-history-filter", "no matching chats")
            wait_layout(
                lambda state: (
                    not any(
                        button["name"].startswith("agent-history-delete:")
                        for button in state["buttons"]
                    )
                ),
                "The history filter did not hide nonmatching conversations",
            )
            screenshot(window, "filtered-empty.png")
            click_control("agent-history-delete-all")
            control("agent-history-confirm-delete")
            screenshot(window, "confirm-all.png")
            click_control("agent-history-confirm-delete")
            expected = sorted(["archived-1"] + [f"saved-{n}" for n in range(40) if n != 3])
            wait(
                lambda: deleted() == expected,
                "Delete all missed unloaded, archived or protected chats",
            )
            control("agent-history-ready")
            screenshot(window, "deleted-all.png")
            assert all(
                c.get("method") not in ("thread/start", "thread/resume", "turn/start")
                for c in calls()
            ), "History deletion started inference or resumed a chat"
            # Refresh the protected chat as idle and resume it with the primary
            # card target; the adjacent Delete button must never resume a chat.
            history["active"] = []
            history["resume_error"] = "saved-3"
            save_history()
            replace_control("agent-history-filter", "")
            refresh_history()
            screenshot(window, "saved-chat-row.png")
            click_control("agent-history-restore:saved-3")
            wait(
                lambda: any(
                    c.get("method") == "thread/resume" and c["params"]["threadId"] == "saved-3"
                    for c in calls()
                ),
                "Conversation card did not resume the selected chat",
            )
            control("agent-history-ready")
            control("agent-history-error-details")
            screenshot(window, "restore-writer-conflict.png")
            click_control("agent-history-running")
            control("agent-running-refresh")
            screenshot(window, "restore-running-agents.png")
            assert not running_agents(), "A failed history browser appeared as a running chat"
            key("Escape")
            absent("agent-running-refresh")
            del history["resume_error"]
            save_history()
            refresh_history()
            click_control("agent-history-restore:saved-3")
            wait(
                lambda: any(c.get("method") == "thread/turns/list" for c in calls()),
                "Conversation did not restore after the writer lock was released",
            )
            absent("agent-history-filter")
            control("agent-composer")
            screenshot(window, "resumed.png")
            assert any(agent["alive"] for agent in running_agents()), (
                "The restored chat was missing from Running"
            )
            assert all(c.get("method") not in ("thread/start", "turn/start") for c in calls()), (
                "Restoring history started inference"
            )
            # Selecting the current chat returns to the existing connection;
            # a second resume would contend with its own active writer.
            resumes_before = sum(c.get("method") == "thread/resume" for c in calls())
            refresh_history("agent-history")
            screenshot(window, "current-chat-row.png")
            click_control("Show current conversation")
            absent("agent-history-filter")
            control("agent-composer")
            assert sum(c.get("method") == "thread/resume" for c in calls()) == resumes_before, (
                "Selecting the open chat tried to acquire a second writer"
            )
            screenshot(window, "returned-to-open-chat.png")
            history["refresh_error"] = True
            save_history()
            refresh_history("agent-history")
            screenshot(window, "backend-error.png")
            close_window(window, process)
        except Exception:
            for source, name in (
                (state_path, "failure-state.json"),
                (root / "agent-rpc.jsonl", "failure-rpc.jsonl"),
                (root / "history-deleted.json", "failure-deleted.json"),
                (root / "history-fixture.json", "failure-fixture.json"),
            ):
                if source.exists():
                    shutil.copy2(source, ARTIFACTS / name)
            if window and process.poll() is None:
                screenshot(window, "failure.png")
            raise
        finally:
            if process.poll() is None:
                process.terminate()
                process.wait(timeout=5)
print(
    "History layout: long titles, compact panels, filter, protected bulk deletion and card resume passed"
)
