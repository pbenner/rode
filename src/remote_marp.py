"""Bounded Marp snapshots through installed project-host tools; no server.

Prepended to files::REMOTE_HELPER and selected by the cancellable renderer coordinator.
Mirrors marp.rs snapshot/export contracts using installed host tools. Scratch follows the
project/encrypted context; source files stay untouched and an existing export needs explicit
overwrite authorization.
"""


def marp_coordinator():
    """Render one bounded source snapshot or atomically publish an explicitly authorized export."""
    import base64
    import hashlib
    import json
    import os
    import resource
    import signal
    import subprocess
    import sys
    import tempfile
    import threading
    import time
    from pathlib import Path

    request = json.loads(sys.stdin.buffer.readline(2 * 1024 * 1024))
    resource.setrlimit(resource.RLIMIT_FSIZE, (64 * 1024 * 1024, 64 * 1024 * 1024))
    cancelled = threading.Event()

    # SSH teardown may signal the command before its stdin reader sees EOF.
    # Keep the coordinator alive long enough to close Marp's detached browser.
    def stop(_signal, _frame):
        """Mark the presentation operation cancelled when a termination signal arrives."""
        cancelled.set()

    signal.signal(signal.SIGHUP, stop)
    signal.signal(signal.SIGTERM, stop)

    def control():
        """Cancel on control-channel input or EOF without retaining Python buffered-stdin locks."""
        os.read(sys.stdin.fileno(), 1)
        cancelled.set()

    threading.Thread(target=control, daemon=True).start()
    text = request["text"].encode("utf-8")
    if len(text) > 256 * 1024:
        raise ValueError("Marp supports source up to 256 KiB.")
    settings = request["settings"]
    for value in (settings["program"], settings["browser"], settings["theme_css"]):
        if len(value.encode("utf-8")) > 4096 or "\0" in value:
            raise ValueError("Invalid Marp executable or asset path.")
    browser_kind = settings.get("browser_kind", "auto")
    if browser_kind not in ("auto", "chrome", "edge", "firefox"):
        raise ValueError("Choose an available Marp browser type.")
    if browser_kind == "auto" and "firefox" in settings["browser"].lower():
        browser_kind = "firefox"
    root = Path(request["root"]).expanduser().resolve(strict=True)

    def resolve(value):
        """Expand the requested host path relative to the captured project root."""
        path = Path(value).expanduser()
        return path if path.is_absolute() else root / path

    source = resolve(request["path"])
    parent = source.parent.resolve(strict=True)
    source = parent / source.name
    output_format = request["format"]
    if output_format not in ("html", "pdf", "pptx"):
        raise ValueError("Unsupported Marp export format.")
    destination, expected = None, None
    if request.get("destination"):
        target = request["destination"]
        destination = resolve(target["path"]).resolve()
        if destination.suffix.lower() != "." + output_format:
            raise ValueError(f"Export filename must end with .{output_format}.")
        if destination == source.resolve():
            raise ValueError("Export cannot replace the Markdown source.")
        if destination.exists():
            if not target["overwrite"]:
                raise ValueError(
                    "Export already exists. Enable Replace existing export to replace this file."
                )
            expected = hashlib.sha256(read(str(destination))).hexdigest()  # noqa: F821 - shared safe file helper
    program = settings["program"].strip()
    if not program:
        local = root / "node_modules/.bin/marp"
        program = str(local) if local.is_file() else "marp"
    elif "/" in program or program.startswith("~"):
        program = str(resolve(program))
    scratch_parent = None
    # Encrypted source previews use RAM-backed scratch or the encrypted root.
    # No plaintext slide export is written into a normal host temporary folder.
    table = Path("/proc/self/mountinfo").read_text()
    mounts = []
    for line in table.splitlines():
        left, separator, right = line.partition(" - ")
        fields, tail = left.split(), right.split()
        if separator and len(fields) >= 5 and tail:
            import re

            point = Path(re.sub(r"\\([0-7]{3})", lambda match: chr(int(match[1], 8)), fields[4]))
            mounts.append((point, tail[0]))
    if any(
        kind == "fuse.gocryptfs" and (root == point or point in root.parents)
        for point, kind in mounts
    ):
        scratch_parent = root
        for candidate in (Path(tempfile.gettempdir()), Path("/dev/shm")):
            applicable = [
                (point, kind)
                for point, kind in mounts
                if candidate == point or point in candidate.parents
            ]
            if applicable and max(applicable, key=lambda item: len(str(item[0])))[1] == "tmpfs":
                scratch_parent = candidate
                break
    with tempfile.TemporaryDirectory(prefix=".rode-marp-", dir=scratch_parent) as directory:
        config = {"html": False}
        if output_format != "html":
            config["baseUrl"] = source.as_uri()
        config_path = Path(directory) / "marp.json"
        config_path.write_text(json.dumps(config), encoding="utf-8")
        output_path = Path(directory) / ("slides." + output_format)
        args = [
            program,
            "-",
            "--output",
            str(output_path),
            "--config-file",
            str(config_path),
            "--no-html",
            "--no-watch",
            "--no-server",
            "--no-preview",
            "--no-parallel",
            "--browser-timeout",
            "30",
        ]
        args += ["--browser", browser_kind]
        if output_format != "html":
            args.append("--" + output_format)
        if settings["allow_local_files"]:
            args.append("--allow-local-files")
        if settings["browser"].strip():
            args += ["--browser-path", str(resolve(settings["browser"]))]
        if settings["theme_css"].strip():
            args += ["--theme-set", str(resolve(settings["theme_css"]))]
        env = dict(os.environ, NO_COLOR="1", TMPDIR=directory, TEMP=directory, TMP=directory)

        try:
            process = subprocess.Popen(
                args,
                cwd=parent,
                env=env,
                stdin=subprocess.PIPE,
                stdout=subprocess.PIPE,
                stderr=subprocess.PIPE,
                start_new_session=True,
            )
        except OSError as error:
            raise ValueError(
                "Marp needs an installed Marp CLI on the project host. PDF/PPTX also need a supported browser. "
                + str(error)
            ) from error
        output = [bytearray(), bytearray()]
        overflow = threading.Event()

        def drain(pipe, result):
            """Drain one subprocess output pipe and flag the 8 MiB limit before retaining further bytes."""
            try:
                while data := pipe.read(65536):
                    result.extend(data)
                    if len(result) > 8 * 1024 * 1024:
                        overflow.set()
                        return
            finally:
                pipe.close()

        def feed():
            """Feed the captured source bytes to the renderer and close stdin even if it exits early."""
            try:
                process.stdin.write(text)
            except (BrokenPipeError, OSError):
                pass
            finally:
                process.stdin.close()

        readers = [
            threading.Thread(target=drain, args=(pipe, output[i]), daemon=True)
            for i, pipe in enumerate((process.stdout, process.stderr))
        ]
        for thread in readers:
            thread.start()
        threading.Thread(target=feed, daemon=True).start()
        started = time.monotonic()
        try:
            while process.poll() is None:
                if cancelled.is_set() or overflow.is_set() or time.monotonic() - started > 90:
                    raise ValueError(
                        "Marp conversion cancelled, timed out, or exceeded 8 MiB output."
                    )
                time.sleep(0.015)
            for thread in readers:
                thread.join(2)
                if thread.is_alive():
                    raise ValueError("Marp output pipes did not close.")
            if overflow.is_set():
                raise ValueError("Marp output exceeded 8 MiB.")
            if process.returncode:
                raise ValueError(
                    "Marp conversion failed: "
                    + output[1].decode("utf-8", errors="replace")
                    + output[0].decode("utf-8", errors="replace")
                )
        finally:
            try:
                os.killpg(process.pid, signal.SIGINT)
            except ProcessLookupError:
                pass
            try:
                process.wait(timeout=2)
            except subprocess.TimeoutExpired:
                pass
            try:
                os.killpg(process.pid, signal.SIGKILL)
            except ProcessLookupError:
                pass
            process.wait()
        if cancelled.is_set():
            raise ValueError("Marp conversion cancelled.")
        with output_path.open("rb") as stream:
            data = stream.read(8 * 1024 * 1024 + 1)
        if not data or len(data) > 8 * 1024 * 1024:
            raise ValueError("Marp output must be nonempty and at most 8 MiB.")
        if output_format == "pdf" and not data.startswith(b"%PDF-"):
            raise ValueError("Marp did not produce a PDF.")
        warnings = (
            output[1]
            .decode("utf-8", errors="replace")
            .encode("utf-8")[:65536]
            .decode("utf-8", errors="ignore")
        )
        if destination:
            write(str(destination), data, expected)  # noqa: F821 - shared locks and atomic revision checks
            return dict(data=None, path=str(destination), warnings=warnings)
        return dict(data=base64.b64encode(data).decode("ascii"), path=None, warnings=warnings)
