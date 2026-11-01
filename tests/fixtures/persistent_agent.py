#!/usr/bin/python3
"""Deterministic agent work that requires no model, account or external service."""

import json
import os
import sys
import threading
from pathlib import Path

root = Path.cwd()
acp = "--acp" in sys.argv
lock = threading.Lock()
cancelled = threading.Event()
approved = threading.Event()
turn = 0
(root / "agent.pid").write_text(str(os.getpid()))
(root / "agent-scope.json").write_text(
    json.dumps(dict(cwd=str(root), private_visible=(root.parent / "private.txt").exists()))
)


def send(value):
    with lock:
        print(json.dumps(value), flush=True)


def reply(message, result):
    send(dict(jsonrpc="2.0", id=message["id"], result=result))


def notify(method, params):
    send(dict(jsonrpc="2.0", method=method, params=params))


def work(message, number, prompt):
    if "approval" in prompt:
        send(
            dict(
                jsonrpc="2.0",
                id="approval-one",
                method="session/request_permission"
                if acp
                else "item/commandExecution/requestApproval",
                params=dict(
                    sessionId="native-thread",
                    threadId="native-thread",
                    turnId=str(number),
                    command="fixture tool",
                    toolCall=dict(title="Fixture tool"),
                    options=[
                        dict(optionId="allow_once", name="Allow once", kind="allow_once"),
                        dict(optionId="reject_once", name="Reject", kind="reject_once"),
                    ],
                ),
            )
        )
        while not approved.wait(0.05):
            if cancelled.is_set():
                return
    (root / "work-started").write_text(str(number))
    while not (root / "continue").exists():
        if cancelled.wait(0.05):
            return
    if cancelled.is_set():
        return
    (root / "work-done").write_text(str(number))
    failed = number == 1 and (root / "fail-first").exists()
    text = f"background {'failed' if failed else 'complete'} {number}"
    if acp:
        notify(
            "session/update",
            dict(
                sessionId="native-thread",
                update=dict(
                    sessionUpdate="agent_message_chunk", content=dict(type="text", text=text)
                ),
            ),
        )
        reply(message, dict(stopReason="cancelled" if failed else "end_turn"))
    else:
        notify(
            "item/agentMessage/delta",
            dict(
                threadId="native-thread", turnId=str(number), itemId=f"answer-{number}", delta=text
            ),
        )
        notify(
            "turn/completed",
            dict(
                threadId="native-thread",
                turn=dict(id=str(number), status="failed" if failed else "completed"),
            ),
        )


for line in sys.stdin:
    message = json.loads(line)
    method = message.get("method", "")
    if message.get("id") == "approval-one" and "result" in message:
        result = message["result"]
        if (
            result.get("decision") in ("accept", "acceptForSession")
            or result.get("outcome", {}).get("optionId") == "allow_once"
        ):
            approved.set()
        else:
            (root / "denied").write_text("denied")
            cancelled.set()
    elif method == "initialize":
        reply(message, dict(protocolVersion=1, agentCapabilities={}) if acp else {})
    elif method == "model/list":
        reply(message, dict(data=[], nextCursor=None))
    elif method in ("thread/start", "session/new"):
        reply(
            message,
            dict(sessionId="native-thread") if acp else dict(thread=dict(id="native-thread")),
        )
    elif method in ("turn/start", "session/prompt"):
        if (root / "record-inputs").exists():
            with (root / "recorded-inputs.jsonl").open("a") as recorded:
                recorded.write(
                    json.dumps(message["params"].get("prompt", message["params"].get("input", [])))
                    + "\n"
                )
        turn += 1
        cancelled.clear()
        params = message["params"]
        prompt = " ".join(
            block.get("text", "") for block in params.get("prompt", params.get("input", []))
        )
        if not acp:
            reply(message, dict(turn=dict(id=str(turn))))
        threading.Thread(target=work, args=(message, turn, prompt), daemon=True).start()
    elif method in ("turn/interrupt", "session/cancel"):
        cancelled.set()
        (root / "cancelled").write_text("cancelled")
        if "id" in message:
            reply(message, {})
        if not acp:
            notify(
                "turn/completed",
                dict(threadId="native-thread", turn=dict(id=str(turn), status="interrupted")),
            )
    elif "id" in message:
        reply(message, {})
