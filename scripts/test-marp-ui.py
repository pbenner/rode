#!/usr/bin/env python3
"""Native Marp slides, source state, exports and dirty guards (local/SSH)."""

import ctypes
import json
import os
import re
import shutil
import subprocess
import tempfile
import time
from pathlib import Path

os.environ["RODE_UI_ARTIFACTS"] = str(Path(__file__).resolve().parent.parent / "target/marp-ui")
from ui_harness import ARTIFACTS, BINARY, click, close_window, display, key, screenshot, text, x11

x11.XResizeWindow.argtypes = [ctypes.c_void_p, ctypes.c_ulong, ctypes.c_uint, ctypes.c_uint]
ROOT = Path(__file__).resolve().parent.parent


def wait_for(check, message, timeout=35):
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        if check():
            return
        time.sleep(0.15)
    raise AssertionError(message)


def command(name):
    key("p", ("Control_L", "Shift_L"))
    text(name)
    key("Return")
    time.sleep(0.35)


def open_file(name):
    # The SSH login terminal initially owns focus; Ctrl+P belongs to its shell.
    command("Open file by path")
    text(name)
    key("Return")
    time.sleep(0.4)


def run(remote=False):
    label = "ssh" if remote else "local"
    with tempfile.TemporaryDirectory(prefix="rode-marp-ui-") as directory:
        base = Path(directory)
        project = base / "project"
        project.mkdir()
        source = project / "deck.marp.md"
        original = "---\nmarp: true\ntheme: default\npaginate: true\nbackgroundColor: '#1c2331'\ncolor: '#ffffff'\n---\n# Marp in RODE\n\n---\n\n<!-- _backgroundColor: '#163b2c' -->\n# Second slide\n"
        source.write_text(original)
        fake = project / "marp.py"
        shutil.copy2(ROOT / "tests/fixtures/marp.py", fake)
        fake.chmod(0o755)
        shutil.copy2(ROOT / "tests/fixtures/pages.pdf", project / "pages.pdf")
        real = bool(os.environ.get("RODE_TEST_MARP")) and not remote
        program = os.environ["RODE_TEST_MARP"] if real else str(fake)
        browser = os.environ.get("RODE_TEST_MARP_BROWSER", "") if real else ""
        settings = {
            "program": program,
            "browser": browser,
            "browser_kind": "auto",
            "allow_local_files": False,
            "theme_css": "",
        }
        environment = dict(
            os.environ, XDG_CONFIG_HOME=str(base / "config"), XDG_DATA_HOME=str(base / "data")
        )
        host = ["rode-test", "", "", os.environ["RODE_TEST_SSH_CONFIG"], ""] if remote else None
        context_key = json.dumps([host, str(project)], separators=(",", ":"))
        policy = {"enabled": remote, "share_network": False}
        preferences = {
            context_key: {
                "marp": settings,
                "sandbox": policy,
                "format_on_save": False,
                "check_on_save": False,
                "live_checks": False,
            }
        }
        config = base / "data/rode"
        config.mkdir(parents=True)
        (config / "app.ron").write_text(
            json.dumps({"project_preferences": json.dumps(preferences)})
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
            args.append(str(project))
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

                def terminal_attached():
                    # Project readiness precedes async terminal attachment.
                    # Wait for its initial focus handoff before driving dialogs.
                    reply = subprocess.run(
                        [str(BINARY), "--session-service", "request"],
                        input=b'{"action":"list"}',
                        env=environment,
                        stdout=subprocess.PIPE,
                        stderr=subprocess.PIPE,
                    )
                    if reply.returncode:
                        return False
                    response = json.loads(reply.stdout)
                    return response.get("kind") == "sessions" and any(
                        info["root"] == str(project) and info["clients"] > 0
                        for info in response.get("data", [])
                    )

                wait_for(mapped, "No Marp window")
                time.sleep(0.7)
                x11.XSetInputFocus(display, window, 1, 0)
                x11.XFlush(display)
                if remote:
                    wait_for(
                        lambda: (
                            "[SSH"
                            in subprocess.check_output(["xwininfo", "-id", hex(window)], text=True)
                            and "Opening project"
                            not in subprocess.check_output(
                                ["xwininfo", "-id", hex(window)], text=True
                            )
                        ),
                        "SSH project did not finish opening",
                    )
                    wait_for(terminal_attached, "SSH project terminal did not attach")
                open_file("deck.marp.md")
                if remote:
                    key("grave", ("Control_L",))
                key("End", ("Control_L",))
                text("\nUNSAVED_SNAPSHOT\n")
                command("Marp: Toggle slide preview")

                def slide(number):
                    filename = label + f"-slide-{number}.png"
                    screenshot(window, filename)
                    if not real:
                        sample = subprocess.check_output(
                            [
                                "convert",
                                str(ARTIFACTS / filename),
                                "-format",
                                "%[pixel:p{840,600}]",
                                "info:",
                            ],
                            text=True,
                        )
                        red, green, blue = map(int, re.findall(r"\d+", sample)[:3])
                        return (
                            (red > 180 and green < 100 and blue < 100)
                            if number == 1
                            else (blue > 180 and red < 100)
                        )
                    pixels = subprocess.check_output(
                        ["convert", str(ARTIFACTS / filename), "-depth", "8", "rgb:-"]
                    )
                    wanted = ((28, 35, 49), (22, 59, 44))[number - 1]
                    return pixels.count(bytes(wanted)) > 15000

                wait_for(lambda: slide(1), "Marp first slide was not rendered")
                assert source.read_text() == original, "Preview saved or changed the source"
                click(800, 450)
                key("Next")
                wait_for(lambda: slide(2), "PageDown did not navigate slides")
                key("s", ("Control_L",))
                wait_for(
                    lambda: "UNSAVED_SNAPSHOT" in source.read_text(),
                    "Saving from Slides lost the source",
                )
                key("f", ("Control_L",))  # Find must return to source editing.
                key("Escape")
                key("End", ("Control_L",))
                text("\nEXPORTED_UNSAVED\n")
                command("Marp: Export HTML")
                key("a", ("Control_L",))
                text("deck.html")
                key("Return")
                output = project / "deck.html"
                wait_for(output.exists, "HTML export did not finish")
                assert "EXPORTED_UNSAVED" in output.read_text()
                assert "EXPORTED_UNSAVED" not in source.read_text(), (
                    "Export silently saved Markdown"
                )
                # Replacing a previous export is explicit in the export dialog.
                key("End", ("Control_L",))
                text("\nREPLACED_EXPORT\n")
                command("Marp: Export HTML")
                key("a", ("Control_L",))
                text("deck.html")
                key("Tab")
                key("space")
                key("Tab")
                key("Return")
                wait_for(
                    lambda: "REPLACED_EXPORT" in output.read_text(),
                    "Explicit export replacement failed",
                )
                screenshot(window, label + "-exported.png")
                key("s", ("Control_L",))
                wait_for(
                    lambda: "REPLACED_EXPORT" in source.read_text(),
                    "Source focus was lost after export",
                )
                command("Marp: New presentation")
                key("w", ("Control_L",))
                screenshot(window, label + "-new-dirty-guard.png")
                key("Escape")
                key("s", ("Control_L",))
                key("a", ("Control_L",))
                text("new.marp.md")
                key("Return")
                wait_for(
                    lambda: (project / "new.marp.md").exists(),
                    "New Marp presentation Save As failed",
                )
                assert "marp: true" in (project / "new.marp.md").read_text()
                open_file("deck.marp.md")
                command("Marp: Toggle slide preview")
                click(800, 450)
                key("Home")
                wait_for(lambda: slide(1), "Returning to Slides failed")
                x11.XResizeWindow(display, window, 850, 560)
                x11.XFlush(display)
                time.sleep(0.4)
                screenshot(window, label + "-compact.png")
                close_window(window, process)
                # Quit while a CLI owns a detached browser-shaped process.
                # A coordinator must finish cleanup even after the GUI is gone.
                settings["program"] = str(fake)
                settings["browser"] = ""
                # The main pass exercises bwrap. Shutdown also has to work on
                # an SSH host with normal access and no PID namespace cleanup.
                if remote:
                    policy["enabled"] = False
                preferences[context_key]["marp"] = settings
                (config / "app.ron").write_text(
                    json.dumps({"project_preferences": json.dumps(preferences)})
                )
                (project / "cancel.marp.md").write_text("---\nmarp: true\n---\nRODE_TEST_SLEEP\n")
                shutdown_args = [arg for arg in args if arg != "--sandbox"] if remote else args
                process = subprocess.Popen(shutdown_args, env=environment, stdout=log, stderr=log)
                wait_for(mapped, "No shutdown-test window")
                time.sleep(0.5)
                x11.XSetInputFocus(display, window, 1, 0)
                x11.XFlush(display)
                if remote:
                    wait_for(
                        lambda: (
                            "Opening project"
                            not in subprocess.check_output(
                                ["xwininfo", "-id", hex(window)], text=True
                            )
                            and "[SSH"
                            in subprocess.check_output(["xwininfo", "-id", hex(window)], text=True)
                        ),
                        "SSH shutdown-test project not ready",
                    )
                    wait_for(terminal_attached, "SSH shutdown-test terminal did not attach")
                open_file("cancel.marp.md")
                if remote:
                    key("grave", ("Control_L",))
                command("Marp: Toggle slide preview")
                wait_for(
                    lambda: (project / "marp-pids.json").exists(), "Shutdown fixture did not start"
                )
                close_window(window, process)
                wait_for(
                    lambda: (project / "marp-stopped").exists(),
                    "Closing RODE orphaned the renderer's browser",
                )
            except Exception:
                if window and process.poll() is None:
                    screenshot(window, label + "-failure.png")
                raise
            finally:
                if process.poll() is None:
                    process.terminate()
                    process.wait(timeout=10)
    print(
        label
        + " Marp slides, navigation, snapshots, source saves, export replacement, dirty guards and renderer shutdown passed"
    )


if __name__ == "__main__":
    if not os.environ.get("RODE_MARP_UI_REMOTE_ONLY"):
        run()
    if os.environ.get("RODE_TEST_SSH_CONFIG"):
        run(True)
