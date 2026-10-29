"""On-demand project services, prepended to RODE's existing SSH helper.

Part of files::REMOTE_HELPER, sharing the interpreter globals of the concatenated peers.
Implements the project.rs request/reply shapes with bounded subprocesses. Tools run inside the
enclosing project sandbox; monitoring is explicitly dispatched on the host. Keep stdout
reserved for the final JSON reply.
"""

import json
import os
import shutil
import signal
import subprocess
import threading
import time


def project_run(args, root=None, text="", timeout=20, env=None, cancel=None):
    """Run literal arguments in an owned group with concurrent bounded pipe draining, timeout and cancellation."""
    limit = 8 * 1024 * 1024
    if len(text.encode("utf-8")) > limit:
        raise ValueError("Command input exceeds 8 MiB.")
    process = subprocess.Popen(
        args,
        cwd=root,
        env=env,
        stdin=subprocess.PIPE,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        start_new_session=True,
    )
    captured = [bytearray(), bytearray()]
    overflow = threading.Event()

    def drain(pipe, buffer):
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
        try:
            process.stdin.write(text.encode("utf-8"))
        except (BrokenPipeError, OSError):
            pass
        finally:
            process.stdin.close()

    readers = [
        threading.Thread(target=drain, args=(pipe, captured[i]), daemon=True)
        for i, pipe in enumerate([process.stdout, process.stderr])
    ]
    for thread in readers:
        thread.start()
    threading.Thread(target=write, daemon=True).start()
    start = time.monotonic()
    while process.poll() is None:
        if overflow.is_set() or time.monotonic() - start > timeout or (cancel and cancel.is_set()):
            os.killpg(process.pid, signal.SIGKILL)
            process.wait()
            raise ValueError("Command exceeded its time or 8 MiB output limit.")
        time.sleep(0.015)
    for thread in readers:
        thread.join(2)
        if thread.is_alive():
            try:
                os.killpg(process.pid, signal.SIGKILL)
            except ProcessLookupError:
                pass
            raise ValueError("Command output pipes did not close.")
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


def project_resolve(root, path):
    """Expand home and resolve relative host paths against the project root."""
    path = os.path.expanduser(path)
    return path if os.path.isabs(path) else os.path.join(root, path)


def project_python(root, value):
    """Resolve an explicit interpreter/environment or discover conventional project environments."""
    if value == "off":
        return ""
    if value == "auto":
        return next(
            (
                os.path.join(root, name, "bin/python")
                for name in (".venv", "venv", "env", ".env")
                if os.path.isfile(os.path.join(root, name, "bin/python"))
            ),
            "python3",
        )
    if not value.strip():
        return "python3"
    path = project_resolve(root, value) if "/" in value or value.startswith("~") else value
    return os.path.join(path, "bin/python") if os.path.isdir(path) else path


def project_environment(python):
    """Build the selected interpreter environment for a new command, without modifying existing shells."""
    env = dict(os.environ)
    if os.path.isabs(python):
        folder = os.path.dirname(python)
        env["PATH"] = folder + ":" + env.get("PATH", "/usr/local/bin:/usr/bin:/bin")
        prefix = os.path.dirname(folder)
        if os.path.isfile(os.path.join(prefix, "pyvenv.cfg")):
            env["VIRTUAL_ENV"] = prefix
    return env


def project_language(path):
    """Look up the embedded RODE_LANGUAGES catalogue by filename/extension."""
    name = os.path.basename(path)
    languages = globals()["RODE_LANGUAGES"]
    exact = next((item for item in languages if name in item["filenames"]), None)
    extension = name.rsplit(".", 1)[-1].lower() if "." in name else ""
    return exact or next((item for item in languages if extension in item["extensions"]), None)


def project_syntax_diagnostics(stderr, tool, source):
    """Normalize installed-tool parser errors to one-based character coordinates."""
    import re

    diagnostics = []
    for line in stderr.splitlines():
        clean = line.strip().removeprefix("[error] ")
        match = (
            re.search(r"^(.*?)\((\d+):(\d+)\)$", clean)
            if tool == "prettier"
            else re.search(r":(\d+):(\d+):\s*(.*)$", clean)
        )
        if not match:
            continue
        message, row, column = (
            match.groups() if tool == "prettier" else (match[3], match[1], match[2])
        )
        row, column = max(1, int(row)), max(1, int(column))
        diagnostics.append(
            dict(
                code=tool,
                message=message.strip(),
                row=row,
                column=column,
                end_row=row,
                end_column=column + 1,
                severity="error",
            )
        )
        if len(diagnostics) == 2000:
            break
    rows = {}
    for diagnostic in diagnostics:
        rows.setdefault(diagnostic["row"], []).append(diagnostic)
    for row, line in enumerate(source.splitlines(), 1):
        for diagnostic in rows.pop(row, []):
            units, column = 0, 1
            for char in line:
                if units >= diagnostic["column"] - 1:
                    break
                units += (
                    len(char.encode("utf-16-le")) // 2
                    if tool == "prettier"
                    else len(char.encode("utf-8"))
                )
                column += 1
            diagnostic["column"], diagnostic["end_column"] = column, column + 1
        if not rows:
            break
    return diagnostics


def project_analyze(request, root):
    """Analyze an unsaved source snapshot through stdin tools; never write the source file."""
    text = request["text"]
    if len(text.encode("utf-8")) > 8 * 1024 * 1024:
        raise ValueError("Buffer exceeds the tool limit.")
    result = dict(text=text, diagnostics=[], warnings=[])
    path = request["path"]
    language = project_language(path)
    if not language:
        raise ValueError("No code tools for this file type.")
    if language["id"] == "latex":
        if request["fix"]:
            raise ValueError("Ruff safe fixes apply only to Python files.")
        return globals()["latex_analyze"](
            dict(request, settings=globals()["latex_defaults"]()), root
        )
    if language["id"] != "python":
        if request["fix"]:
            raise ValueError("Ruff safe fixes apply only to Python files.")
        tool = language["formatter"]
        if not tool:
            raise ValueError(
                "No formatter for this language. Use the project terminal for other tools."
            )
        if not request["format"] and not language["check"]:
            raise ValueError(
                f"{tool} provides formatting only; use the project terminal for compiler/linter checks."
            )
        args = {
            "gofmt": ["-e"],
            "rustfmt": ["--emit", "stdout", "--edition", "2024"],
            "clang-format": ["--style=file", "--assume-filename", path],
            "prettier": ["--stdin-filepath", path],
            "shfmt": ["--filename", path],
        }[tool]
        executable = tool
        local = os.path.join(root, "node_modules/.bin", tool)
        if tool == "prettier" and os.path.isfile(local):
            executable = local
        try:
            output = project_run(
                [executable, *args], root, text, env=dict(os.environ, NO_COLOR="1")
            )
        except FileNotFoundError as error:
            raise ValueError(
                f"{tool} is unavailable. Install it on the project host and make it available on PATH."
            ) from error
        if not output["code"]:
            if request["format"]:
                result["text"] = output["stdout"]
        else:
            result["diagnostics"] = project_syntax_diagnostics(output["stderr"], tool, text)
            if not result["diagnostics"]:
                raise ValueError(f"{tool}: {output['stderr'].strip()}")
            if request["format"]:
                result["warnings"].append(
                    f"{tool} reported syntax errors; the buffer was kept unchanged."
                )
        return result
    python = project_python(root, request["python"])
    ruff = request["ruff"].strip()
    uvx = not ruff or ruff == "uvx ruff"
    if uvx:
        local_uvx = os.path.expanduser("~/.local/bin/uvx")
        program = shutil.which("uvx") or (
            local_uvx if os.path.isfile(local_uvx) and os.access(local_uvx, os.X_OK) else "uvx"
        )
        command = [program, "ruff"]
    elif ruff != "auto":
        command = [project_resolve(root, ruff) if "/" in ruff or ruff.startswith("~") else ruff]
    elif os.path.isfile(os.path.join(os.path.dirname(python), "ruff")):
        command = [os.path.join(os.path.dirname(python), "ruff")]
    else:
        try:
            probe = project_run(
                [
                    python,
                    "-c",
                    "import importlib.util;raise SystemExit(0 if importlib.util.find_spec('ruff') else 1)",
                ],
                root,
                timeout=5,
            )
            command = [python, "-m", "ruff"] if probe["code"] == 0 else ["ruff"]
        except OSError:
            command = ["ruff"]
    env = project_environment(python)
    env.pop("RUFF_OUTPUT_FILE", None)
    if uvx and "UV_TOOL_DIR" not in env:
        env["UV_TOOL_DIR"] = os.path.join(
            env.get("UV_CACHE_DIR", os.path.expanduser("~/.cache/uv")), "tools"
        )

    def execute(args, source):
        try:
            return project_run(command + args, root, source, env=env)
        except OSError as error:
            raise ValueError(
                "Ruff is unavailable. Install uv on the project host for uvx ruff, or choose auto / an explicit Ruff executable in Project settings: "
                + str(error)
            )

    if request["fix"]:
        result["text"] = project_checked(
            execute(
                [
                    "check",
                    "--fix",
                    "--no-unsafe-fixes",
                    "--exit-zero",
                    "--stdin-filename",
                    path,
                    "-",
                ],
                result["text"],
            )
        )
    if request["format"]:
        output = execute(["format", "--stdin-filename", path, "-"], result["text"])
        if output["code"] == 0:
            result["text"] = output["stdout"]
        elif request["check"]:
            result["warnings"].append(output["stderr"])
        else:
            raise ValueError(output["stderr"])
    if request["check"]:
        output = execute(
            ["check", "--no-fix", "--output-format=json", "--stdin-filename", path, "-"],
            result["text"],
        )
        if output["code"] not in (0, 1):
            raise ValueError(output["stderr"])
        for diagnostic in json.loads(output["stdout"])[:2000]:
            code = diagnostic.get("code") or "syntax"
            location = diagnostic.get("location", {})
            end = diagnostic.get("end_location", {})
            result["diagnostics"].append(
                dict(
                    code=code,
                    message=diagnostic.get("message", "Issue"),
                    row=location.get("row", 1),
                    column=location.get("column", 1),
                    end_row=end.get("row", 1),
                    end_column=end.get("column", 1),
                    severity="error"
                    if code in ("syntax", "invalid-syntax") or code.startswith(("E", "F"))
                    else "warning",
                )
            )
    return result


def project_read(path):
    """Read an optional host counter file, returning empty text when unavailable."""
    try:
        with open(path, encoding="utf-8") as stream:
            return stream.read(1024 * 1024)
    except OSError:
        return ""


def project_number(path):
    """Read an optional nonnegative host counter, preserving unavailable values as None."""
    try:
        return int(project_read(path).strip())
    except ValueError:
        return None


def project_monitor(root):
    """Collect a bounded host utilization snapshot without starting continuous monitoring."""
    result = dict(
        hostname=project_read("/proc/sys/kernel/hostname").strip(),
        cpus=[],
        memory_total=0,
        memory_available=0,
        swap_total=0,
        swap_free=0,
        load=[],
        uptime=0.0,
        net_received=0,
        net_sent=0,
        disk_total=0,
        disk_free=0,
        gpus=[],
        gpu_note="",
        processes=[],
    )
    for line in project_read("/proc/stat").splitlines():
        if line.startswith("cpu"):
            fields = line.split()
            values = [int(value) for value in fields[1:9]]
            result["cpus"].append(dict(name=fields[0], total=sum(values), idle=sum(values[3:5])))
    if not result["cpus"]:
        raise ValueError("Host monitoring currently requires Linux /proc.")
    memory = {
        line.split(":")[0]: int(line.split()[1]) * 1024
        for line in project_read("/proc/meminfo").splitlines()
    }
    for output, key in [
        ("memory_total", "MemTotal"),
        ("memory_available", "MemAvailable"),
        ("swap_total", "SwapTotal"),
        ("swap_free", "SwapFree"),
    ]:
        result[output] = memory.get(key, 0)
    result["load"] = [float(value) for value in project_read("/proc/loadavg").split()[:3]]
    result["uptime"] = float((project_read("/proc/uptime").split() or [0])[0])
    for line in project_read("/proc/net/dev").splitlines():
        if ":" in line:
            name, rest = line.split(":", 1)
            if name.strip() != "lo":
                fields = rest.split()
                result["net_received"] += int(fields[0])
                result["net_sent"] += int(fields[8])
    try:
        stats = os.statvfs(root)
        result.update(
            disk_total=stats.f_blocks * stats.f_frsize, disk_free=stats.f_bavail * stats.f_frsize
        )
    except OSError:
        pass

    def numeric(value):
        try:
            return float(value)
        except ValueError:
            return None

    try:
        output = project_run(
            [
                "nvidia-smi",
                "--query-gpu=index,name,utilization.gpu,memory.used,memory.total,temperature.gpu,power.draw",
                "--format=csv,noheader,nounits",
            ],
            timeout=3,
        )
        if output["code"] == 0:
            for line in output["stdout"].splitlines():
                fields = [field.strip() for field in line.split(",")]
                if len(fields) == 7:
                    used, total = numeric(fields[3]), numeric(fields[4])
                    result["gpus"].append(
                        dict(
                            name="GPU " + fields[0] + " · " + fields[1].strip('"'),
                            usage=numeric(fields[2]),
                            memory_used=int(used * 1048576) if used is not None else None,
                            memory_total=int(total * 1048576) if total is not None else None,
                            temperature=numeric(fields[5]),
                            power=numeric(fields[6]),
                        )
                    )
        else:
            result["gpu_note"] = output["stderr"].strip()[:256]
    except (OSError, ValueError):
        pass
    try:
        for card in sorted(os.listdir("/sys/class/drm")):
            if not card.startswith("card") or not card[4:].isdigit():
                continue
            device = "/sys/class/drm/" + card + "/device"
            vendor = project_read(device + "/vendor").strip()
            if vendor == "0x10de":
                continue
            brand = {"0x1002": "AMD", "0x8086": "Intel"}.get(vendor, "DRM")
            temperature = None
            try:
                for folder in os.listdir(device + "/hwmon"):
                    value = project_number(device + "/hwmon/" + folder + "/temp1_input")
                    if value is not None:
                        temperature = value / 1000.0
                        break
            except OSError:
                pass
            result["gpus"].append(
                dict(
                    name=brand + " " + card,
                    usage=project_number(device + "/gpu_busy_percent"),
                    memory_used=project_number(device + "/mem_info_vram_used"),
                    memory_total=project_number(device + "/mem_info_vram_total"),
                    temperature=temperature,
                    power=None,
                )
            )
    except OSError:
        pass
    try:
        output = project_run(["ps", "-eo", "pid,pcpu,pmem,comm", "--sort=-pcpu"], timeout=2)
        for line in output["stdout"].splitlines()[1:13]:
            fields = line.split(None, 3)
            if len(fields) == 4:
                result["processes"].append(dict(zip(["pid", "cpu", "memory", "name"], fields)))
    except (OSError, ValueError):
        pass
    return result


def project_handle(request):
    """Dispatch the project.rs service protocol; caller supplies the execution boundary."""
    if request["task"] == "extension":
        return extension_run(  # noqa: F821 - concatenated peer
            os.path.realpath(os.path.expanduser(request["root"])), request["invocation"]
        )
    task = request["task"]
    root = os.path.expanduser(request["root"])
    if task == "latex_analyze":
        return dict(kind="analysis", data=globals()["latex_analyze"](request, root))
    if task == "sessions":
        return dict(kind="sessions", data=globals()["native_sessions"](request))
    if task == "slurm":
        return globals()["slurm_service"](request, root)
    if task == "git":
        args = request["args"]
        if (
            not args
            or len(args) > 128
            or sum(len(value.encode("utf-8")) for value in args) > 64 * 1024
        ):
            raise ValueError("Invalid Git command size.")
        env = dict(
            os.environ,
            GIT_TERMINAL_PROMPT="0",
            GIT_EDITOR="true",
            GIT_SEQUENCE_EDITOR="true",
            LC_ALL="C.UTF-8",
        )
        data = project_run(
            [
                "git",
                "--no-pager",
                "--literal-pathspecs",
                "-c",
                "color.ui=false",
                "-c",
                "core.quotePath=false",
            ]
            + args,
            root,
            request.get("input") or "",
            timeout=120,
            env=env,
        )
        return dict(kind="command", data=data)
    if task == "monitor":
        return dict(kind="monitor", data=project_monitor(root))
    if task == "environments":
        candidates = [dict(name="System Python", python="")]
        for name in (".venv", "venv", "env", ".env"):
            path = os.path.join(root, name, "bin/python")
            if os.path.isfile(path):
                candidates.append(dict(name=name, python=path))
        python = project_python(root, request["selected"])
        data = json.loads(
            project_checked(
                project_run(
                    [
                        python,
                        "-c",
                        "import json,sys;print(json.dumps(dict(executable=sys.executable,version=sys.version.split()[0],prefix=sys.prefix,base_prefix=sys.base_prefix)))",
                    ],
                    root,
                    timeout=10,
                )
            )
        )
        data["candidates"] = candidates
        return dict(kind="environments", data=data)
    if task == "create_environment":
        root = os.path.realpath(root)
        path = project_resolve(root, request["path"])
        if os.path.exists(path):
            raise ValueError(
                "The environment destination already exists. Select it, or choose a new folder."
            )
        parent = os.path.realpath(os.path.dirname(path))
        if os.path.commonpath([root, parent]) != root:
            raise ValueError("Create environments inside the project folder.")
        path = os.path.join(parent, os.path.basename(path))
        try:
            os.mkdir(path)
        except FileExistsError:
            raise ValueError(
                "The environment destination already exists. Select it, or choose a new folder."
            ) from None
        command = [project_python(root, request["python"]), "-m", "venv", path]
        if not request.get("with_pip", True):
            command.append("--without-pip")
        output = project_run(command, root, timeout=120)
        return dict(kind="command", data=output)
    if task == "analyze":
        return dict(kind="analysis", data=project_analyze(request, root))
    raise ValueError("Unknown project service")
