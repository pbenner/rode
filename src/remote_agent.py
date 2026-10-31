"""Supervise one project-host stdio agent over its existing authenticated SSH channel.

Standalone program passed to the project-host interpreter by process.rs. The first JSON line
captures the root, literal command, actual sandbox prefix and environment; later lines feed
the owned child. stdin loss or termination stops only that child group, keeping the
independent OpenSSH master and project terminals intact.
"""

import errno
import json
import os
import shutil
import signal
import subprocess
import sys
import threading
import time


class Input:
    """Bounded byte reader that preserves lines already buffered after the launch request."""

    def __init__(self):
        """Create the pending input buffer without reading or starting a process."""
        self.pending = bytearray()

    def readline(self):
        """Return one bounded input line or remaining EOF bytes without unbounded readline allocation."""
        while True:
            end = self.pending.find(b"\n")
            if end >= 0:
                if end >= 16 * 1024 * 1024:
                    raise ValueError("Process request exceeds 16 MiB.")
                line = bytes(self.pending[: end + 1])
                del self.pending[: end + 1]
                return line
            if len(self.pending) > 16 * 1024 * 1024:
                raise ValueError("Process request exceeds 16 MiB.")
            data = os.read(sys.stdin.fileno(), 65536)
            if not data:
                result = bytes(self.pending)
                self.pending.clear()
                return result
            self.pending.extend(data)


def run():
    """Validate the captured launch request, supervise one child group and forward bounded input."""
    source = Input()
    line = source.readline()
    if len(line) > 256 * 1024 or not line.endswith(b"\n"):
        raise ValueError("Invalid agent launch request.")
    spec = json.loads(line)
    root = os.path.realpath(os.path.expanduser(spec["root"]))
    argv = spec["argv"]
    if not argv or len(argv) > 64 or sum(len(arg) for arg in argv) > 128 * 1024:
        raise ValueError("Invalid agent arguments.")
    program = os.path.expanduser(argv[0])
    if "/" in program:
        argv[0] = program if os.path.isabs(program) else os.path.join(root, program)
    else:
        local = os.path.expanduser("~/.local/bin/" + program)
        argv[0] = shutil.which(program) or (local if os.path.isfile(local) else program)
    env = dict(os.environ, NO_COLOR="1")
    python = spec.get("python", "auto")
    if python == "auto":
        python = next(
            (
                os.path.join(root, folder, "bin/python")
                for folder in (".venv", "venv", "env", ".env")
                if os.path.isfile(os.path.join(root, folder, "bin/python"))
            ),
            "python3",
        )
    if "/" in python or python.startswith("~"):
        python = os.path.expanduser(python)
        if not os.path.isabs(python):
            python = os.path.join(root, python)
        if os.path.isdir(python):
            python = os.path.join(python, "bin/python")
        env["PATH"] = os.path.dirname(python) + os.pathsep + env.get("PATH", "")
        prefix = os.path.dirname(os.path.dirname(python))
        if os.path.isfile(os.path.join(prefix, "pyvenv.cfg")):
            env["VIRTUAL_ENV"] = prefix
            env.pop("PYTHONHOME", None)
    if spec.get("python_program"):
        argv[0] = python if python not in ("auto", "off", "") else "python3"
    deadline = time.monotonic() + 1
    while True:
        try:
            child = subprocess.Popen(
                spec["prefix"] + argv,
                cwd=root,
                env=env,
                stdin=subprocess.PIPE,
                stdout=sys.stdout.buffer,
                stderr=sys.stderr.buffer,
                start_new_session=True,
            )
            break
        except OSError as error:
            if error.errno != errno.ETXTBSY or time.monotonic() >= deadline:
                raise
            time.sleep(0.01)
    closed = threading.Event()

    def stop(*_args):
        closed.set()
        try:
            os.killpg(child.pid, signal.SIGTERM)
        except ProcessLookupError:
            pass

    for sig in (signal.SIGTERM, signal.SIGHUP, signal.SIGINT):
        signal.signal(sig, stop)

    def forward():
        try:
            while True:
                data = source.readline()
                if not data:
                    break
                if len(data) > 16 * 1024 * 1024:
                    raise ValueError("Process request exceeds 16 MiB.")
                child.stdin.write(data)
                child.stdin.flush()
        except (BrokenPipeError, OSError, ValueError):
            pass
        finally:
            stop()

    threading.Thread(target=forward, daemon=True).start()
    while child.poll() is None:
        if closed.wait(0.1):
            try:
                child.wait(timeout=1)
            except subprocess.TimeoutExpired:
                os.killpg(child.pid, signal.SIGKILL)
            break
    code = child.wait()
    # Also clean up descendants after a natural exit of the agent.
    try:
        os.killpg(child.pid, signal.SIGKILL)
    except ProcessLookupError:
        pass
    return code


try:
    sys.exit(run())
except Exception as error:
    print("Cannot start remote agent: " + str(error), file=sys.stderr)
    sys.exit(1)
