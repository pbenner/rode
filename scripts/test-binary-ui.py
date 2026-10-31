#!/usr/bin/env python3
"""Native hex edits, protected saves, static analysis and extension preferences."""

import json
import os
import re
import shutil
import subprocess
import tempfile
import time
from pathlib import Path

os.environ["RODE_UI_ARTIFACTS"] = str(Path(__file__).resolve().parent.parent / "target/binary-ui")
from ui_harness import (
    ARTIFACTS,
    BINARY,
    click,
    close_window,
    display,
    focus_window,
    key,
    screenshot,
    text,
    x11,
    xtest,
)


def wait_for(check, message):
    until = time.monotonic() + 10
    while time.monotonic() < until:
        if check():
            return
        time.sleep(0.1)
    raise AssertionError(message)


def command(name):
    key("p", ("Control_L", "Shift_L"))
    text(name)
    key("Return")
    time.sleep(0.35)


def open_file(name):
    command("Open file by path")
    text(name)
    key("Return")
    time.sleep(0.5)


def wheel(x, y, down=True, ticks=8):
    xtest.XTestFakeMotionEvent(display, -1, x, y, 0)
    for _ in range(ticks):
        xtest.XTestFakeButtonEvent(display, 5 if down else 4, 1, 0)
        xtest.XTestFakeButtonEvent(display, 5 if down else 4, 0, 0)
        x11.XFlush(display)
        time.sleep(0.07)
    time.sleep(0.3)


def preferences(path):
    if not path.exists():
        return {}
    content = path.read_text().rstrip()
    values = json.loads(content[:-1].rstrip().removesuffix(",") + "}")
    return json.loads(values.get("extension_preferences", "{}"))


def run(remote=False):
    label = "ssh" if remote else "local"
    with tempfile.TemporaryDirectory(prefix="rode-binary-ui-") as directory:
        base = Path(directory)
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
        env = dict(
            os.environ, XDG_CONFIG_HOME=str(base / "config"), XDG_DATA_HOME=str(base / "data")
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
        with (ARTIFACTS / (label + ".log")).open("w") as log:
            process = subprocess.Popen(args, env=env, stdout=log, stderr=log)
            window = None
            try:

                def mapped():
                    nonlocal window
                    assert process.poll() is None
                    tree = subprocess.check_output(["xwininfo", "-root", "-tree"], text=True)
                    match = re.search(r'(0x[0-9a-f]+) "RODE', tree)
                    if match:
                        window = int(match[1], 16)
                    return match

                wait_for(mapped, "No binary editor window")
                time.sleep(0.8)
                focus_window(window)
                # These coordinates are relative to the ordinary local project layout.
                # Remote sandbox banners add 40 px above document content.
                dy = 40 if remote else 0
                open_file("sample.bin")
                screenshot(window, label + "-hex.png")
                click(315, 215 + dy)  # Edit bytes
                click(480, 215 + dy)
                key("a", ("Control_L",))
                text("90 90 C3")
                click(592, 215 + dy)  # Overwrite
                key("s", ("Control_L",))
                wait_for(
                    lambda: source.read_bytes()[:3] == b"\x90\x90\xc3",
                    "Hex overwrite was not saved",
                )
                click(405, 310 + dy)  # byte zero; direct typing and undo should reach the grid
                text("cc")
                key("z", ("Control_L",))
                key("s", ("Control_L",))
                time.sleep(0.3)
                assert source.read_bytes()[:3] == b"\x90\x90\xc3", (
                    "Byte undo did not restore the saved patch"
                )
                click(405, 310 + dy)
                text("cc")
                key("s", ("Control_L",))
                wait_for(lambda: source.read_bytes()[0] == 0xCC, "Direct hex input failed")
                # Save conflict must preserve both external data and dirty byte edits.
                click(405, 310 + dy)
                text("aa")
                external = b"\xee" + source.read_bytes()[1:]
                source.write_bytes(external)
                key("s", ("Control_L",))
                time.sleep(0.4)
                assert source.read_bytes() == external, "Binary conflict overwrote external changes"
                screenshot(window, label + "-conflict.png")
                key("w", ("Control_L",))
                time.sleep(0.2)
                screenshot(window, label + "-dirty-guard.png")
                key("Escape")
                key("s", ("Control_L", "Shift_L"))
                key("a", ("Control_L",))
                text("copy.bin")
                key("Return")
                wait_for(lambda: (root / "copy.bin").exists(), "Binary Save As failed")
                assert (root / "copy.bin").read_bytes()[0] == 0xAA
                open_file("program")
                click(466, 150 + dy)  # Overview
                screenshot(window, label + "-object.png")
                click(930, 150 + dy)  # Disassembly
                click(580, 182 + dy)  # Analyze / decode here
                time.sleep(0.6)
                screenshot(window, label + "-disassembly.png")
                # The direct shortcut opens the manager; toggles save immediately.
                key("x", ("Control_L", "Shift_L"))
                screenshot(window, label + "-extension-manager.png")
                click(427, 442)  # System monitor, after Debugger and Git in the centered modal.
                prefs = base / "data/rode/app.ron"
                wait_for(
                    lambda: preferences(prefs).get("rode.monitor", {}).get("enabled") is False,
                    "Extension preference was not persisted",
                )
                key("Escape")
                key("x", ("Control_L", "Shift_L"))  # Reopen the manager after its layout changes.
                wheel(900, 500, ticks=20)
                screenshot(window, label + "-installed-adapter.png")
                # Last entry is the installed adapter; locating its checkbox by
                # screenshot is stable once scrolled to the bottom.
                click(427, 535)
                wait_for(
                    lambda: preferences(prefs).get("test.adapter", {}).get("enabled") is True,
                    "Installed adapter checkbox was not enabled",
                )
                screenshot(window, label + "-adapter-enabled.png")
                key("Escape")
                open_file("note.txt")
                key("a", ("Control_L",))
                key("x", ("Control_L", "Shift_L"))  # Must not cut the selected source.
                key("Escape")
                command("Code: Format document")
                time.sleep(0.5)
                key("s", ("Control_L",))
                wait_for(
                    lambda: (root / "note.txt").read_text() == "KEEP SOURCE\n",
                    "Installed formatter did not update/save the buffer",
                )
                command("Test adapter: Inspect project")
                time.sleep(0.4)
                screenshot(window, label + "-adapter-output.png")
                close_window(window, process)
            except Exception:
                if window and process.poll() is None:
                    screenshot(window, label + "-failure.png")
                raise
            finally:
                if process.poll() is None:
                    if window:
                        close_window(window, process)
                    else:
                        process.terminate()
                        process.wait(timeout=5)
    print(label + " hex / extension UI checks passed")


if __name__ == "__main__":
    if not os.environ.get("RODE_BINARY_UI_REMOTE_ONLY"):
        run()
    if os.environ.get("RODE_TEST_SSH_CONFIG"):
        run(True)
