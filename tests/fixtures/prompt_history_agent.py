#!/usr/bin/python3
"""Read-only Codex history peer; any attempt to resume or run a turn fails."""

import json
import sys
from pathlib import Path

root = Path.cwd()
options = json.loads((root / "recall-source.json").read_text())
for line in sys.stdin:
    request = json.loads(line)
    with (root / "recall-rpc.jsonl").open("a") as log:
        log.write(json.dumps(request) + "\n")
    method = request.get("method")
    if method == "initialized":
        continue
    if method == "initialize":
        result = {}
    elif method == "thread/read":
        result = dict(
            thread=dict(
                id="live-thread",
                cwd=str(root.parent if options.get("wrong_root") else root),
                modelProvider="openai",
                status=dict(type="active"),
            )
        )
    elif method == "thread/turns/list":
        params = request["params"]
        assert params["threadId"] == "live-thread"
        assert params["limit"] == 1 and params["sortDirection"] == "desc"
        index = int(params.get("cursor", len(options["prompts"]))) - 1
        items = [
            dict(id=str(i), type="commandExecution", aggregatedOutput="saved tool")
            for i in range(150)
        ]
        items.append(
            dict(
                id=f"user-{index}",
                type="userMessage",
                content=[dict(type="text", text=options["prompts"][index])],
            )
        )
        result = dict(
            data=[dict(id=f"turn-{index}", items=items)], nextCursor=str(index) if index else None
        )
        if options.get("repeat_cursor"):
            result["nextCursor"] = str(len(options["prompts"]))
    else:
        raise AssertionError(f"Unsafe or unexpected history method: {method}")
    print(json.dumps(dict(id=request["id"], result=result)), flush=True)
