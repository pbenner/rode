#!/usr/bin/python3
"""Deterministic, offline app-server peer for the README's labelled Undo demo.

This is a recording fixture, not an agent implementation. It edits one setting
in the disposable signal-lab project and reports that real edit through Codex's
turn-diff protocol. It never contacts a model or invokes another executable.
"""

import difflib
import json
import sys
from pathlib import Path


def emit(value):
    """Write one complete protocol message, preserving native event order."""
    print(json.dumps(value), flush=True)


def notify(method, params):
    """Publish a notification on this fixture's one conversation."""
    emit(dict(jsonrpc="2.0", method=method, params=dict(threadId="tour-thread", **params)))


def edit():
    """Apply the single expected demo edit and return its complete unified diff."""
    path = Path("analysis.py")
    before = path.read_text()
    if before.count("learning_rate=0.05") != 1:
        raise ValueError("README demo source does not have the expected learning rate")
    after = before.replace("learning_rate=0.05", "learning_rate=0.06", 1)
    path.write_text(after)
    return "diff --git a/analysis.py b/analysis.py\n" + "".join(
        difflib.unified_diff(
            before.splitlines(True), after.splitlines(True), "a/analysis.py", "b/analysis.py"
        )
    )


def main():
    """Answer startup requests and one explicit, fully scripted editing prompt."""
    for line in sys.stdin:
        request = json.loads(line)
        if "id" not in request:
            continue
        method = request.get("method")
        result = {}
        if method in ("thread/start", "thread/resume"):
            result = dict(thread=dict(id="tour-thread"))
        elif method == "model/list":
            result = dict(data=[], nextCursor=None)
        elif method == "turn/start":
            result = dict(turn=dict(id="tour-turn"))
        emit(dict(jsonrpc="2.0", id=request["id"], result=result))
        if method == "turn/start":
            notify("turn/started", dict(turn=dict(id="tour-turn")))
            notify("turn/diff/updated", dict(turnId="tour-turn", diff=edit()))
            notify(
                "item/agentMessage/delta",
                dict(
                    turnId="tour-turn",
                    itemId="tour-answer",
                    delta=(
                        "Updated `analysis.py`: the default learning rate is now **0.06**, "
                        "previously **0.05**.\n\n"
                        "This scripted demonstration changes only that setting. "
                        "Use **Undo last edits** to restore **0.05** while keeping "
                        "your earlier **0.04 → 0.05** change."
                    ),
                ),
            )
            notify("turn/completed", dict(turn=dict(id="tour-turn", status="completed")))


if __name__ == "__main__":
    main()
