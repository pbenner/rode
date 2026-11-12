#!/usr/bin/env python3
"""Native language highlighting, Go run/build, and Prettier buffer checks."""

import json
import os
import shutil
import subprocess
import time
from pathlib import Path

os.environ["RODE_UI_ARTIFACTS"] = str(
    Path(__file__).resolve().parent.parent / "target/languages-ui"
)
from ui_harness import (
    ARTIFACTS,
    BINARY,
    NativeUI,
    close_window,
    fixture_directory,
    fixture_environment,
    focus_window,
    key,
    screenshot,
    stop_process,
    text,
    wait_for_project,
    wait_for_window,
)


def wait_for(check, message):
    deadline = time.monotonic() + 60
    while time.monotonic() < deadline:
        if check():
            return
        time.sleep(0.1)
    raise AssertionError(message)


def run():
    assert shutil.which("go") and shutil.which("gofmt"), "Add Go to PATH"
    prettier = Path(os.environ["RODE_TEST_PRETTIER"])
    with fixture_directory(prefix="rode-languages-ui-") as base:
        root = base / "project"
        root.mkdir()
        # os.WriteFile needs Go 1.16. A newer minimum makes older CI Go
        # installations fetch a toolchain instead of exercising the editor.
        (root / "go.mod").write_text("module example.test/rode-languages\n\ngo 1.16\n")
        go = root / "main.go"
        go.write_text(
            'package main\nimport "os"\nfunc main(){os.WriteFile("ran.ok",[]byte("ok"),0600)}\n'
        )
        ts = root / "main.ts"
        ts.write_text('const value:string="hello";\n')
        tools = root / "node_modules/.bin"
        tools.mkdir(parents=True)
        (tools / "prettier").symlink_to(prettier)
        shell = base / "shell"
        shell.write_text("#!/bin/sh\nexec /bin/bash --noprofile --norc\n")
        shell.chmod(0o755)
        environment = fixture_environment(
            base,
            XDG_CONFIG_HOME=str(base / "config"),
            XDG_DATA_HOME=str(base / "data"),
            SHELL=str(shell),
            GOROOT=str(Path(shutil.which("go")).resolve().parent.parent),
            # This stdlib-only fixture must use installed tools and private
            # caches, independently of user settings or network availability.
            GOTOOLCHAIN="local",
            GOPROXY="off",
            GOWORK="off",
            GOENV="off",
            GOFLAGS="",
            GOCACHE=str(base / "go-cache"),
            GOPATH=str(base / "go-path"),
        )
        toolchain = subprocess.run(
            ["go", "version"],
            cwd=root,
            env=environment,
            capture_output=True,
            text=True,
            timeout=15,
            check=True,
        )
        toolchain_log = json.dumps(
            {"version": toolchain.stdout.strip()}
            | {name: environment[name] for name in ("GOROOT", "GOTOOLCHAIN", "GOCACHE", "GOPROXY")},
            indent=2,
        )
        (ARTIFACTS / "toolchain.json").write_text(toolchain_log + "\n")
        print(toolchain_log, flush=True)
        environment["RODE_UI_TEST_STATE"] = str(base / "ui-state.json")
        log_path = ARTIFACTS / ("native.log")
        with log_path.open("w") as log:
            process = subprocess.Popen(
                [str(BINARY), str(root)], env=environment, stdout=log, stderr=log
            )
            window = None
            try:
                window = wait_for_window(process, log_path)
                wait_for_project(window, process, log_path, root)
                focus_window(window)
                ui = NativeUI(window, process, log_path, environment["RODE_UI_TEST_STATE"], root)
                ui.open_file("main.go")
                ui.command("Code: Format document")
                ui.control("source-editor-modified")
                key("s", ("Control_L",))
                wait_for(lambda: "func main() {" in go.read_text(), "Go formatting was not saved")
                screenshot(window, "go-highlight.png")
                ui.command("Code: Run current file / project")
                wait_for(
                    lambda: (root / "ran.ok").exists(), "Go Run did not execute in the project"
                )
                ui.command("Code: Build current file / project")
                wait_for(
                    lambda: (root / "rode-languages").exists(), "Go Build did not produce a binary"
                )
                screenshot(window, "go-run-build.png")
                ui.open_file("main.ts")
                ui.command("Code: Format document")
                ui.control("source-editor-modified")
                key("s", ("Control_L",))
                wait_for(
                    lambda: ts.read_text() == 'const value: string = "hello";\n',
                    "Prettier result was not saved",
                )
                screenshot(window, "typescript-highlight.png")
                # Focus the text editor before replacing the buffer.
                ui.click("source-editor")
                ui.control("source-editor", focused=True)
                key("a", ("Control_L",))
                text("const value = ;")
                ui.source("const value = ;")
                ui.command("Code: Check document")
                ui.command("Code: Problems")
                ui.control("problems-check-complete")
                screenshot(window, "typescript-problems.png")
                assert ts.read_text() == 'const value: string = "hello";\n', "Checking changed disk"
                # Saving invalid syntax must still complete without changing the buffer.
                ui.command("Save file")
                wait_for(
                    lambda: ts.read_text() == "const value = ;", "Syntax errors prevented saving"
                )
                close_window(window, process)
                print("Native Go highlighting/format/run/build and TS format/check passed.")
            except Exception:
                if window and process.poll() is None:
                    screenshot(window, "failure.png")
                raise
            finally:
                stop_process(process)


if __name__ == "__main__":
    run()
