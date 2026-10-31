"""One owned Jupyter kernel; JSON lines over the existing workspace process channel.

Standalone bridge launched on demand inside the project-host sandbox through process.rs.
Requires an explicitly installed jupyter_client and kernelspec; these are not file-helper
dependencies. A reader thread handles interrupts/shutdown while a bounded queue serializes
execution. stdin loss ends the owned kernel, and output carries execution ids for
source-version checks.
"""

import json
import os
import queue
import signal
import socket
import sys
import tempfile
import threading
import time

LIMIT = 256 * 1024
OUTPUT_LIMIT = 1024 * 1024
commands = queue.Queue(maxsize=4)
closed = threading.Event()
interrupt = threading.Event()
write_lock = threading.Lock()


def emit(kind, **data):
    """Write a bounded execution-tagged JSON notification under the shared output lock."""
    line = json.dumps(dict(kind=kind, **data), ensure_ascii=False, default=str)
    if len(line.encode("utf-8")) > 2 * 1024 * 1024:
        raise ValueError("Notebook response exceeds 2 MiB.")
    with write_lock:
        sys.stdout.write(line + "\n")
        sys.stdout.flush()


def read_commands():
    """Parse bounded stdin commands and keep interruption independent of the execution queue."""
    pending = bytearray()
    try:
        while not closed.is_set():
            data = os.read(sys.stdin.fileno(), 65536)
            if not data:
                break
            pending.extend(data)
            while b"\n" in pending:
                line, _, rest = pending.partition(b"\n")
                pending = bytearray(rest)
                if len(line) > LIMIT:
                    raise ValueError("Notebook command exceeds 256 KiB.")
                message = json.loads(line)
                if message.get("cmd") == "interrupt":
                    interrupt.set()
                elif message.get("cmd") == "shutdown":
                    closed.set()
                else:
                    commands.put_nowait(message)
            if len(pending) > LIMIT:
                raise ValueError("Notebook command exceeds 256 KiB.")
    except (OSError, ValueError, queue.Full):
        pass
    finally:
        closed.set()


def terminate(*_args):
    """End the bridge through normal finally-based kernel cleanup."""
    raise SystemExit(0)


def execute(manager, client, message):
    """Run one bounded code snapshot and translate only matching Jupyter messages into safe outputs."""
    code = message.get("code", "")
    request_id = message["id"]
    if not isinstance(code, str) or len(code.encode("utf-8")) > 128 * 1024:
        raise ValueError("Code cells are limited to 128 KiB per execution.")
    interrupt.clear()
    msg_id = client.execute(code, store_history=True, allow_stdin=False, stop_on_error=True)
    total, count, status = 0, None, "ok"
    clear_next = False
    while not closed.is_set():
        if interrupt.is_set():
            manager.interrupt_kernel()
            interrupt.clear()
        try:
            event = client.get_iopub_msg(timeout=0.1)
        except queue.Empty:
            if not manager.is_alive():
                raise ValueError("Notebook kernel exited.")
            continue
        if event.get("parent_header", {}).get("msg_id") != msg_id:
            continue
        kind, content = event["msg_type"], event["content"]
        if kind == "status" and content.get("execution_state") == "idle":
            break
        if kind == "execute_input":
            count = content.get("execution_count")
            emit("count", id=request_id, count=count)
        elif kind == "clear_output":
            if content.get("wait"):
                clear_next = True
            else:
                emit("clear", id=request_id)
        elif kind in ("stream", "execute_result", "display_data", "update_display_data", "error"):
            if kind == "stream":
                output = dict(
                    output_type="stream",
                    name=content.get("name", "stdout"),
                    text=content.get("text", ""),
                )
            elif kind == "error":
                status = "error"
                output = {
                    key: content.get(key, [] if key == "traceback" else "")
                    for key in ("ename", "evalue", "traceback")
                }
                output["output_type"] = "error"
            else:
                output = dict(
                    output_type="execute_result" if kind == "execute_result" else "display_data",
                    data=content.get("data", {}),
                    metadata=content.get("metadata", {}),
                )
                if kind == "execute_result":
                    output["execution_count"] = content.get("execution_count")
            size = len(json.dumps(output, ensure_ascii=False).encode("utf-8"))
            total += size
            if total > OUTPUT_LIMIT:
                emit(
                    "error",
                    id=request_id,
                    message="Cell output exceeded 1 MiB; the kernel was stopped. Earlier output is preserved.",
                )
                closed.set()
                break
            if clear_next:
                emit("clear", id=request_id)
                clear_next = False
            emit(
                "output",
                id=request_id,
                output=output,
                update=kind == "update_display_data",
                display_id=content.get("transient", {}).get("display_id"),
            )
    if closed.is_set():
        return
    # Consume this request's shell reply so errors/aborts stop Run All reliably.
    deadline = time.monotonic() + 3
    while time.monotonic() < deadline and not closed.is_set():
        try:
            reply = client.get_shell_msg(timeout=0.1)
        except queue.Empty:
            continue
        if reply.get("parent_header", {}).get("msg_id") == msg_id:
            status = reply["content"].get("status", status)
            count = reply["content"].get("execution_count", count)
            break
    emit("done", id=request_id, status=status, count=count)


def run():
    """Start one explicitly installed kernelspec with private IPC runtime data and scoped shutdown."""
    threading.Thread(target=read_commands, daemon=True).start()
    spec = commands.get(timeout=10)
    if spec.get("cmd") != "start":
        raise ValueError("Expected notebook kernel configuration.")
    try:
        import zmq
        from jupyter_client import KernelManager
    except ImportError as error:
        raise ValueError(
            "Install jupyter_client and a kernel (for Python: ipykernel) in the selected project-host interpreter. RODE does not install them automatically."
        ) from error
    manager, client = None, None
    # IPC sockets stay on the project host and work without shared networking.
    with tempfile.TemporaryDirectory(prefix="rode-notebook-") as runtime:
        try:
            context = zmq.Context()
            context.setsockopt(zmq.MAXMSGSIZE, 2 * 1024 * 1024)
            manager = KernelManager(
                kernel_name=spec.get("kernel", "python3"),
                transport="ipc",
                ip=os.path.join(runtime, "channel"),
                connection_file=os.path.join(runtime, "kernel.json"),
                context=context,
            )
            provisioner = manager.kernel_spec.metadata.get("kernel_provisioner", {}).get(
                "provisioner_name", "local-provisioner"
            )
            if provisioner != "local-provisioner":
                raise ValueError(
                    "Only local kernel provisioners on the project host are supported."
                )
            env = dict(
                os.environ,
                JUPYTER_RUNTIME_DIR=runtime,
                IPYTHONDIR=os.path.join(runtime, "ipython"),
                MPLCONFIGDIR=os.path.join(runtime, "matplotlib"),
            )
            if manager.kernel_spec.language == "python":
                env.setdefault("MPLBACKEND", "module://matplotlib_inline.backend_inline")
            manager.start_kernel(cwd=os.getcwd(), env=env, stdout=sys.stderr, stderr=sys.stderr)
            client = manager.blocking_client()
            client.start_channels()
            client.wait_for_ready(timeout=20)
            info_id = client.kernel_info()
            info = {}
            deadline = time.monotonic() + 5
            while time.monotonic() < deadline:
                try:
                    reply = client.get_shell_msg(timeout=0.1)
                except queue.Empty:
                    continue
                if reply.get("parent_header", {}).get("msg_id") == info_id:
                    info = reply.get("content", {}).get("language_info", {})
                    break
            emit(
                "ready",
                kernel=spec.get("kernel", "python3"),
                python=sys.executable,
                host=socket.gethostname(),
                slurm_job=os.environ.get("SLURM_JOB_ID"),
                language_info=info,
                pid=getattr(manager.provisioner, "pid", None),
            )
            while not closed.is_set():
                try:
                    message = commands.get(timeout=0.1)
                except queue.Empty:
                    if not manager.is_alive():
                        raise ValueError("Notebook kernel exited.")
                    continue
                if message.get("cmd") == "execute":
                    execute(manager, client, message)
        finally:
            if manager is not None:
                if manager.has_kernel:
                    manager.shutdown_kernel(now=True)
                manager.cleanup_resources()
            if client is not None:
                client.stop_channels()
            if manager is not None:
                context.destroy(linger=0)


if __name__ == "__main__":
    for sig in (signal.SIGTERM, signal.SIGHUP):
        signal.signal(sig, terminate)
    try:
        run()
    except Exception as error:
        emit("error", message=str(error)[:8192])
        sys.exit(1)
