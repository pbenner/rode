"""LaTeX build/lint/format peer; standard-library only, on the project host.

Prepended to files::REMOTE_HELPER and also selected as a cancellable coordinator by the
transport. Mirrors latex.rs saved-source builds and stdin-only buffer tools. Shared
output-directory locking serializes compiler publication; cancellation removes only the owned
process tree, and compiler output never replaces source buffers.
"""


def latex_defaults():
    """Return serializable tool defaults matching latex::Settings."""
    return dict(
        program="latexmk",
        arguments=[
            "-norc",
            "-pdf",
            "-interaction=nonstopmode",
            "-halt-on-error",
            "-file-line-error",
            "-no-shell-escape",
            "-outdir={outdir}",
            "{file}",
        ],
        main_file="",
        output_directory="",
        pdf_file="",
        formatter="latexindent",
        checker="chktex",
        timeout_seconds=120,
    )


def latex_settings(settings):
    """Merge defaults and validate literal compiler/tool settings."""
    values = [
        settings[key]
        for key in ("program", "main_file", "output_directory", "pdf_file", "formatter", "checker")
    ]
    arguments = settings["arguments"]
    if not settings["program"].strip() or not 1 <= settings["timeout_seconds"] <= 300:
        raise ValueError("Choose a LaTeX executable and a timeout of 1–300 seconds.")
    if len(arguments) > 128 or sum(len(arg.encode()) for arg in arguments) > 32 * 1024:
        raise ValueError("LaTeX arguments exceed their limit.")
    if any(len(value.encode()) > 4096 or "\0" in value for value in values + arguments):
        raise ValueError("Invalid LaTeX executable, path or argument.")


def latex_main(root, path, settings):
    """Resolve explicit main source or a bounded root-comment chain, rejecting cycles."""
    from pathlib import Path

    def resolve(base, value):
        value = Path(value).expanduser()
        return value if value.is_absolute() else base / value

    if settings["main_file"].strip():
        return resolve(root, settings["main_file"]).resolve(strict=True)
    source, visited = resolve(root, path), set()
    for _ in range(8):
        source = source.resolve(strict=True)
        if source in visited:
            raise ValueError("Cycle in LaTeX root comments.")
        visited.add(source)
        with source.open("rb") as stream:
            header = stream.read(8192).decode("utf-8", errors="replace")
        target = None
        for line in header.splitlines()[:32]:
            line = line.strip()
            if not line.startswith("%"):
                continue
            key, separator, value = line[1:].strip().partition("=")
            if separator and key.strip().lower() == "!tex root":
                target = value.strip().strip('"').strip("'") or None
                if target:
                    break
        if not target:
            return source
        source = resolve(source.parent, target)
    raise ValueError("More than eight LaTeX root redirects.")


def latex_issues(output, directory, main):
    """Parse bounded compiler diagnostics into host paths and normalized source locations."""
    import re
    from pathlib import Path

    result, seen, pending = [], set(), None
    for line in output.splitlines():
        if len(result) == 2000:
            break
        line = line.strip()
        match = re.match(
            r"^(.*\.(?:tex|ltx|sty|cls|bib|dtx|ins)):(\d+):(?:(\d+):)?\s*(.*)$", line, re.I
        )
        if match:
            filename, row, column, message = match.groups()
            path = Path(filename).expanduser()
            if not path.is_absolute():
                path = directory / path
            path = str(path.resolve())
            row, column = max(1, int(row)), max(1, int(column or 1))
        elif line.startswith("! "):
            pending = line[2:]
            continue
        elif pending and (match := re.match(r"^l\.(\d+)\s", line)):
            path, row, column, message = str(main), max(1, int(match[1])), 1, pending
            pending = None
        else:
            continue
        message = message.strip()[:4096]
        key = path, row, message
        if key in seen:
            continue
        seen.add(key)
        result.append(
            dict(
                path=path,
                row=row,
                column=column,
                end_row=row,
                end_column=column + 1,
                message=message,
                code="LaTeX",
                severity="warning" if "warning" in message.lower() else "error",
            )
        )
    return result


def latex_analyze(request, root):
    """Format/check the submitted TeX snapshot using stdin tools without source writes."""
    import os

    settings, text = request["settings"], request["text"]
    latex_settings(settings)
    if len(text.encode()) > 8 * 1024 * 1024:
        raise ValueError("Buffer exceeds the tool limit.")
    source = globals()["project_resolve"](root, request["path"])
    directory = os.path.dirname(source)
    if not os.path.isdir(directory):
        directory = root
    result = dict(text=text, diagnostics=[], warnings=[])
    if request["format"] and settings["formatter"].strip():
        program = settings["formatter"]
        if "/" in program or program.startswith("~"):
            program = globals()["project_resolve"](root, program)
        try:
            result["text"] = globals()["project_checked"](
                globals()["project_run"]([program, "-g=/dev/null", "-l", "-"], directory, text)
            )
        except FileNotFoundError as error:
            raise ValueError(
                "latexindent is unavailable. Install it on the project host or configure the LaTeX formatter path."
            ) from error
    if request["check"] and settings["checker"].strip():
        program = settings["checker"]
        if "/" in program or program.startswith("~"):
            program = globals()["project_resolve"](root, program)
        try:
            output = globals()["project_run"](
                [program, "-q", "-I0", "-f", "%l:%c:%d:%k:%n:%m\n"],
                directory,
                result["text"],
                env=dict(os.environ, LC_ALL="C.UTF-8"),
            )
        except FileNotFoundError as error:
            message = "chktex is unavailable. Install it on the project host or configure the LaTeX checker path."
            if request["format"]:
                result["warnings"].append(message)
                return result
            raise ValueError(message) from error
        lines = result["text"].splitlines()
        for line in output["stdout"].splitlines():
            parts = line.split(":", 5)
            if len(parts) != 6 or not parts[0].isdigit() or not parts[1].isdigit():
                continue
            row, column = max(1, int(parts[0])), max(1, int(parts[1]))
            if row <= len(lines):
                units, adjusted = 0, 1
                for char in lines[row - 1]:
                    if units >= column - 1:
                        break
                    units += len(char.encode())
                    adjusted += 1
                column = adjusted
            result["diagnostics"].append(
                dict(
                    code="chktex " + parts[4],
                    message=parts[5].strip(),
                    row=row,
                    column=column,
                    end_row=row,
                    end_column=column + 1,
                    severity="error" if parts[3].lower() == "error" else "warning",
                )
            )
            if len(result["diagnostics"]) == 2000:
                break
        if output["code"] not in (0, 2, 3) or (output["code"] and not result["diagnostics"]):
            raise ValueError("ChkTeX failed: " + output["stderr"] + output["stdout"])
    return result


def latex_coordinator():
    """Run a cancellable saved-source build under the shared output-directory lock."""
    import fcntl
    import json
    import os
    import re
    import resource
    import signal
    import sys
    import threading
    from pathlib import Path

    request = json.loads(sys.stdin.buffer.readline(256 * 1024))
    settings = request["settings"]
    latex_settings(settings)
    cancelled = threading.Event()

    def stop(_signal, _frame):
        cancelled.set()

    signal.signal(signal.SIGHUP, stop)
    signal.signal(signal.SIGTERM, stop)

    def control():
        os.read(sys.stdin.fileno(), 1)
        cancelled.set()

    resource.setrlimit(resource.RLIMIT_FSIZE, (64 * 1024 * 1024, 64 * 1024 * 1024))
    threading.Thread(target=control, daemon=True).start()
    root = Path(request["root"]).expanduser().resolve(strict=True)
    source = latex_main(root, request["path"], settings)
    if not source.is_file() or source.stat().st_size > 8 * 1024 * 1024:
        raise ValueError("LaTeX sources must be regular files up to 8 MiB.")

    def resolve(base, value):
        path = Path(value).expanduser()
        return path if path.is_absolute() else base / path

    output = (
        resolve(root, settings["output_directory"])
        if settings["output_directory"].strip()
        else source.parent
    )
    output.mkdir(parents=True, exist_ok=True)
    output = output.resolve(strict=True)
    descriptor = os.open(output, os.O_RDONLY | os.O_DIRECTORY)
    try:
        try:
            fcntl.flock(descriptor, fcntl.LOCK_EX | fcntl.LOCK_NB)
        except BlockingIOError as error:
            raise ValueError(
                "Another RODE build is using this LaTeX output directory. Try again when it finishes."
            ) from error
        substitutions = {
            "file": str(source),
            "dir": str(source.parent),
            "root": str(root),
            "outdir": str(output),
        }
        args = [
            re.sub(r"\{(file|dir|root|outdir)\}", lambda match: substitutions[match[1]], arg)
            for arg in settings["arguments"]
        ]
        program = settings["program"]
        if "/" in program or program.startswith("~"):
            program = str(resolve(root, program))
        result = globals()["project_run"](
            [program, *args],
            str(source.parent),
            timeout=settings["timeout_seconds"],
            env=dict(os.environ, NO_COLOR="1", LC_ALL="C.UTF-8"),
            cancel=cancelled,
        )
        reply = dict(
            source=str(source),
            output=result,
            diagnostics=latex_issues(
                result["stdout"] + "\n" + result["stderr"], source.parent, source
            ),
            pdf=None,
            note="",
        )
        if not result["code"]:
            pdf = (
                resolve(output, settings["pdf_file"])
                if settings["pdf_file"].strip()
                else output / (source.stem + ".pdf")
            )
            try:
                if not pdf.is_file() or pdf.stat().st_size > 8 * 1024 * 1024:
                    raise ValueError(
                        "Generated PDF exceeds the 8 MiB preview limit or is not a regular file."
                    )
                with pdf.open("rb") as stream:
                    if stream.read(5) != b"%PDF-":
                        raise ValueError("Build output is not a PDF.")
                reply["pdf"] = str(pdf.resolve(strict=True))
            except (OSError, ValueError) as error:
                reply["note"] = f"No usable PDF at {pdf}: {error}"
        return reply
    finally:
        os.close(descriptor)
