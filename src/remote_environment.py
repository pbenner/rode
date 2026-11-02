"""Bounded host commands and literal environment overlays shared by project and mise helpers."""

import errno
import json
import os
import signal
import subprocess
import threading
import time


def project_environment_settings(settings, env):
    """Apply validated literal values and runtime directories on this execution host."""
    import re
    import shlex

    variables = settings.get("variables", {})
    runtimes = settings.get("runtimes", {})
    if not isinstance(settings.get("mise", False), bool):
        raise ValueError("Invalid mise opt-in.")
    if len(variables) > 64 or len(runtimes) > 32 or len(json.dumps(settings).encode()) > 65536:
        raise ValueError("Too many project environment settings.")
    exports = []
    for name, value in variables.items():
        if not re.fullmatch(r"[A-Za-z_][A-Za-z_0-9]{0,127}", name):
            raise ValueError("Invalid environment variable name.")
        if len(value.encode()) > 4096 or any(c in value for c in "\0\n\r"):
            raise ValueError("Invalid environment variable value.")
        env[name] = value
        exports.append(f"export {name}={shlex.quote(value)};")
    directories = []
    if runtimes.get("rustc") and "RUSTC" not in variables:
        env["RUSTC"] = runtimes["rustc"]
    if runtimes.get("rustc") and "RUSTC" not in variables:
        exports.append(f"export RUSTC={shlex.quote(runtimes['rustc'])};")
    for name, value in sorted(runtimes.items()):
        if (
            not re.fullmatch(r"[A-Za-z_0-9-]{1,64}", name)
            or len(value.encode()) > 4096
            or any(ord(c) < 32 or ord(c) == 127 for c in value)
        ):
            raise ValueError("Invalid runtime executable.")
        if "/" in value and not os.path.isabs(value):
            raise ValueError("Use an absolute host path for a runtime executable.")
        if os.path.isabs(value):
            directory = os.path.dirname(value)
            if directory not in directories:
                directories.append(directory)
    if directories:
        prefix = ":".join(directories)
        env["PATH"] = prefix + ":" + env.get("PATH", "")
        exports.append(f'export PATH={shlex.quote(prefix)}:"$PATH";')
    return env, " ".join(exports)


def project_run(args, root=None, text="", timeout=20, env=None, cancel=None):
    """Run literal arguments in an owned group with concurrent bounded pipe draining, timeout and cancellation."""
    limit = 8 * 1024 * 1024
    if len(text.encode("utf-8")) > limit:
        raise ValueError("Command input exceeds 8 MiB.")
    spawn_until = time.monotonic() + min(timeout, 1)
    while True:
        if cancel and cancel.is_set():
            raise ValueError("Command cancelled before startup.")
        try:
            process = subprocess.Popen(
                args,
                cwd=root,
                env=env,
                stdin=subprocess.PIPE,
                stdout=subprocess.PIPE,
                stderr=subprocess.PIPE,
                start_new_session=True,
            )
            break
        except OSError as error:
            if error.errno != errno.ETXTBSY or time.monotonic() >= spawn_until:
                raise
            time.sleep(0.01)
    captured = [bytearray(), bytearray()]
    overflow = threading.Event()

    def drain(pipe, buffer):
        """Drain one output stream concurrently, flagging overflow beyond the shared command byte limit."""
        try:
            while True:
                data = pipe.read(65536)
                if not data:
                    return
                buffer.extend(data)
                if len(buffer) > limit:
                    overflow.set()
                    return
        finally:
            pipe.close()

    def write():
        """Feed the captured UTF-8 source and close stdin even when the child stops reading."""
        try:
            process.stdin.write(text.encode("utf-8"))
        except (BrokenPipeError, OSError):
            pass
        finally:
            try:
                process.stdin.close()
            except (BrokenPipeError, OSError):
                pass

    workers = [
        threading.Thread(target=drain, args=(pipe, captured[i]), daemon=True)
        for i, pipe in enumerate([process.stdout, process.stderr])
    ]
    workers.append(threading.Thread(target=write, daemon=True))
    for thread in workers:
        thread.start()
    start = time.monotonic()
    while process.poll() is None:
        if overflow.is_set() or time.monotonic() - start > timeout or (cancel and cancel.is_set()):
            os.killpg(process.pid, signal.SIGKILL)
            process.wait()
            raise ValueError("Command exceeded its time or 8 MiB output limit.")
        time.sleep(0.015)
    for thread in workers:
        thread.join(2)
        if thread.is_alive():
            try:
                os.killpg(process.pid, signal.SIGKILL)
            except ProcessLookupError:
                pass
            raise ValueError("Command pipes did not close.")
    if overflow.is_set():
        raise ValueError("Command output exceeded 8 MiB.")
    return dict(
        code=process.returncode,
        stdout=captured[0].decode("utf-8"),
        stderr=captured[1].decode("utf-8"),
    )


def project_checked(output):
    """Return successful stdout or raise a bounded combined-output failure."""
    if output["code"]:
        raise ValueError(output["stderr"] + output["stdout"])
    return output["stdout"]
