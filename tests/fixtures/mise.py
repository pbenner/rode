#!/usr/bin/env python3
"""Offline mise peer for host/policy/environment/task regressions; never downloads tools."""

import json
import os
import sys
from pathlib import Path

arguments = sys.argv[1:]
if arguments[:1] == ["-C"]:
    os.chdir(arguments[1])
    arguments = arguments[2:]
root = Path.cwd()
with (root / "mise-calls.jsonl").open("a") as log:
    log.write(json.dumps(dict(args=arguments, root=str(root))) + "\n")
if os.environ.get("MISE_AUTO_INSTALL") != "0":
    raise SystemExit("automatic installation was not disabled")
if os.environ.get("MISE_PARANOID") != "1":
    raise SystemExit("config trust checks were not enforced")
if arguments == ["--version"]:
    print("mise fixture")
elif arguments == ["trust"]:
    (root / "mise-trusted").touch()
elif not (root / "mise-trusted").exists():
    raise SystemExit("Project configuration is not trusted")
elif arguments == ["install"]:
    (root / "mise-installed").touch()
elif arguments == ["env", "--json"]:
    if (root / "mise-invalid").exists():
        print('{"INVALID": 42}')
    else:
        print(
            json.dumps(
                dict(
                    PATH=str(root / "mise-bin") + ":" + os.environ["PATH"],
                    RODE_MISE_VALUE="from-mise",
                    RODE_MISE_SECRET="do-not-store-or-print",
                )
            )
        )
elif arguments == ["env", "--shell", "bash"]:
    import shlex

    print(
        f'export RODE_MISE_VALUE=from-mise; export PATH={shlex.quote(str(root / "mise-bin"))}:"$PATH";'
    )
elif arguments == ["ls", "--current", "--json"]:
    print(
        json.dumps(
            dict(node=[dict(version="fixture", installed=not (root / "mise-missing").exists())])
        )
    )
elif arguments == ["tasks", "ls", "--local", "--json"]:
    print(json.dumps([dict(name="smoke", description="Write a task result")]))
elif arguments == ["run", "smoke"]:
    (root / "mise-task-result").write_text("task passed")
elif arguments[:2] == ["exec", "--"]:
    os.execvpe(arguments[2], arguments[2:], dict(os.environ, RODE_MISE_VALUE="from-mise"))
else:
    raise SystemExit("Unexpected mise arguments: " + repr(arguments))
