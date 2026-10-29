#!/usr/bin/env python3
"""Native debugger controls, source guards, PTY input and SSH sandbox UI."""

import ctypes
import json
import os
import re
import shutil
import subprocess
import tempfile
import time
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
os.environ["RODE_UI_ARTIFACTS"] = str(ROOT / "target/debugger-ui")
from ui_harness import ARTIFACTS, BINARY, close_window, display, key, screenshot, text, theme, x11

x11.XResizeWindow.argtypes = [ctypes.c_void_p, ctypes.c_ulong, ctypes.c_uint, ctypes.c_uint]


def wait_for(check, message):
    deadline = time.monotonic() + 20
    while time.monotonic() < deadline:
        if check():
            return
        time.sleep(0.05)
    raise AssertionError(message)


def command(value):
    key("p", ("Control_L", "Shift_L"))
    text(value)
    key("Return")
    time.sleep(0.2)


def run(remote=False, isolated=False):
    label = "ssh" if remote else "sandbox" if isolated else "local"
    with tempfile.TemporaryDirectory(prefix="rode-debugger-ui-") as directory:
        base = Path(directory)
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
        )
        Path(environment["HOME"]).mkdir()
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
        with (ARTIFACTS / (label + ".log")).open("w") as log:
            process = subprocess.Popen(args, env=environment, stdout=log, stderr=log)
            window = None
            try:

                def mapped():
                    nonlocal window
                    assert process.poll() is None
                    match = re.search(
                        r'(0x[0-9a-f]+) "RODE',
                        subprocess.check_output(["xwininfo", "-root", "-tree"], text=True),
                    )
                    if match:
                        window = int(match[1], 16)
                    return match

                wait_for(mapped, "No debugger window")
                time.sleep(1.5)
                x11.XSetInputFocus(display, window, 1, 0)
                x11.XFlush(display)
                command("Open file by path")
                text("main.py")
                key("Return")
                time.sleep(0.4)
                key("End", ("Control_L",))
                text("# unsaved\n")
                key("F5")
                time.sleep(0.3)
                rpc = project / "debug-rpc.jsonl"
                assert not rpc.exists(), "Dirty source launched a debugger"
                key("s", ("Control_L",))
                wait_for(lambda: "unsaved" in source.read_text(), "Source save failed")
                # Return to source after the guard; F9 must persist immediately.
                key("Home", ("Control_L",))
                key("F9")
                key("F5")
                wait_for(
                    lambda: rpc.exists() and '"command": "scopes"' in rpc.read_text(),
                    "Debugger never paused",
                )
                time.sleep(0.5)
                screenshot(window, label + "-paused.png")
                messages = [json.loads(line) for line in rpc.read_text().splitlines()]
                assert any(
                    m.get("command") == "setBreakpoints" and m["arguments"]["breakpoints"]
                    for m in messages
                )
                key("F10")
                wait_for(lambda: '"command": "next"' in rpc.read_text(), "F10 did not step")
                time.sleep(0.5)
                command("Toggle terminal")
                command("Toggle terminal")
                text("RODE\n")
                wait_for(
                    lambda: (project / "input-result").exists(), "Debug PTY input was not delivered"
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
                theme("phosphor")
                screenshot(window, label + "-phosphor.png")
                x11.XResizeWindow(display, window, 850, 560)
                x11.XFlush(display)
                time.sleep(0.4)
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
            finally:
                if process.poll() is None:
                    process.terminate()
                    process.wait(timeout=8)


if not os.environ.get("RODE_DEBUGGER_UI_REMOTE_ONLY"):
    run()
    run(isolated=True)
if os.environ.get("RODE_TEST_SSH_CONFIG"):
    run(remote=True)
