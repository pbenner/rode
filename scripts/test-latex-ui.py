#!/usr/bin/env python3
"""Native LaTeX build/preview, diagnostics and generic settings, locally/SSH."""

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
os.environ["RODE_UI_ARTIFACTS"] = str(ROOT / "target/latex-ui")
from ui_harness import (
    ARTIFACTS,
    BINARY,
    close_window,
    display,
    focus_window,
    key,
    screenshot,
    text,
    x11,
)

x11.XResizeWindow.argtypes = [ctypes.c_void_p, ctypes.c_ulong, ctypes.c_uint, ctypes.c_uint]


def wait_for(check, message):
    deadline = time.monotonic() + 20
    while time.monotonic() < deadline:
        if check():
            return
        time.sleep(0.1)
    raise AssertionError(message)


def command(value):
    key("p", ("Control_L", "Shift_L"))
    text(value)
    key("Return")
    time.sleep(0.3)


def open_file(name):
    command("Open file by path")
    text(name)
    key("Return")
    time.sleep(0.4)


def run(remote=False, isolated=False):
    label = "ssh" if remote else "sandbox" if isolated else "local"
    with tempfile.TemporaryDirectory(prefix="rode-latex-ui-") as directory:
        base = Path(directory)
        project = base / "project"
        project.mkdir()
        source = project / "main.tex"
        original = "% UNFORMATTED ISSUE\n\\documentclass{article}\n\\usepackage{color}\n\\pagestyle{empty}\n\\begin{document}\n\\color{red}\\rule{200pt}{120pt}\nHello RODE\n\\end{document}\n"
        source.write_text(original)
        fake = project / "latex-tool"
        shutil.copy2(ROOT / "tests/fixtures/latex.py", fake)
        fake.chmod(0o755)
        shutil.copy2(ROOT / "tests/fixtures/pages.pdf", project / "pages.pdf")
        real = bool(os.environ.get("RODE_LATEX_UI_REAL")) and not remote and not isolated
        settings = dict(
            program="latexmk" if real else str(fake),
            arguments=[
                "-norc",
                "-pdf",
                "-interaction=nonstopmode",
                "-halt-on-error",
                "-file-line-error",
                "-no-shell-escape",
                "-outdir={outdir}",
                "{file}",
            ]
            if real
            else ["{file}", "{outdir}"],
            main_file="",
            output_directory="build/latex",
            pdf_file="",
            formatter=str(fake),
            checker=str(fake),
            timeout_seconds=120,
        )
        environment = dict(
            os.environ,
            XDG_CONFIG_HOME=str(base / "config"),
            XDG_DATA_HOME=str(base / "data"),
            HOME=str(base / "home"),
            TEXMFVAR=str(base / "texmf-var"),
            TEXMFCONFIG=str(base / "texmf-config"),
        )
        Path(environment["HOME"]).mkdir()
        host = ["rode-test", "", "", os.environ["RODE_TEST_SSH_CONFIG"], ""] if remote else None
        context_key = json.dumps([host, str(project)], separators=(",", ":"))
        preferences = {
            context_key: dict(
                latex=settings,
                sandbox=dict(enabled=isolated or remote),
                format_on_save=False,
                check_on_save=False,
                live_checks=False,
            )
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

                wait_for(mapped, "No LaTeX window")
                time.sleep(1)
                focus_window(window)
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
                        "SSH project did not become ready",
                    )
                    time.sleep(0.8)
                open_file("main.tex")
                key("End", ("Control_L",))
                text("\n% UNSAVED\n")
                command("LaTeX: Build and preview PDF")
                pdf = project / "build/latex/main.pdf"
                assert not pdf.exists(), "Build accepted a dirty source"
                assert source.read_text() == original
                key("s", ("Control_L",))
                wait_for(
                    lambda: "UNSAVED" in source.read_text(),
                    "Source did not save after the build guard",
                )
                command("Code: Format document")
                time.sleep(0.5)
                assert "UNFORMATTED" in source.read_text(), (
                    "Formatting wrote source behind the save guard"
                )
                key("s", ("Control_L",))
                wait_for(
                    lambda: "formatted" in source.read_text(),
                    "Configured formatter did not update/save the buffer",
                )
                command("Code: Check document")
                command("Code: Problems")
                screenshot(window, label + "-problems.png")
                command("Code: Problems")
                command("LaTeX: Build and preview PDF")

                def rendered(color, name):
                    screenshot(window, name)
                    raw = subprocess.check_output(
                        ["convert", str(ARTIFACTS / name), "-depth", "8", "rgb:-"]
                    )
                    choices = (
                        [(255, 0, 0), (229, 38, 38)]
                        if color == "red"
                        else [(0, 0, 255), (38, 38, 229)]
                    )
                    return sum(raw.count(bytes(rgb)) for rgb in choices) > 8000

                wait_for(
                    lambda: pdf.exists() and rendered("red", label + "-preview.png"),
                    "Compiled PDF did not render",
                )
                open_file("main.tex")
                key("a", ("Control_L",))
                text(
                    source.read_text().replace("\\color{red}", "\\color{blue}") + "\n% BLUE_BUILD\n"
                )
                key("s", ("Control_L",))
                wait_for(
                    lambda: "BLUE_BUILD" in source.read_text(), "Second source version did not save"
                )
                command("LaTeX: Build and preview PDF")
                wait_for(
                    lambda: rendered("blue", label + "-reloaded-pdf.png"),
                    "Existing PDF tab did not reload",
                )
                x11.XResizeWindow(display, window, 850, 560)
                x11.XFlush(display)
                time.sleep(0.4)
                screenshot(window, label + "-compact.png")
                x11.XResizeWindow(display, window, 1380, 900)
                x11.XFlush(display)
                open_file("main.tex")
                key("a", ("Control_L",))
                text(
                    "\\documentclass{article}\n\\begin{document}\n\\undefinedRODEcommand\n\\end{document}\n% ERROR\n"
                )
                key("s", ("Control_L",))
                wait_for(lambda: "ERROR" in source.read_text(), "Invalid source did not save")
                command("LaTeX: Build PDF")
                build_log = project / "build/latex/main.log"
                wait_for(
                    lambda: (
                        build_log.exists()
                        and "Undefined control sequence" in build_log.read_text(errors="replace")
                    ),
                    "Compiler error was not captured",
                )
                time.sleep(0.4)
                screenshot(window, label + "-compiler-errors.png")
                close_window(window, process)
                print(
                    label
                    + ": LaTeX dirty guard, custom tools, diagnostics, build/preview, PDF reload, compact layout and compiler errors passed."
                )
            except Exception:
                if window and process.poll() is None:
                    screenshot(window, label + "-failure.png")
                raise
            finally:
                if process.poll() is None:
                    process.terminate()
                    process.wait(timeout=5)


if os.environ.get("RODE_LATEX_UI_REMOTE_ONLY") != "1":
    run()
    run(isolated=True)
if os.environ.get("RODE_TEST_SSH_CONFIG"):
    run(remote=True)
