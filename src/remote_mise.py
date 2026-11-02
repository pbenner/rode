"""Optional mise integration; resolution happens on the execution host, inside its policy."""

import json
import os
import shutil

# Populated by the concatenated peer with the same standalone launcher used by Rust.
RODE_MISE_LAUNCHER = ""


def mise_launch_prefix(root, settings, python=""):
    """Place provider resolution after the sandbox boundary and before the target command."""
    return [
        "python3",
        "-c",
        RODE_MISE_LAUNCHER,
        json.dumps(dict(root=root, settings=settings, python=python)),
        "--",
    ]


def mise_program(settings):
    """Find an explicitly configured executable or the host's installed user-local mise."""
    program = settings.get("runtimes", {}).get("mise") or "mise"
    if "/" in program:
        return program
    return shutil.which(program) or os.path.expanduser("~/.local/bin/" + program)


def mise_command(root, settings, arguments, timeout=15, cancel=None):
    """Run a bounded noninteractive mise command without implicit trust or installation."""
    globals()["project_environment_settings"](
        settings, dict(os.environ)
    )  # Validate before launching.
    env = dict(
        os.environ,
        MISE_AUTO_INSTALL="0",
        MISE_EXEC_AUTO_INSTALL="0",
        MISE_PARANOID="1",
        MISE_YES="0",
    )
    output = globals()["project_run"](
        [mise_program(settings)] + arguments,
        root=root,
        env=env,
        timeout=timeout,
        cancel=cancel,
    )
    if arguments == ["env", "--json"] and output["code"]:
        # Failed providers can leave partial values on stdout; never put those in a UI error.
        raise ValueError(output["stderr"][:8192] or "Cannot resolve the mise environment")
    return globals()["project_checked"](output)


def mise_environment(root, settings, python="", cancel=None):
    """Resolve fresh values without persisting them; explicit Rode values and Python win."""
    tools = json.loads(mise_command(root, settings, ["ls", "--current", "--json"], cancel=cancel))
    if not isinstance(tools, dict):
        raise ValueError("mise returned invalid tool metadata")
    missing = [
        name
        for name, versions in tools.items()
        if isinstance(versions, list)
        and any(
            isinstance(version, dict) and version.get("installed") is False for version in versions
        )
    ]
    if missing:
        raise ValueError(
            "Missing mise tools: " + ", ".join(missing) + ". Use Install declared tools."
        )
    values = json.loads(mise_command(root, settings, ["env", "--json"], cancel=cancel))
    if (
        not isinstance(values, dict)
        or len(values) > 2048
        or len(json.dumps(values).encode()) > 1024 * 1024
        or any(
            not isinstance(name, str)
            or not isinstance(value, str)
            or not name
            or "=" in name
            or "\0" in name + value
            for name, value in values.items()
        )
    ):
        raise ValueError("mise returned an invalid or oversized environment")
    env = dict(os.environ)
    env.update(values)
    env, _ = globals()["project_environment_settings"](settings, env)
    # An explicit interpreter or conventional project venv retains its existing precedence.
    if python == "auto":
        python = next(
            (
                os.path.join(root, folder, "bin/python")
                for folder in (".venv", "venv", "env", ".env")
                if os.path.isfile(os.path.join(root, folder, "bin/python"))
            ),
            "",
        )
    if python and python not in ("off", "auto") and ("/" in python or python.startswith("~")):
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
    return env


def mise_handle(request):
    """Inspect tools/tasks or perform an explicitly requested setup operation; omit env values."""
    root, settings, action = request["root"], request["settings"], request["action"]
    if action == "trust":
        return {"output": mise_command(root, settings, ["trust"], timeout=30)[:65536]}
    if action == "install":
        return {"output": mise_command(root, settings, ["install"], timeout=120)[:65536]}
    if action != "inspect":
        raise ValueError("Unknown mise operation")
    version = mise_command(root, settings, ["--version"]).strip()
    tools = json.loads(mise_command(root, settings, ["ls", "--current", "--json"]))
    tasks = json.loads(mise_command(root, settings, ["tasks", "ls", "--local", "--json"]))
    if not isinstance(tools, dict) or not isinstance(tasks, list) or len(tasks) > 256:
        raise ValueError("Invalid mise tool/task metadata")
    return dict(
        version=version[:256],
        tools=[str(name)[:128] for name in list(tools)[:128]],
        tasks=[
            dict(name=task["name"], description=str(task.get("description") or "")[:1024])
            for task in tasks
            if isinstance(task, dict)
            and isinstance(task.get("name"), str)
            and 0 < len(task["name"]) <= 256
            and not task["name"].startswith("-")
            and not any(ord(char) < 32 for char in task["name"])
        ],
        output="",
    )
