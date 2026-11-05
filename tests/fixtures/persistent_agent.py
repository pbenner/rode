#!/usr/bin/python3
"""Deterministic agent work that requires no model, account or external service."""

import json
import os
import sys
import threading
import time
from pathlib import Path

root = Path.cwd()
acp = "--acp" in sys.argv
lock = threading.Lock()
cancelled = threading.Event()
approved = threading.Event()
turn = 0


def snapshot(name, text):
    """Publish marker contents before a polling reader can observe the new file."""
    pending = root / f".{name}.{os.getpid()}.{threading.get_ident()}.tmp"
    try:
        pending.write_text(text)
        pending.replace(root / name)
    finally:
        pending.unlink(missing_ok=True)


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
    snapshot("work-started", str(number))
    while not (root / "continue").exists():
        if cancelled.wait(0.05):
            return
    if cancelled.is_set():
        return
    if prompt.startswith("overflow-") and not (root / "overflowed").exists():
        snapshot("overflowed", prompt)
        output = sys.stderr if prompt == "overflow-stderr" else sys.stdout
        limit = 1024 * 1024 if output is sys.stderr else 4 * 1024 * 1024
        output.write("x" * (limit + 1) + "\n")
        output.flush()
        return
    if prompt == "long-stderr":
        sys.stderr.write("d" * (1024 * 1024 - 1) + "\n")
        sys.stderr.flush()
    snapshot("work-done", str(number))
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
    if (root / "record-inputs").exists():
        with (root / "backend-rpc.jsonl").open("a") as recorded:
            recorded.write(json.dumps(message) + "\n")
    method = message.get("method", "")
    if message.get("id") == "approval-one" and "result" in message:
        result = message["result"]
        if (
            result.get("decision") in ("accept", "acceptForSession")
            or result.get("outcome", {}).get("optionId") == "allow_once"
        ):
            approved.set()
        else:
            snapshot("denied", "denied")
            cancelled.set()
    elif method == "initialize":
        deadline = time.monotonic() + 15
        while (root / "delay-prompt-bootstrap").exists() and time.monotonic() < deadline:
            time.sleep(0.02)
        reply(
            message,
            dict(
                protocolVersion=1,
                agentCapabilities={"loadSession": not (root / "no-resume").exists()},
            )
            if acp
            else {},
        )
    elif method == "model/list":
        reply(message, dict(data=[], nextCursor=None))
    elif method in ("thread/start", "session/new", "thread/resume", "session/load"):
        snapshot("agent.pid", str(os.getpid()))
        snapshot(
            "agent-scope.json",
            json.dumps(dict(cwd=str(root), private_visible=(root.parent / "private.txt").exists())),
        )
        if method == "session/load":
            notify(
                "session/update",
                dict(
                    sessionId="native-thread",
                    update=dict(
                        sessionUpdate="agent_message_chunk",
                        content=dict(
                            type="text", text="saved replay must not duplicate the local transcript"
                        ),
                    ),
                ),
            )
        identity = (
            "wrong-thread"
            if method == "thread/resume" and (root / "no-resume").exists()
            else "native-thread"
        )
        reply(
            message,
            dict(sessionId=identity) if acp else dict(thread=dict(id=identity)),
        )
    elif method == "thread/read":
        reply(
            message,
            dict(
                thread=dict(
                    id="native-thread",
                    cwd=str(root),
                    modelProvider="openai",
                    status=dict(type="active"),
                )
            ),
        )
    elif method == "thread/turns/list":
        saved = root / "saved-prompts.json"
        prompts = json.loads(saved.read_text()) if saved.exists() else []
        recorded = root / "recorded-inputs.jsonl"
        if recorded.exists():
            for line in recorded.read_text().splitlines():
                try:
                    blocks = json.loads(line)
                except json.JSONDecodeError:
                    continue
                prompts.append(blocks[0]["text"])
        index = int(message["params"].get("cursor", len(prompts))) - 1
        reply(
            message,
            dict(
                data=[
                    dict(
                        id=f"saved-{index}",
                        items=[
                            dict(
                                type="userMessage", content=[dict(type="text", text=prompts[index])]
                            )
                        ],
                    )
                ]
                if index >= 0
                else [],
                nextCursor=str(index) if index > 0 else None,
            ),
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
        snapshot("cancelled", "cancelled")
        if "id" in message:
            reply(message, {})
        if not acp:
            notify(
                "turn/completed",
                dict(threadId="native-thread", turn=dict(id=str(turn), status="interrupted")),
            )
    elif "id" in message:
        reply(message, {})
