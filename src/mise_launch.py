"""Resolve the environment once before exec; stdin remains available for the target protocol."""

import json
import os
import signal
import sys
import threading

cancelled = threading.Event()
for handled_signal in (signal.SIGTERM, signal.SIGHUP, signal.SIGINT):
    signal.signal(handled_signal, lambda *_: cancelled.set())
try:
    specification = json.loads(sys.argv[1])
    command = sys.argv[2:]
    if not command or command.pop(0) != "--" or not command:
        raise ValueError("Missing mise launch command")
    root = os.path.realpath(os.path.expanduser(specification["root"]))
    environment = globals()["mise_environment"](
        root, specification["settings"], specification.get("python", ""), cancelled
    )
    if cancelled.is_set():
        raise ValueError("mise launch cancelled")
    os.chdir(root)
    os.execvpe(command[0], command, environment)
except Exception as error:
    # Never print the resolved values, which may contain project secrets.
    print(f"RODE mise: {error}", file=sys.stderr)
    sys.exit(1)
