#!/usr/bin/env python3
"""Native debugger controls, source guards, PTY input and SSH sandbox UI."""

import json
import os
import shutil
import subprocess
import time
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
os.environ["RODE_UI_ARTIFACTS"] = str(ROOT / "target/debugger-ui")
from ui_harness import (
    ARTIFACTS,
    BINARY,
    NativeUI,
    close_window,
    fixture_directory,
    focus_window,
    key,
    read_json_lines,
    screenshot,
    stop_process,
    text,
    wait_for_project,
    wait_for_ui_state,
    wait_for_window,
)


def wait_for(check, message):
    deadline = time.monotonic() + 20
    while time.monotonic() < deadline:
        if check():
            return
        time.sleep(0.05)
    raise AssertionError(message)


def run(remote=False, isolated=False):
    label = "ssh" if remote else "sandbox" if isolated else "local"
    with fixture_directory(prefix="rode-debugger-ui-") as base:
        project = base / "project"
        project.mkdir()
        source = project / "main.py"
        original = "answer = 40\nanswer += 2\nprint(answer)\n# debug sample\n"
        source.write_text(original)
        adapter = project / "adapter.py"
        shutil.copy2(ROOT / "tests/fixtures/debug_adapter.py", adapter)
        (base / "private.txt").write_text("private")
        environment = dict(
            os.environ,
            HOME=str(base / "home"),
            XDG_CONFIG_HOME=str(base / "config"),
            XDG_DATA_HOME=str(base / "data"),
            RODE_SESSION_DIR=str(base / "runtime"),
            RODE_UI_TEST_STATE=str(base / "ui-state.json"),
        )
        Path(environment["HOME"]).mkdir()

        def service_rpc(action, **values):
            result = json.loads(
                subprocess.check_output(
                    [str(BINARY), "--session-service", "request"],
                    input=json.dumps(dict(action=action, **values)).encode(),
                    env=environment,
                )
            )
            assert result["kind"] != "error", result
            return result.get("data")

        host = ["rode-test", "", "", os.environ["RODE_TEST_SSH_CONFIG"], ""] if remote else None
        context_key = json.dumps([host, str(project)], separators=(",", ":"))
        prefs = {
            context_key: dict(
                debugger=dict(
                    config=dict(
                        backend="custom",
                        adapter="python3",
                        adapter_arguments=[str(adapter)],
                        program="{file}",
                        extra={"flood": True},
                    ),
                    watches=["answer"],
                ),
                sandbox=dict(enabled=isolated or remote),
                format_on_save=False,
                check_on_save=False,
                live_checks=False,
            )
        }
        config = base / "data/rode"
        config.mkdir(parents=True)
        (config / "app.ron").write_text(json.dumps(dict(project_preferences=json.dumps(prefs))))
        args = [str(BINARY), "--theme", "carbon"]
        if remote:
            # SSH authentication needs a real PTY; avoid shell startup rc noise
            # in this host's hermetic localhost test (same as other UI fixtures).
            tools = base / "tools"
            tools.mkdir()
            wrapper = tools / "ssh"
            wrapper.write_text(
                "#!/usr/bin/python3\nimport os,sys\nbinary="
                + repr(shutil.which("ssh"))
                + "\nos.execv(binary,[binary]+['-T' if a=='-tt' else a for a in sys.argv[1:]])\n"
            )
            wrapper.chmod(0o755)
            environment["PATH"] = str(tools) + os.pathsep + os.environ["PATH"]
            args += [
                "--ssh",
                "rode-test",
                "--ssh-config",
                os.environ["RODE_TEST_SSH_CONFIG"],
                "--remote-path",
                str(project),
                "--sandbox",
            ]
        else:
            args += [str(project)]
            if isolated:
                args += ["--sandbox"]
        log_path = ARTIFACTS / (label + ".log")
        with log_path.open("w") as log:
            process = subprocess.Popen(args, env=environment, stdout=log, stderr=log)
            window = None
            try:
                window = wait_for_window(process, log_path)
                wait_for_project(window, process, log_path, project)
                focus_window(window)
                ui = NativeUI(window, process, log_path, base / "ui-state.json", project)
                ui.open_file("main.py")
                ui.control("source-editor", focused=True)
                key("End", ("Control_L",))
                text("# unsaved\n")
                ui.source(original + "# unsaved\n")
                key("F5")
                ui.notice(
                    "Save modified sources before debugging. The debugger runs saved project files."
                )
                rpc = project / "debug-rpc.jsonl"
                assert not rpc.exists(), "Dirty source launched a debugger"
                key("s", ("Control_L",))
                wait_for(lambda: "unsaved" in source.read_text(), "Source save failed")
                # Code and debugger tools share the workspace; return to source.
                key("space", ("Control_L",))
                key("e")
                key("Home", ("Control_L",))
                ui.control("source-editor-at-start", focused=True)
                key("F9")
                ui.control("source-editor-breakpoints")
                key("F5")
                wait_for(
                    lambda: rpc.exists() and '"command": "scopes"' in rpc.read_text(),
                    "Debugger never paused",
                )
                ui.control("source-editor", focused=True)
                screenshot(window, label + "-paused.png")
                if not remote and not isolated:
                    ui.command("Debug: Show debugger")
                    screenshot(window, "persistent-controls.png")
                    runtime = next(i for i in service_rpc("runtimes") if i["kind"] == "debugger")
                    ui.click("debugger-persist")
                    # Quit immediately after requesting persistence; closing must
                    # preserve the mutation until the host acknowledges it.
                    close_window(window, process)
                    wait_for(
                        lambda: any(
                            i["id"] == runtime["id"] and i["persistent"] and not i["attached"]
                            for i in service_rpc("runtimes")
                        ),
                        "Debugger persistence/quit was not acknowledged",
                    )
                    process = subprocess.Popen(args, env=environment, stdout=log, stderr=log)
                    window = wait_for_window(process, log_path)
                    wait_for_project(window, process, log_path, project)
                    focus_window(window)
                    ui = NativeUI(window, process, log_path, base / "ui-state.json", project)
                    ui.command("Debug: Show debugger")
                    screenshot(window, "before-reattach.png")
                    ui.click("debugger-reattach")
                    wait_for(
                        lambda: any(
                            i["id"] == runtime["id"] and i["attached"]
                            for i in service_rpc("runtimes")
                        ),
                        "Debugger did not reattach",
                    )
                    screenshot(window, "reattached.png")
                messages = read_json_lines(rpc)
                assert any(
                    m.get("command") == "setBreakpoints" and m["arguments"]["breakpoints"]
                    for m in messages
                )
                pauses = sum(m.get("command") == "scopes" for m in read_json_lines(rpc))
                key("F10")
                wait_for(lambda: '"command": "next"' in rpc.read_text(), "F10 did not step")
                # A step request can precede its stopped/source-navigation reply,
                # especially while the debug terminal drains its output burst.
                # Let that source jump finish before requesting terminal focus.
                wait_for(
                    lambda: (
                        sum(m.get("command") == "scopes" for m in read_json_lines(rpc)) > pauses
                    ),
                    "Debugger did not inspect the stepped frame",
                )
                before = json.loads((base / "ui-state.json").read_text())["pass"]
                wait_for_ui_state(
                    process,
                    log_path,
                    base / "ui-state.json",
                    lambda state: (
                        state["pass"] > before
                        and any(
                            button["name"] == "source-editor"
                            and button["enabled"]
                            and button["focused"]
                            for button in state["buttons"]
                        )
                    ),
                    "Stepped source did not acquire input focus",
                )
                key("space", ("Control_L",))
                key("t")
                wait_for_ui_state(
                    process,
                    log_path,
                    base / "ui-state.json",
                    lambda state: any(
                        button["name"].startswith("terminal-input:debug-")
                        and button["enabled"]
                        and button["focused"]
                        for button in state["buttons"]
                    ),
                    "Debug terminal did not acquire input focus",
                )
                wait_for(
                    lambda: (
                        (project / "output-ready").exists()
                        and (project / "output-ready").read_text() == "ready"
                    ),
                    "Debuggee did not finish its output burst before requesting input",
                )
                text("RODE\n")
                wait_for(
                    lambda: (
                        (project / "input-result").exists()
                        and (project / "input-result").read_text() == "RODE"
                    ),
                    "Debug PTY input was not delivered completely",
                )
                assert (project / "input-result").read_text() == "RODE"
                # No background RPC or busy redraw loop while stopped and idle.
                time.sleep(0.5)

                def process_stats():
                    fields = (
                        Path(f"/proc/{process.pid}/stat").read_text().rsplit(") ", 1)[1].split()
                    )
                    return (int(fields[11]) + int(fields[12])) / os.sysconf("SC_CLK_TCK"), int(
                        fields[21]
                    ) * os.sysconf("SC_PAGE_SIZE")

                before_cpu, _ = process_stats()
                rpc_before = rpc.read_text()
                measured_at = time.monotonic()
                time.sleep(2)
                after_cpu, rss = process_stats()
                print(
                    f"{label}: paused idle CPU {(after_cpu - before_cpu) / (time.monotonic() - measured_at) * 100:.1f}%; RSS {rss / 1024 / 1024:.1f} MiB"
                )
                assert rpc.read_text() == rpc_before, "Idle debugger issued new requests"
                # Show the saved project's UI under both themes and min size.
                ui.command("Theme: Phosphor")
                screenshot(window, label + "-phosphor.png")
                ui.resize(850, 560)
                screenshot(window, label + "-compact.png")
                key("F5", ("Shift_L",))
                wait_for(
                    lambda: (project / "disconnected.json").exists(), "Stop did not disconnect"
                )
                assert json.loads((project / "disconnected.json").read_text())["terminateDebuggee"]
                close_window(window, process)
                process.wait(timeout=10)
                contents = (config / "app.ron").read_text()
                assert "breakpoints" in contents, "Breakpoint settings were not saved"
                assert process.returncode == 0
                print(label + " debugger UI passed")
            except Exception:
                if window is not None and process.poll() is None:
                    screenshot(window, label + "-failure.png")
                for source_path, suffix in (
                    (base / "ui-state.json", "layout.json"),
                    (project / "debug-rpc.jsonl", "rpc.jsonl"),
                ):
                    if source_path.exists():
                        shutil.copyfile(source_path, ARTIFACTS / (label + "-failure-" + suffix))
                raise
            finally:
                stop_process(process)
                for runtime in service_rpc("runtimes") or []:
                    service_rpc("end_runtime", id=runtime["id"], policy=runtime["policy"])


if not os.environ.get("RODE_DEBUGGER_UI_REMOTE_ONLY"):
    run()
    run(isolated=True)
if os.environ.get("RODE_TEST_SSH_CONFIG"):
    run(remote=True)
