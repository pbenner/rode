#!/usr/bin/env python3
"""Native LaTeX build/preview, diagnostics and generic settings, locally/SSH."""

import json
import os
import shutil
import subprocess
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
os.environ["RODE_UI_ARTIFACTS"] = str(ROOT / "target/latex-ui")
from ui_fixture import native_session, project_args
from ui_harness import ARTIFACTS, fixture_directory, fixture_environment, key, screenshot, text


def guarded_tools(ui, source, original, label):
    ui.open_file("main.tex")
    ui.control("source-editor", focused=True)
    key("End", ("Control_L",))
    text("\n% UNSAVED\n")
    ui.source(original + "\n% UNSAVED\n")
    ui.command("LaTeX: Build and preview PDF")
    ui.notice("Save modified files before building LaTeX. Builds use the saved project files.")
    assert not (ui.project_root / "build/latex/main.pdf").exists(), "Build accepted a dirty source"
    assert source.read_text() == original
    ui.dismiss_notice()
    key("s", ("Control_L",))
    ui.wait(lambda: "UNSAVED" in source.read_text(), "Source did not save after the build guard")
    ui.control("source-editor-saved")
    ui.command("Code: Format document")
    ui.source((original + "\n% UNSAVED\n").replace("UNFORMATTED", "formatted"))
    assert "UNFORMATTED" in source.read_text(), "Formatting wrote source behind the save guard"
    key("s", ("Control_L",))
    ui.wait(
        lambda: "formatted" in source.read_text(),
        "Configured formatter did not update/save the buffer",
    )
    ui.control("source-editor-saved")
    ui.command("Code: Check document")
    ui.command("Code: Problems")
    ui.control("workbench-panel:PROBLEMS")
    screenshot(ui.window, label + "-problems.png")
    ui.command("Code: Problems")


def build_preview(ui, source, label):
    def rendered(color, name):
        screenshot(ui.window, name)
        raw = subprocess.check_output(["convert", str(ARTIFACTS / name), "-depth", "8", "rgb:-"])
        choices = [(255, 0, 0), (229, 38, 38)] if color == "red" else [(0, 0, 255), (38, 38, 229)]
        return sum(raw.count(bytes(rgb)) for rgb in choices) > 8000

    ui.command("LaTeX: Build and preview PDF")
    pdf = ui.project_root / "build/latex/main.pdf"
    ui.wait(
        lambda: pdf.exists() and rendered("red", label + "-preview.png"),
        "Compiled PDF did not render",
        timeout=120,
    )
    ui.open_file("main.tex")
    updated = source.read_text().replace("\\color{red}", "\\color{blue}") + "\n% BLUE_BUILD\n"
    ui.fill("source-editor", updated)
    key("s", ("Control_L",))
    ui.wait(lambda: source.read_text() == updated, "Second source version did not save")
    ui.control("source-editor-saved")
    ui.command("LaTeX: Build and preview PDF")
    ui.wait(
        lambda: rendered("blue", label + "-reloaded-pdf.png"),
        "Existing PDF tab did not reload",
        timeout=120,
    )
    ui.resize(850, 560)
    screenshot(ui.window, label + "-compact.png")
    ui.resize(1380, 900)


def compiler_errors(ui, source, label):
    ui.open_file("main.tex")
    invalid = "\\documentclass{article}\n\\begin{document}\n\\undefinedRODEcommand\n\\end{document}\n% ERROR\n"
    ui.fill("source-editor", invalid)
    key("s", ("Control_L",))
    ui.wait(lambda: source.read_text() == invalid, "Invalid source did not save")
    ui.control("source-editor-saved")
    ui.command("LaTeX: Build PDF")
    build_log = ui.project_root / "build/latex/main.log"
    ui.wait(
        lambda: "Undefined control sequence" in build_log.read_text(errors="replace"),
        "Compiler error was not captured",
    )
    ui.layout(
        lambda state: any(
            button["name"].startswith("latex-result:") and button["name"] != "latex-result:0"
            for button in state["buttons"]
        ),
        "Compiler failure did not reach the UI",
    )
    screenshot(ui.window, label + "-compiler-errors.png")


def run(remote=False, isolated=False):
    label = "ssh" if remote else "sandbox" if isolated else "local"
    with fixture_directory(prefix="rode-latex-ui-") as base:
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
        environment = fixture_environment(
            base,
            XDG_CONFIG_HOME=str(base / "config"),
            XDG_DATA_HOME=str(base / "data"),
            HOME=str(base / "home"),
            TEXMFVAR=str(base / "texmf-var"),
            TEXMFCONFIG=str(base / "texmf-config"),
        )
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
        args = project_args(project, remote=remote, sandbox=isolated or remote)
        with native_session(args, environment, project, label) as ui:
            guarded_tools(ui, source, original, label)
            build_preview(ui, source, label)
            compiler_errors(ui, source, label)
        print(
            label
            + ": LaTeX protected sources, custom tools, build/preview, PDF reload and diagnostics passed."
        )


if os.environ.get("RODE_LATEX_UI_REMOTE_ONLY") != "1":
    run()
    run(isolated=True)
if os.environ.get("RODE_TEST_SSH_CONFIG"):
    run(remote=True)
