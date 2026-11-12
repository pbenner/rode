#!/usr/bin/env python3
"""Native hex edits, protected saves, static analysis and extension preferences."""

import hashlib
import json
import os
import shutil
import subprocess
import time
from pathlib import Path

os.environ["RODE_UI_ARTIFACTS"] = str(Path(__file__).resolve().parent.parent / "target/binary-ui")
from ui_harness import (
    ARTIFACTS,
    BINARY,
    NativeUI,
    close_window,
    fixture_directory,
    fixture_environment,
    focus_window,
    key,
    read_preferences,
    screenshot,
    stop_process,
    text,
    wait_for_project,
    wait_for_window,
)


def wait_for(check, message):
    until = time.monotonic() + 10
    while time.monotonic() < until:
        if check():
            return
        time.sleep(0.1)
    raise AssertionError(message)


def toggle_extension(ui, identity):
    """Filter by stable ID, then activate the checkbox using native keyboard focus."""
    key("x", ("Control_L", "Shift_L"))
    ui.fill("extension-filter", identity)
    ui.click("extension-toggle:" + identity)


def first_byte(ui):
    ui.click("hex-offset")
    ui.fill("hex-offset", "0")
    ui.click("hex-go")
    ui.click("hex-byte:0")
    ui.control("hex-grid", focused=True)


def preferences(path):
    if not path.exists():
        return {}
    values = read_preferences(path)
    return json.loads(values.get("extension_preferences", "{}"))


def run(remote=False):
    label = "ssh" if remote else "local"
    with fixture_directory(prefix="rode-binary-ui-") as base:
        env = fixture_environment(base)
        root = base / "project"
        root.mkdir()
        source = root / "sample.bin"
        original = bytes(range(256)) * 16
        source.write_bytes(original)
        (root / "note.txt").write_text("keep source\n")
        shutil.copyfile("/bin/true", root / "program")
        adapter = root / "adapter.py"
        shutil.copyfile(
            Path(__file__).resolve().parent.parent / "tests/fixtures/extension.py", adapter
        )
        manifests = base / "config/rode/extensions"
        manifests.mkdir(parents=True)
        (manifests / "test.adapter.json").write_text(
            json.dumps(
                {
                    "api": 1,
                    "id": "test.adapter",
                    "name": "Test adapter",
                    "version": "1.0",
                    "description": "A user-installed formatter and inspection command",
                    "tool": {
                        "program": "python3",
                        "args": [str(adapter)],
                        "languages": ["text"],
                        "format": True,
                        "check": True,
                        "commands": [{"id": "test.inspect", "title": "Inspect project"}],
                    },
                }
            )
        )
        args = [str(BINARY), "--theme", "carbon"]
        if remote:
            tools = base / "tools"
            tools.mkdir()
            wrapper = tools / "ssh"
            wrapper.write_text(
                "#!/usr/bin/python3\nimport os,sys\nbinary="
                + repr(shutil.which("ssh"))
                + "\nos.execv(binary,[binary]+['-T' if a=='-tt' else a for a in sys.argv[1:]])\n"
            )
            wrapper.chmod(0o755)
            env["PATH"] = str(tools) + os.pathsep + os.environ["PATH"]
            args += [
                "--ssh",
                "rode-test",
                "--ssh-config",
                os.environ["RODE_TEST_SSH_CONFIG"],
                "--remote-path",
                str(root),
                "--sandbox",
            ]
        else:
            args += [str(root)]
        env["RODE_UI_TEST_STATE"] = str(base / "ui-state.json")
        log_path = ARTIFACTS / (label + ".log")
        with log_path.open("w") as log:
            process = subprocess.Popen(args, env=env, stdout=log, stderr=log)
            window = None
            try:
                window = wait_for_window(process, log_path)
                wait_for_project(window, process, log_path, root)
                focus_window(window)
                ui = NativeUI(window, process, log_path, env["RODE_UI_TEST_STATE"], root)
                ui.open_file("sample.bin")
                screenshot(window, label + "-hex.png")
                ui.click("hex-edit")
                ui.click("hex-patch")
                ui.fill("hex-patch", "90 90 C3")
                ui.click("hex-overwrite")
                ui.absent("hex-saved")
                key("s", ("Control_L",))
                wait_for(
                    lambda: source.read_bytes()[:3] == b"\x90\x90\xc3",
                    "Hex overwrite was not saved",
                )
                first_byte(ui)
                text("cc")
                ui.absent("hex-saved")
                key("z", ("Control_L",))
                ui.control("hex-saved")
                key("s", ("Control_L",))
                ui.control("hex-saved")
                assert source.read_bytes()[:3] == b"\x90\x90\xc3", (
                    "Byte undo did not restore the saved patch"
                )
                first_byte(ui)
                text("cc")
                ui.absent("hex-saved")
                key("s", ("Control_L",))
                wait_for(lambda: source.read_bytes()[0] == 0xCC, "Direct hex input failed")
                # Save conflict must preserve both external data and dirty byte edits.
                ui.dismiss_notice()
                first_byte(ui)
                text("aa")
                ui.absent("hex-saved")
                external = b"\xee" + source.read_bytes()[1:]
                source.write_bytes(external)
                key("s", ("Control_L",))
                ui.control("notice-dismiss")
                assert source.read_bytes() == external, "Binary conflict overwrote external changes"
                screenshot(window, label + "-conflict.png")
                key("w", ("Control_L",))
                ui.control("guard-keep-editing")
                screenshot(window, label + "-dirty-guard.png")
                key("Escape")
                ui.absent("guard-keep-editing")
                key("s", ("Control_L", "Shift_L"))
                ui.fill_path("copy.bin")
                key("Return")
                wait_for(lambda: (root / "copy.bin").exists(), "Binary Save As failed")
                ui.absent("file-dialog-path")
                ui.control("hex-saved")
                assert (root / "copy.bin").read_bytes()[0] == 0xAA
                ui.open_file("program")
                ui.click("binary-tab:Overview")
                screenshot(window, label + "-object.png")
                ui.click("binary-tab:Disassembly")
                ui.click("hex-analyze")
                ui.control("hex-analysis-ready")
                screenshot(window, label + "-disassembly.png")
                # The direct shortcut opens the manager; toggles save immediately.
                toggle_extension(ui, "rode.monitor")
                screenshot(window, label + "-extension-manager.png")
                prefs = base / "data/rode/app.ron"
                wait_for(
                    lambda: preferences(prefs).get("rode.monitor", {}).get("enabled") is False,
                    "Extension preference was not persisted",
                )
                key("Escape")
                ui.absent("extension-filter")
                toggle_extension(ui, "test.adapter")
                screenshot(window, label + "-installed-adapter.png")
                wait_for(
                    lambda: preferences(prefs).get("test.adapter", {}).get("enabled") is True,
                    "Installed adapter checkbox was not enabled",
                )
                screenshot(window, label + "-adapter-enabled.png")
                key("Escape")
                ui.absent("extension-filter")
                ui.open_file("note.txt")
                key("a", ("Control_L",))
                key("x", ("Control_L", "Shift_L"))  # Must not cut the selected source.
                ui.control("extension-filter")
                key("Escape")
                ui.absent("extension-filter")
                ui.command("Code: Format document")
                ui.control("source-editor-modified")
                key("s", ("Control_L",))
                wait_for(
                    lambda: (root / "note.txt").read_text() == "KEEP SOURCE\n",
                    "Installed formatter did not update/save the buffer",
                )
                ui.command("Test adapter: Inspect project")
                output = f"Adapter on {root} · test.inspect"
                ui.control("extension-output-value:" + hashlib.sha256(output.encode()).hexdigest())
                screenshot(window, label + "-adapter-output.png")
                close_window(window, process)
            except Exception:
                if window and process.poll() is None:
                    screenshot(window, label + "-failure.png")
                raise
            finally:
                stop_process(process)
    print(label + " hex / extension UI checks passed")


if __name__ == "__main__":
    if not os.environ.get("RODE_BINARY_UI_REMOTE_ONLY"):
        run()
    if os.environ.get("RODE_TEST_SSH_CONFIG"):
        run(True)
