#!/usr/bin/env python3
"""Scripted app-server peer: requests tools but never executes them or calls a model."""

import json
import os
import sys
from pathlib import Path

root = Path.cwd()
(root / "file-agent.pid").write_text(str(os.getpid()))
steps = json.loads((root / "file-tool-steps.json").read_text())
index = 0
turn = 0


def emit(value):
    print(json.dumps(value), flush=True)


def result(request, value):
    emit({"id": request["id"], "result": value})


def advance():
    global index
    if index == len(steps):
        emit(
            {
                "method": "turn/completed",
                "params": {
                    "threadId": "file-thread",
                    "turn": {"id": f"turn-{turn}", "status": "completed"},
                },
            }
        )
        return
    step = steps[index]
    params = dict(threadId="file-thread", turnId=f"turn-{turn}", callId=f"file-{index}")
    params.update(step.get("params", {}))
    emit({"id": 100 + index, "method": step.get("method", "item/tool/call"), "params": params})


for line in sys.stdin:
    request = json.loads(line)
    with (root / "file-agent-rpc.jsonl").open("a") as log:
        log.write(json.dumps(request) + "\n")
    method = request.get("method")
    if method == "initialize":
        result(request, {"userAgent": "file-only-fixture"})
    elif method == "config/read":
        features = {}
        for argument in sys.argv:
            if argument.startswith("features="):
                for item in argument.removeprefix("features={").removesuffix("}").split(","):
                    key, value = item.split("=", 1)
                    features[key] = json.loads(value)
        if (root / "ignore-shell-restriction").exists():
            features["shell_tool"] = True
        result(
            request,
            {
                "config": {
                    "features": features,
                    "mcp_servers": {"dangerous-server": {"enabled": True, "command": "sh"}},
                }
            },
        )
    elif method == "thread/start":
        assert request["params"]["sandbox"] == "read-only"
        assert request["params"]["approvalPolicy"] == "never"
        assert request["params"]["config"]["mcp_servers.dangerous-server.enabled"] is False
        assert len(request["params"]["dynamicTools"]) == 6
        result(request, {"thread": {"id": "file-thread"}})
    elif method == "turn/start":
        turn += 1
        index = 0
        result(request, {"turn": {"id": f"turn-{turn}"}})
        advance()
    elif method == "turn/interrupt":
        result(request, {})
        index = len(steps)
        advance()
    elif method == "model/list":
        result(request, {"data": [], "nextCursor": None})
    elif method == "initialized":
        pass
    elif method is None and request.get("id") == 100 + index:
        index += 1
        advance()
