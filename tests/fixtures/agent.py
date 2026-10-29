#!/usr/bin/env python3
"""Deterministic coding-agent peer; never calls a model or external service."""

import json
import os
import signal
import subprocess
import sys
from pathlib import Path

root = Path.cwd()
acp = False
counter = 0
pending = None
waiting = None
child = None


def metadata(name, text, append=False):
    try:
        with (root / name).open("a" if append else "w") as stream:
            stream.write(text)
    except OSError:
        pass


def emit(value):
    print(json.dumps(value), flush=True)


def response(request, result):
    emit({"jsonrpc": "2.0", "id": request["id"], "result": result})


def notify(method, params):
    emit({"jsonrpc": "2.0", "method": method, "params": params})


def reply(text):
    if acp:
        notify(
            "session/update",
            {
                "sessionId": "session-1",
                "update": {
                    "sessionUpdate": "agent_message_chunk",
                    "content": {"type": "text", "text": text},
                },
            },
        )
    else:
        notify(
            "item/agentMessage/delta",
            {"threadId": "thread-1", "itemId": f"reply-{counter}", "delta": text},
        )


def finish(text, interrupted=False):
    global waiting, child
    if child:
        child.terminate()
        child.wait()
        child = None
    reply(text)
    if acp:
        response(waiting, {"stopReason": "cancelled" if interrupted else "end_turn"})
    else:
        notify(
            "turn/completed",
            {
                "threadId": "thread-1",
                "turn": {
                    "id": f"turn-{counter}",
                    "status": "interrupted" if interrupted else "completed",
                },
            },
        )
    waiting = None


try:
    for line in sys.stdin:
        request = json.loads(line)
        metadata("agent-rpc.jsonl", json.dumps(request) + "\n", append=True)
        method = request.get("method")
        params = request.get("params", {})
        if method == "initialize":
            acp = "protocolVersion" in params
            response(
                request,
                {"protocolVersion": 1, "agentCapabilities": {}}
                if acp
                else {"userAgent": "fixture"},
            )
        elif method in ("thread/start", "session/new"):
            assert params["cwd"] == str(root), params
            metadata(
                "agent-scope.json",
                json.dumps(
                    {
                        "cwd": str(root),
                        "python": os.environ.get("VIRTUAL_ENV"),
                        "private_visible": (root.parent / "private.txt").exists(),
                        "argv": sys.argv[1:],
                    }
                ),
            )
            response(request, {"sessionId": "session-1"} if acp else {"thread": {"id": "thread-1"}})
        elif method in ("turn/start", "session/prompt"):
            counter += 1
            waiting = request
            content = params.get("prompt", params.get("input"))
            message = content[0]["text"]
            if not acp:
                response(request, {"turn": {"id": f"turn-{counter}"}})
            if message == "try-write":
                try:
                    (root / "blocked.txt").write_text("must be blocked in read-only mode")
                    finish("Write succeeded.")
                except OSError:
                    finish("Write blocked by the project sandbox.")
            elif message == "wait":
                child = subprocess.Popen([sys.executable, "-c", "import time; time.sleep(60)"])
                (root / "agent-child.pid").write_text(str(child.pid))
                reply("Working on a long task…")
            elif message == "approve":
                pending = "approval"
                emit(
                    {
                        "id": 400,
                        "method": "session/request_permission"
                        if acp
                        else "item/commandExecution/requestApproval",
                        "params": {
                            "sessionId": "session-1",
                            "threadId": "thread-1",
                            "turnId": f"turn-{counter}",
                            "itemId": "tool-1",
                            "command": "create approved.txt",
                            "toolCall": {"title": "Create approved.txt"},
                            "options": [
                                {"optionId": "yes", "name": "Allow once", "kind": "allow_once"},
                                {"optionId": "no", "name": "Decline", "kind": "reject_once"},
                            ],
                        },
                    }
                )
            elif message == "question":
                pending = "question"
                emit(
                    {
                        "id": 401,
                        "method": "item/tool/requestUserInput",
                        "params": {
                            "threadId": "thread-1",
                            "turnId": f"turn-{counter}",
                            "itemId": "question-1",
                            "isBlocking": True,
                            "questions": [
                                {
                                    "id": "test_mode",
                                    "header": "Tests",
                                    "question": "Which tests?",
                                    "options": [
                                        {"label": "All", "description": "Run the full suite"}
                                    ],
                                }
                            ],
                        },
                    }
                )
            elif message == "bad-protocol":
                print("this is not JSON-RPC", flush=True)
            elif message == "overflow":
                print("x" * (256 * 1024 + 10), flush=True)
            elif message == "exit":
                finish("Last reply before exit.")
                break
            else:
                if message == "rewrite":
                    (root / "main.py").write_text("# written by the agent\n")
                (root / "agent-created.txt").write_text(message)
                reply("Project agent response: ")
                finish(f"{message} (turn {counter}).\n\n```python\nprint('hello')\n```")
        elif method in ("turn/interrupt", "session/cancel"):
            if "id" in request:
                response(request, {})
            finish("Task stopped.", interrupted=True)
        elif method is None and pending:
            if pending == "approval":
                result = request["result"]
                if (
                    result.get("decision") == "cancel"
                    or result.get("outcome", {}).get("outcome") == "cancelled"
                ):
                    pending = None
                    continue
                allowed = (
                    result.get("decision") == "accept"
                    or result.get("outcome", {}).get("optionId") == "yes"
                )
                if allowed:
                    (root / "approved.txt").write_text("approved")
                finish("Approved." if allowed else "Declined.")
            else:
                (root / "answers.json").write_text(json.dumps(request["result"]))
                finish("Answer received.")
            pending = None
        elif method == "initialized":
            pass
        elif method:
            response(request, {})
finally:
    if child:
        try:
            os.kill(child.pid, signal.SIGTERM)
        except ProcessLookupError:
            pass
