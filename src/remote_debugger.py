"""DAP/PTY coordinator on the project host, inside its prepared sandbox.

Standalone peer of debugger/bridge.rs, launched inside the prepared project sandbox. JSON-line
coordinator messages wrap byte-counted DAP frames and base64 PTY data. Adapter and
runInTerminal targets share that namespace; cleanup tracks owned descendants, while debugger
attach detaches from an externally owned target.
"""

import base64
import ctypes
import errno
import json
import os
import pty
import queue
import re
import select
import signal
import struct
import subprocess
import sys
import termios
import threading
import time
from collections import deque
from pathlib import Path

LIMIT = 1024 * 1024
stop = threading.Event()
output = threading.Lock()


def emit(value):
    """Serialize one bounded coordinator notification under the stdout lock."""
    if stop.is_set():
        raise EOFError
    with output:
        sys.stdout.write(json.dumps(value, ensure_ascii=False) + "\n")
        sys.stdout.flush()


def read_dap(stream):
    """Read a bounded byte-counted DAP frame, returning None only for clean header EOF."""
    length, total = None, 0
    while True:
        line = stream.readline(8193)
        if not line:
            if total:
                raise ValueError("Truncated debugger header.")
            return None
        total += len(line)
        if total > 8192 or not line.isascii():
            raise ValueError("Invalid/oversized debugger header.")
        if line in (b"\r\n", b"\n"):
            break
        key, separator, value = line.partition(b":")
        if not separator:
            raise ValueError("Invalid debugger header.")
        if key.lower() == b"content-length":
            value = value.strip()
            if length is not None or not value.isdigit():
                raise ValueError("Invalid/duplicate debugger Content-Length.")
            length = int(value)
            if not 1 <= length <= LIMIT:
                raise ValueError("Debugger message exceeds 1 MiB.")
    if length is None:
        raise ValueError("Missing debugger Content-Length.")
    data = bytearray()
    while len(data) < length:
        block = stream.read(length - len(data))
        if not block:
            raise ValueError("Truncated debugger body.")
        data.extend(block)
    return json.loads(data)


def substitute(value, values, depth=0):
    """Expand supported placeholders in string values with bounded JSON nesting."""
    if depth > 24:
        raise ValueError("Debugger argument nesting exceeds its limit.")
    if isinstance(value, str):
        return re.sub(r"\{([^{}]*)\}", lambda m: values.get(m[1], m[0]), value)
    if isinstance(value, list):
        return [substitute(item, values, depth + 1) for item in value]
    if isinstance(value, dict):
        return {key: substitute(item, values, depth + 1) for key, item in value.items()}
    return value


def prepare(spec):
    """Resolve installed adapter/debuggee arguments and environment on the project host."""
    root = Path(spec["root"]).expanduser().resolve(strict=True)
    config = spec["config"]
    python = spec.get("python", "auto")
    if python == "auto":
        python = next(
            (
                str(root / folder / "bin/python")
                for folder in (".venv", "venv", "env", ".env")
                if (root / folder / "bin/python").is_file()
            ),
            "python3",
        )
    elif python in ("", "off"):
        python = "python3"
    elif "/" in python or python.startswith("~"):
        path = Path(python).expanduser()
        python = str(path if path.is_absolute() else root / path)
        if Path(python).is_dir():
            python = str(Path(python) / "bin/python")
    file = str(root / spec["file"]) if spec["file"] else ""
    values = dict(
        root=str(root),
        file=file,
        dir=str(Path(file).parent if file else root),
        stem=Path(file).stem,
        python=python,
    )
    backend = config["backend"]
    adapter = substitute(config["adapter"], values) or (
        python if backend == "python" else "lldb-dap" if backend == "lldb" else ""
    )
    if not adapter:
        raise ValueError("Choose a DAP adapter executable.")
    if "/" in adapter or adapter.startswith("~"):
        adapter = str(root / os.path.expanduser(adapter))
    argv = [adapter]
    if not config["adapter_arguments"] and backend == "python":
        argv += ["-m", "debugpy.adapter"]
    else:
        argv += substitute(config["adapter_arguments"], values)
    arguments = dict(
        cwd=str(root / os.path.expanduser(substitute(config["cwd"], values))),
        args=substitute(config["arguments"], values),
        stopOnEntry=config["stop_on_entry"],
    )
    program = substitute(config["program"], values)
    if program:
        arguments["program"] = str(root / os.path.expanduser(program))
    if backend == "python":
        arguments.update(
            python=[python], console="integratedTerminal", justMyCode=False, subProcess=False
        )
    elif backend == "lldb":
        arguments.update(console="integratedTerminal", runInTerminal=True, disableASLR=False)
    extra = substitute(config["extra"], values)
    if not isinstance(extra, dict):
        raise ValueError("Advanced debugger arguments must be an object.")
    arguments.update(extra)
    if not config["attach"] and not any(
        arguments.get(key) for key in ("program", "module", "launchCommands")
    ):
        raise ValueError("Choose a saved program/script or module before debugging.")
    env = dict(os.environ)
    if "/" in python:
        env["PATH"] = str(Path(python).parent) + os.pathsep + env.get("PATH", "")
        prefix = Path(python).parent.parent
        if (prefix / "pyvenv.cfg").is_file():
            env["VIRTUAL_ENV"] = str(prefix)
            env.pop("PYTHONHOME", None)
    return argv, arguments, env


def process(pid):
    """Read host process-parent/group information for scoped descendant cleanup."""
    try:
        fields = Path(f"/proc/{pid}/stat").read_text().rsplit(") ", 1)[1].split()
        return int(fields[1]), int(fields[19])
    except (OSError, ValueError, IndexError):
        return None


def cleanup():
    """Stop only tracked adapter/debuggee descendants, including detached owned children."""
    for sig in (signal.SIGTERM, signal.SIGKILL, signal.SIGKILL):
        records = []
        for index, path in enumerate(Path("/proc").iterdir()):
            if index >= 65536:
                break
            if path.name.isdigit() and (info := process(int(path.name))):
                records.append((int(path.name), *info))
        owned = {os.getpid()}
        for _ in range(128):
            before = len(owned)
            owned.update(pid for pid, parent, _ in records if parent in owned)
            if len(owned) == before:
                break
        for pid, _parent, stamp in reversed(records):
            if pid != os.getpid() and pid in owned and (info := process(pid)) and info[1] == stamp:
                try:
                    os.kill(pid, sig)
                except ProcessLookupError:
                    pass
        time.sleep(0.04)
        try:
            while os.waitpid(-1, os.WNOHANG)[0] > 0:
                pass
        except ChildProcessError:
            pass


def run():
    """Route DAP/control/PTY messages while supervising the single host-scoped debug session."""
    request = json.loads(sys.stdin.buffer.readline(256 * 1024 + 1))
    if request.get("op") != "start":
        raise ValueError("Expected a debugger launch.")
    spec = request["launch"]
    argv, arguments, env = prepare(spec)
    adapter = subprocess.Popen(
        argv,
        cwd=spec["root"],
        env=env,
        stdin=subprocess.PIPE,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        start_new_session=True,
    )
    incoming, writes = queue.Queue(maxsize=32), queue.Queue(maxsize=32)
    terminals = {}
    serial = 0
    pending = deque()

    def read_adapter():
        """Decode adapter DAP frames and publish messages, EOF or protocol errors to the coordinator."""
        try:
            while (message := read_dap(adapter.stdout)) is not None:
                incoming.put(("adapter", message))
            incoming.put(("adapter_eof", None))
        except (OSError, ValueError) as error:
            incoming.put(("error", str(error)))

    def read_errors():
        """Convert bounded adapter stderr lines into DAP output events; reject oversized lines."""
        while line := adapter.stderr.readline(8193):
            if len(line) > 8192:
                incoming.put(("error", "Adapter stderr line exceeds 8 KiB."))
                return
            incoming.put(
                (
                    "adapter",
                    dict(
                        type="event",
                        event="output",
                        body=dict(category="stderr", output=line.decode(errors="replace")),
                    ),
                )
            )

    def write_adapter():
        """Write queued JSON as byte-counted DAP frames without blocking the coordinator loop."""
        while not stop.is_set():
            try:
                message = writes.get(timeout=0.05)
            except queue.Empty:
                continue
            data = json.dumps(message, ensure_ascii=False).encode()
            if len(data) > LIMIT:
                incoming.put(("error", "Debugger message exceeds 1 MiB."))
                return
            try:
                adapter.stdin.write(f"Content-Length: {len(data)}\r\n\r\n".encode() + data)
                adapter.stdin.flush()
            except (OSError, BrokenPipeError) as error:
                incoming.put(("error", str(error)))
                return

    def read_client():
        """Read bounded client JSON lines and always publish EOF so owner cleanup can run."""
        try:
            while line := sys.stdin.buffer.readline(256 * 1024 + 1):
                if len(line) > 256 * 1024:
                    raise ValueError("Debugger request exceeds 256 KiB.")
                incoming.put(("client", json.loads(line)))
        except (OSError, ValueError) as error:
            incoming.put(("error", str(error)))
        finally:
            incoming.put(("eof", None))

    for target in (read_adapter, read_errors, write_adapter, read_client):
        threading.Thread(target=target, daemon=True).start()
    emit(
        dict(
            kind="dap",
            message=dict(type="event", event="rodeReady", body=dict(arguments=arguments)),
        )
    )

    def start_terminal(body, ident):
        """Validate literal runInTerminal arguments and start an owned PTY in the captured host environment."""
        if body.get("kind", "integrated") != "integrated" or body.get(
            "argsCanBeInterpretedByShell"
        ):
            raise ValueError("Only an integrated terminal with literal arguments is supported.")
        args = body.get("args", [])
        if not args or len(args) > 128 or len(json.dumps(body).encode()) > 128 * 1024:
            raise ValueError("Invalid debug terminal request.")
        child_env = dict(env)
        changes = body.get("env") or {}
        if len(changes) > 256:
            raise ValueError("Debug terminal environment exceeds its limit.")
        for key, value in changes.items():
            if not key or "=" in key or "\0" in key:
                raise ValueError("Invalid terminal environment.")
            if value is None:
                child_env.pop(key, None)
            else:
                child_env[key] = value
        master, slave = pty.openpty()
        try:
            # The child becomes its own session before this small exec wrapper.
            # Avoid preexec_fn in this multithreaded coordinator.
            bootstrap = "import os,sys,json,fcntl,termios; fcntl.ioctl(0,termios.TIOCSCTTY,0); a=json.loads(sys.argv[1]); os.execvpe(a[0],a,os.environ)"
            child = subprocess.Popen(
                [sys.executable, "-c", bootstrap, json.dumps(args)],
                cwd=body.get("cwd") or spec["root"],
                env=child_env,
                stdin=slave,
                stdout=slave,
                stderr=slave,
                start_new_session=True,
            )
        finally:
            os.close(slave)
        inputs = queue.Queue(maxsize=64)
        terminals[ident] = (master, child, inputs)

        def write_terminal():
            """Drain queued terminal input to its PTY master while the coordinator and child remain alive."""
            while not stop.is_set() and child.poll() is None:
                try:
                    data = inputs.get(timeout=0.05)
                except queue.Empty:
                    continue
                try:
                    while data:
                        data = data[os.write(master, data) :]
                except OSError:
                    return

        threading.Thread(target=write_terminal, daemon=True).start()
        emit(dict(kind="terminal", id=ident, title=body.get("title", "Debug program")[:120]))

        def read_terminal():
            """Forward PTY output as base64 events, reap the child and report its terminal identity on exit."""
            try:
                while not stop.is_set():
                    if not select.select([master], [], [], 0.05)[0]:
                        continue
                    data = os.read(master, 16384)
                    if not data:
                        break
                    emit(dict(kind="data", id=ident, data=base64.b64encode(data).decode()))
            except OSError as error:
                if error.errno not in (errno.EIO, errno.EBADF):
                    incoming.put(("error", str(error)))
            finally:
                child.wait()
                try:
                    emit(dict(kind="terminal_exited", id=ident))
                except EOFError:
                    pass

        threading.Thread(target=read_terminal, daemon=True).start()
        return dict(processId=child.pid, shellProcessId=child.pid)

    while not stop.is_set():
        while pending:
            try:
                writes.put_nowait(pending[0])
            except queue.Full:
                break
            pending.popleft()
        try:
            kind, message = incoming.get(timeout=0.05)
        except queue.Empty:
            continue
        if kind in ("eof", "adapter_eof"):
            break
        if kind == "error":
            raise ValueError(message)
        if kind == "client":
            operation = message.get("op")
            if operation == "shutdown":
                break
            if operation == "dap":
                if len(pending) >= 256:
                    raise ValueError("Too many queued debugger requests.")
                pending.append(message["message"])
            elif operation == "input":
                data = message["data"]
                if len(data) > 32768:
                    raise ValueError("Terminal input exceeds its limit.")
                if entry := terminals.get(message["id"]):
                    try:
                        entry[2].put_nowait(base64.b64decode(data, validate=True))
                    except queue.Full:
                        emit(
                            dict(
                                kind="dap",
                                message=dict(
                                    type="event",
                                    event="output",
                                    body=dict(
                                        category="stderr",
                                        output="Debug terminal input is full; some input was not sent. Retry after the program reads stdin.\n",
                                    ),
                                ),
                            )
                        )
            elif operation == "resize" and (entry := terminals.get(message["id"])):
                import fcntl

                fcntl.ioctl(
                    entry[0],
                    termios.TIOCSWINSZ,
                    struct.pack(
                        "HHHH",
                        min(500, max(1, message["rows"])),
                        min(1000, max(1, message["columns"])),
                        0,
                        0,
                    ),
                )
            elif operation == "end_terminal" and (entry := terminals.pop(message["id"], None)):
                if entry[1].poll() is None:
                    entry[1].terminate()
                os.close(entry[0])
        elif message.get("type") == "request":
            command = message.get("command")
            response = dict(seq=0, type="response", request_seq=message.get("seq"), command=command)
            try:
                if command != "runInTerminal" or len(terminals) >= 4:
                    raise ValueError("Unsupported debugger reverse request or too many terminals.")
                serial += 1
                response.update(success=True, body=start_terminal(message["arguments"], serial))
            except (OSError, ValueError) as error:
                response.update(success=False, message=str(error))
            emit(
                dict(
                    kind="dap",
                    message=dict(type="event", event="rodeReverseResponse", body=response),
                )
            )
        else:
            emit(dict(kind="dap", message=message))


try:
    ctypes.CDLL(None).prctl(36, 1, 0, 0, 0)  # PR_SET_CHILD_SUBREAPER
    for sig in (signal.SIGTERM, signal.SIGHUP, signal.SIGINT):
        signal.signal(sig, lambda *_: stop.set())
    run()
except (OSError, ValueError, queue.Full, EOFError) as error:
    print("Debugger: " + str(error), file=sys.stderr)
finally:
    stop.set()
    cleanup()
