"""Old RODE wire protocol with a real shell, used only in private test runtimes."""

import json
import os
import signal
import socket
import struct
import subprocess
import sys
import threading
from pathlib import Path

runtime, root, identifier, policy = sys.argv[1:]
Path(runtime).mkdir(mode=0o700)
master, slave = os.openpty()
shell = subprocess.Popen(
    ["/bin/sh"], stdin=slave, stdout=slave, stderr=slave, cwd=root, start_new_session=True
)
os.close(slave)
lock = threading.RLock()
viewers = set()
history = bytearray()
stopping = threading.Event()
name = "Old service terminal"


def frame(stream, kind, payload):
    stream.sendall(struct.pack(">I", len(payload) + 1) + kind + payload)


def reply(stream, kind, data=None):
    frame(stream, b"J", json.dumps(dict(kind=kind, data=data)).encode())


def read_exact(stream, count):
    result = bytearray()
    while len(result) < count:
        part = stream.recv(count - len(result))
        if not part:
            raise EOFError
        result.extend(part)
    return result


def receive(stream):
    size = struct.unpack(">I", read_exact(stream, 4))[0]
    if not 0 < size <= 1024 * 1024:
        raise ValueError("Invalid frame")
    value = read_exact(stream, size)
    return value[:1], value[1:]


def info():
    return dict(
        id=identifier,
        name=name,
        root=root,
        policy=policy,
        mode="NORMAL MODE",
        alive=shell.poll() is None,
        clients=len(viewers),
        created=1,
        exit_code=shell.poll(),
    )


def output():
    while not stopping.is_set():
        try:
            data = os.read(master, 8192)
        except OSError:
            return
        if not data:
            return
        with lock:
            history.extend(data)
            del history[: max(0, len(history) - 512 * 1024)]
            for viewer in list(viewers):
                try:
                    frame(viewer, b"O", data)
                except OSError:
                    viewers.discard(viewer)


def handle(stream):
    global name
    try:
        kind, data = receive(stream)
        assert kind == b"J"
        request = json.loads(data)
        action = request["action"]
        with lock:
            with (Path(runtime) / "requests").open("a") as log:
                log.write(action + "\n")
            if action == "list":
                reply(stream, "sessions", [info()])
            elif action not in ("attach", "rename", "end", "shutdown", "create"):
                reply(
                    stream,
                    "error",
                    f"unknown variant `{action}`, expected one of `list`, `create`, "
                    "`attach`, `rename`, `end`, `shutdown` at line 1 column 27",
                )
            elif action == "shutdown":
                reply(stream, "error", "End running terminals before stopping the service")
            elif request.get("id") != identifier or request.get("policy") != policy:
                reply(stream, "error", "Wrong session or policy")
            elif action == "attach":
                reply(stream, "session", info())
                frame(stream, b"S", struct.pack(">HH", 24, 100))
                frame(stream, b"O", history)
                viewers.add(stream)
            elif action == "rename":
                name = request["name"]
                reply(stream, "session", info())
            elif action == "end":
                reply(stream, "ok")
                stopping.set()
            else:
                reply(stream, "error", "New work must use the compatible service")
        if action == "attach":
            while not stopping.is_set():
                kind, data = receive(stream)
                if kind == b"I":
                    with (Path(runtime) / "input").open("a") as log:
                        log.write(repr(bytes(data)) + "\n")
                    os.write(master, data)
                elif kind == b"D":
                    break
    except (OSError, EOFError):
        pass
    finally:
        with lock:
            viewers.discard(stream)
        stream.close()


def stop(_signum, _frame):
    stopping.set()


signal.signal(signal.SIGTERM, stop)
signal.signal(signal.SIGINT, stop)
threading.Thread(target=output, daemon=True).start()
listener = socket.socket(socket.AF_UNIX)
listener.bind(str(Path(runtime) / "socket"))
os.chmod(Path(runtime) / "socket", 0o600)
listener.listen()
listener.settimeout(0.2)
try:
    while not stopping.is_set():
        try:
            connection, _ = listener.accept()
        except TimeoutError:
            continue
        threading.Thread(target=handle, args=(connection,), daemon=True).start()
finally:
    listener.close()
    (Path(runtime) / "socket").unlink(missing_ok=True)
    if shell.poll() is None:
        os.killpg(shell.pid, signal.SIGKILL)
    shell.wait(timeout=5)
    os.close(master)
