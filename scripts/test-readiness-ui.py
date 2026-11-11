#!/usr/bin/env python3
"""Exercise native input acknowledgements while GUI frames are deliberately delayed."""

import os
import signal
import subprocess
import tempfile
import threading
from pathlib import Path

os.environ["RODE_UI_ARTIFACTS"] = str(
    Path(__file__).resolve().parent.parent / "target/readiness-ui"
)
from ui_harness import (
    ARTIFACTS,
    BINARY,
    NativeUI,
    close_window,
    focus_window,
    key,
    screenshot,
    stop_process,
    text,
    wait_for_project,
    wait_for_window,
)


def run():
    with tempfile.TemporaryDirectory(prefix="rode-readiness-ui-") as directory:
        base = Path(directory)
        project = base / "project"
        project.mkdir()
        first = project / "first.txt"
        first.write_text("initial\n")
        (project / "second.txt").write_text("other\n")
        state = ARTIFACTS / "state.json"
        environment = dict(
            os.environ,
            XDG_CONFIG_HOME=str(base / "config"),
            XDG_DATA_HOME=str(base / "data"),
            RODE_UI_TEST_STATE=str(state),
        )
        log_path = ARTIFACTS / "native.log"
        with log_path.open("w") as log:
            process = subprocess.Popen(
                [str(BINARY), str(project)], env=environment, stdout=log, stderr=log
            )
            # pidfds ensure a terminated fixture's PID can never target another process.
            try:
                pidfd = os.pidfd_open(process.pid)
            except OSError:
                stop_process(process)
                raise
            stopping = threading.Event()
            pauses = []
            controller_errors = []

            def send(kind):
                try:
                    signal.pidfd_send_signal(pidfd, kind)
                    return True
                except ProcessLookupError:
                    return False

            def delay_frames():
                try:
                    while not stopping.wait(0.15):
                        if not send(signal.SIGSTOP):
                            break
                        pauses.append(True)
                        stopping.wait(0.25)
                        send(signal.SIGCONT)
                except BaseException as error:
                    controller_errors.append(error)
                finally:
                    send(signal.SIGCONT)

            controller = threading.Thread(target=delay_frames)
            window = None
            try:
                window = wait_for_window(process, log_path)
                wait_for_project(window, process, log_path, project)
                focus_window(window)
                ui = NativeUI(window, process, log_path, state, project)
                controller.start()
                ui.open_file("first.txt")
                ui.control("source-editor", focused=True)
                before = ui.layout()["pass"]
                key("End", ("Control_L",))
                text("first line\nsecond line\n")
                expected = "initial\nfirst line\nsecond line\n"
                ui.source(expected, changed_after=before)
                # The final Enter above belongs to the editor, never the new palette.
                ui.open_file("second.txt")
                ui.source("other\n")
                ui.open_file("first.txt")
                ui.source(expected)
                key("s", ("Control_L",))
                ui.control("source-editor-saved")
                assert first.read_text() == expected
                stopping.set()
                controller.join(timeout=2)
                assert not controller.is_alive(), "Frame-delay controller did not stop"
                assert not controller_errors, controller_errors
                assert pauses, "No delayed frames were exercised"
                close_window(window, process)
                print(
                    f"{len(pauses)} controlled GUI pauses preserve complete paths, multiline edits and modal input order."
                )
            except Exception:
                if window is not None and process.poll() is None:
                    screenshot(window, "failure.png")
                raise
            finally:
                stopping.set()
                if controller.ident is not None:
                    controller.join(timeout=2)
                send(signal.SIGCONT)
                stop_process(process)
                os.close(pidfd)


if __name__ == "__main__":
    run()
