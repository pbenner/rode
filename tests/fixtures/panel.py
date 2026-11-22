"""Synthetic desktop panel provider for native integration tests; never contacts a service."""

import json
import os
import sys
from pathlib import Path

log = Path(sys.argv[1])
running = False
for line in sys.stdin:
    request = json.loads(line)
    action = request["action"]
    with log.open("a") as output:
        output.write(
            json.dumps(
                dict(
                    pid=os.getpid(),
                    action=action,
                    cwd=os.getcwd(),
                    filter=request["values"].get("filter", ""),
                    settings=request.get("settings", {}),
                )
            )
            + "\n"
        )
    if action == "start":
        running = True
    elif action == "stop":
        running = False
    view = dict(
        title="Demo dashboard",
        status="Monitoring" if running else "Ready",
        fields=[dict(key="filter", label="Filter", value=request["values"].get("filter", ""))],
        metrics=[dict(label="Completed", value="12 / 20", progress=0.6)],
        tables=[
            dict(
                title="Items",
                columns=["Name", "State"],
                rows=[[f"Example {index}", "Ready"] for index in range(512)],
            )
        ],
        actions=[
            dict(id="refresh", label="Refresh", enabled=True),
            dict(
                id="stop" if running else "start",
                label="Stop" if running else "Start",
                enabled=True,
            ),
            dict(
                id="commit",
                label="Commit test",
                enabled=True,
                primary=True,
                confirm="Commit this synthetic action?",
            ),
        ],
        refresh_seconds=30 if running else None,
    )
    print(json.dumps(dict(api=request["api"], id=request["id"], view=view)), flush=True)
