"""Deploy/request the standalone native PTY helper over the existing SSH channel.

Prepended to files::REMOTE_HELPER. Native helper deployment is checksum/version checked, and
existing service generations are preserved during upgrades. Session shells belong to the host
service; closing an SSH channel only closes its view according to the explicit persistence
setting. Slurm management uses exact UID-filtered allocation identifiers.
"""


def native_sessions(request):
    """Verify/deploy the matching native helper and execute a policy-checked service operation."""
    import base64
    import hashlib
    import json
    import os
    import platform
    import shlex
    import stat
    import tempfile

    helper = request.get("helper")
    if not helper:
        raise ValueError(
            "The native rode-sessiond helper is required for remote persistent terminals."
        )
    digest = helper["digest"]
    if len(digest) != 64 or any(c not in "0123456789abcdef" for c in digest):
        raise ValueError("Invalid terminal helper digest")
    if platform.system() != "Linux" or platform.machine() != helper["architecture"]:
        raise ValueError("Choose a native session helper matching the remote Linux architecture.")
    runtime = request.get("directory")
    if runtime:
        os.makedirs(runtime, mode=0o700, exist_ok=True)
        runtime_meta = os.lstat(runtime)
        if (
            not stat.S_ISDIR(runtime_meta.st_mode)
            or runtime_meta.st_uid != os.geteuid()
            or runtime_meta.st_mode & 0o077
        ):
            raise ValueError("Terminal runtime directory must be owner-only.")
    directory = (
        os.path.join(runtime, "helpers")
        if runtime
        else os.path.expanduser("~/.local/share/rode/session-helpers")
    )
    os.makedirs(directory, mode=0o700, exist_ok=True)
    metadata = os.lstat(directory)
    if (
        not stat.S_ISDIR(metadata.st_mode)
        or metadata.st_uid != os.geteuid()
        or metadata.st_mode & 0o077
    ):
        raise ValueError("Session helper directory must be an owner-only directory.")
    executable = os.path.join(directory, "sessiond-" + digest)
    if not os.path.exists(executable):
        encoded = helper.get("data")
        if not encoded:
            raise ValueError(
                "Native terminal helper is missing; retry to deploy the packaged helper."
            )
        if len(encoded) > 32 * 1024 * 1024:
            raise ValueError("Session helper upload exceeds 24 MiB.")
        data = base64.b64decode(encoded, validate=True)
        if len(data) > 24 * 1024 * 1024 or hashlib.sha256(data).hexdigest() != digest:
            raise ValueError("Terminal helper upload failed its integrity check.")
        descriptor, temporary = tempfile.mkstemp(prefix=".sessiond-", dir=directory)
        try:
            with os.fdopen(descriptor, "wb") as output:
                output.write(data)
                output.flush()
                os.fsync(output.fileno())
                os.fchmod(output.fileno(), 0o700)
            os.replace(temporary, executable)
        finally:
            if os.path.exists(temporary):
                os.unlink(temporary)
    metadata = os.lstat(executable)
    if (
        not stat.S_ISREG(metadata.st_mode)
        or metadata.st_uid != os.geteuid()
        or metadata.st_mode & 0o077
    ):
        raise ValueError("Unsafe terminal helper file.")
    with open(executable, "rb") as source:
        if hashlib.sha256(source.read(24 * 1024 * 1024 + 1)).hexdigest() != digest:
            raise ValueError("Installed terminal helper failed its integrity check.")
    protocol = globals()["project_run"]([executable, "protocol"], timeout=5)
    if protocol["code"] != 0 or protocol["stdout"].strip() != "4":
        raise ValueError(
            "The packaged terminal helper uses an older protocol. Rebuild it with "
            "scripts/build-sessiond.sh or select an updated RODE_SESSION_HELPER. "
            "Existing terminal jobs were not touched."
        )
    operation = request["operation"]
    payload = dict(operation)
    if operation["action"] == "agents":
        import shutil

        payload = operation["request"]
        if payload["action"] not in ("agents", "create_agent", "end_agent"):
            raise ValueError("Only agent control requests are accepted here.")
        if payload["action"] == "create_agent":
            spec = payload["spec"]
            root, program = spec["plan"]["root"], spec["executable"]
            if "/" in program or program.startswith("~"):
                program = globals()["project_resolve"](root, program)
            else:
                local = os.path.expanduser("~/.local/bin/" + program)
                program = shutil.which(program) or (local if os.path.isfile(local) else program)
            spec["executable"] = program
            python = globals()["project_python"](root, spec["python"])
            spec["env"] = globals()["project_environment"](python)
    if operation["action"] == "create":
        plan = operation["plan"]
        argv = list(plan["prefix"]) if plan["active"] else []
        allocation = operation.get("allocation")
        argv += allocation["argv"] if allocation else [os.environ.get("SHELL", "/bin/sh"), "-l"]
        python = globals()["project_python"](plan["root"], operation["python"])
        env = globals()["project_environment"](python)
        initial_input = ""
        if allocation:
            env = {
                key: value for key, value in env.items() if not key.startswith(("SALLOC_", "SRUN_"))
            }
        if not allocation and os.path.isabs(python):
            folder, prefix = os.path.dirname(python), os.path.dirname(os.path.dirname(python))
            deactivate = "if command -v deactivate >/dev/null 2>&1; then deactivate; fi; "
            if os.path.isfile(os.path.join(prefix, "pyvenv.cfg")):
                activate = shlex.quote(os.path.join(prefix, "bin/activate"))
                initial_input = (
                    f"{deactivate}if [ -r {activate} ]; then . {activate}; else "
                    f'export VIRTUAL_ENV={shlex.quote(prefix)}; export PATH={shlex.quote(folder)}:"$PATH"; '
                    "unset PYTHONHOME; fi\n"
                )
            else:
                initial_input = f'{deactivate}unset VIRTUAL_ENV PYTHONHOME; export PATH={shlex.quote(folder)}:"$PATH"\n'
        payload = dict(
            action="create",
            id=operation["id"],
            name=operation["name"],
            root=plan["root"],
            policy=operation["policy"],
            mode=(
                "SANDBOX · bwrap · READ ONLY"
                if plan["active"] and plan["project_read_only"]
                else "SANDBOX · bwrap"
                if plan["active"]
                else "NORMAL MODE · sandbox unavailable"
                if plan["requested"]
                else "NORMAL MODE"
            ),
            argv=argv,
            initial_input=initial_input,
            env=env,
            persistent=operation["persistent"],
            allocation=allocation["job_name"] if allocation else None,
        )
    env = dict(os.environ)
    if request.get("directory"):
        env["RODE_SESSION_DIR"] = request["directory"]
    data = globals()["project_checked"](
        globals()["project_run"](
            [executable, "request"], text=json.dumps(payload), env=env, timeout=20
        )
    )
    response = json.loads(data)
    if response["kind"] == "error":
        raise ValueError(response["data"])
    # Environment is part of the attach invocation, not terminal shell commands.
    prefix = []
    if request.get("directory"):
        prefix = ["env", "RODE_SESSION_DIR=" + request["directory"], executable]
        executable = prefix.pop(0)
    return dict(response=response, executable=executable, prefix=prefix)


def slurm_service(request, root):
    """Query the effective UID's allocations or revalidate and cancel one exact allocation."""
    import os

    queue = request["queue"]
    cancel = request["cancel_program"]

    def program(value):
        return (
            globals()["project_resolve"](root, value)
            if "/" in value or value.startswith("~")
            else value
        )

    env = {
        key: value
        for key, value in os.environ.items()
        if not key.startswith(("SCANCEL_", "SQUEUE_")) and key != "SLURM_CLUSTERS"
    }
    args = [
        program(queue),
        "--noheader",
        "--array",
        "--local",
        "--user",
        str(os.geteuid()),
        "--Format",
        "JobID:0|,State:0|,TimeUsed:0|,TimeLeft:0|,NodeList:0|,tres-alloc:0|,Name:0",
    ]
    output = globals()["project_checked"](globals()["project_run"](args, root, env=env, timeout=15))
    jobs = []

    def valid_id(value):
        import re

        return bool(re.fullmatch(r"[0-9]{1,20}(?:_[0-9]{1,20})?(?:\+[0-9]{1,20})?", value))

    for line in output.splitlines():
        if not line.strip():
            continue
        fields = [value.strip() for value in line.split("|", 6)]
        if len(fields) != 7 or not valid_id(fields[0]):
            raise ValueError("Slurm returned an unsupported job record")
        generic, typed, malformed = None, 0, False
        for entry in fields[5].split(","):
            key, separator, value = entry.strip().partition("=")
            if separator and (key == "gres/gpu" or key.startswith("gres/gpu:")):
                try:
                    number = int(value)
                    if not value.isascii() or not value.isdecimal() or number > 2**64 - 1:
                        raise ValueError("Invalid GPU count")
                    if key == "gres/gpu":
                        generic = number
                    else:
                        typed += number
                        if typed > 2**64 - 1:
                            raise ValueError("GPU count overflow")
                except ValueError:
                    malformed = True
        gpus = (
            None
            if malformed or fields[5] in ("", "N/A", "(null)", "None")
            else generic
            if generic is not None
            else typed
        )
        jobs.append(
            dict(
                id=fields[0],
                state=fields[1],
                elapsed=fields[2],
                remaining=fields[3],
                nodes=fields[4],
                tres=fields[5],
                name=fields[6],
                gpus=gpus,
            )
        )
        if len(jobs) > 512:
            raise ValueError(
                "More than 512 Slurm jobs; use squeue in the terminal to inspect them all"
            )
    if request.get("cancel"):
        job = request["cancel"]
        if not valid_id(job) or not any(item["id"] == job for item in jobs):
            raise ValueError(
                "Allocation is no longer in your jobs on this host; refresh before releasing it"
            )
        return dict(
            kind="command",
            data=globals()["project_run"](
                [program(cancel), "--user", str(os.geteuid()), "--", job], root, env=env, timeout=20
            ),
        )
    return dict(kind="slurm", data=dict(jobs=jobs, warning=""))
