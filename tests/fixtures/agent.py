#!/usr/bin/env python3
"""Deterministic coding-agent peer; never calls a model or external service."""

import json
import os
import signal
import subprocess
import sys
import time
from pathlib import Path

root = Path.cwd()
acp = False
thread_id = "thread-1"
session_id = "session-1"
counter = 0
pending = None
waiting = None
child = None
option_values = {"model": "fixture-fast", "mode": "ask", "effort": "low", "automatic": False}


def history_options():
    path = root / "history-fixture.json"
    return json.loads(path.read_text()) if path.exists() else {}


def deleted_history():
    path = root / "history-deleted.json"
    return set(json.loads(path.read_text())) if path.exists() else set()


def saved(identity):
    """Agent-owned history, with a deliberately foreign row to check client isolation."""
    options = history_options()
    title = options.get("titles", {}).get(identity, "Saved conversation " + identity)
    return {
        "id": identity,
        "sessionId": identity,
        "cwd": str(root),
        "modelProvider": "rode_local"
        if any("model_providers.rode_local" in a for a in sys.argv)
        else "openai",
        "name": title,
        "title": title,
        "preview": options.get("previews", {}).get(identity, "Earlier work on this project"),
        "updatedAt": options.get("updated", {}).get(identity, 1700000000),
        "status": {"type": "idle"},
    }


def controls():
    """Advertise dependent model/effort choices and a permission mode without inference."""
    options = []
    for name, category, values in [
        ("model", "model", ["fixture-fast", "fixture-deep"]),
        ("mode", "mode", ["ask", "code"]),
        (
            "effort",
            "thought_level",
            ["low", "high"] if option_values["model"] == "fixture-deep" else ["low"],
        ),
    ]:
        options.append(
            {
                "id": name,
                "name": name.title(),
                "category": category,
                "type": "select",
                "currentValue": option_values[name],
                "options": [{"value": value, "name": value} for value in values],
            }
        )
    options.append(
        {
            "id": "automatic",
            "name": "Automatic mode",
            "category": "mode",
            "type": "boolean",
            "currentValue": option_values["automatic"],
        }
    )
    return options


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
                "sessionId": session_id,
                "update": {
                    "sessionUpdate": "agent_message_chunk",
                    "content": {"type": "text", "text": text},
                },
            },
        )
    else:
        notify(
            "item/agentMessage/delta",
            {"threadId": thread_id, "itemId": f"reply-{counter}", "delta": text},
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
                "threadId": thread_id,
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
                {
                    "protocolVersion": 1,
                    "agentCapabilities": {}
                    if "--no-history" in sys.argv
                    else {
                        "loadSession": True,
                        "sessionCapabilities": dict(
                            {"list": {}},
                            **(
                                {"delete": {}}
                                if history_options().get("delete_supported", True)
                                else {}
                            ),
                        ),
                        "promptCapabilities": {"image": "--image-input" in sys.argv},
                    },
                }
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
            result = {"thread": {"id": thread_id}}
            if acp:
                result = {"sessionId": session_id}
                if "--legacy-controls" in sys.argv:
                    result["modes"] = {
                        "currentModeId": option_values["mode"],
                        "availableModes": [{"id": mode, "name": mode} for mode in ["ask", "code"]],
                    }
                else:
                    result["configOptions"] = controls()
            response(request, result)
        elif method in ("thread/list", "session/list"):
            assert params["cwd"] == str(root)
            options = history_options()
            if options.get("refresh_error") and deleted_history():
                emit(
                    {
                        "id": request["id"],
                        "error": {"code": -32000, "message": "List refresh failed"},
                    }
                )
                continue
            if params.get("archived"):
                rows = [saved("archived-1")] if options.get("archived") else []
                next_cursor = None
            elif options.get("count"):
                start = int(params.get("cursor", "0"))
                stop = min(start + 32, options["count"])
                rows = [saved(f"saved-{n}") for n in range(start, stop)]
                next_cursor = str(stop) if stop < options["count"] else None
                if not start:
                    rows.insert(0, dict(saved("foreign"), cwd=str(root.parent)))
            elif params.get("cursor"):
                rows = [saved("saved-2")]
                next_cursor = None
            else:
                foreign = dict(saved("foreign"), cwd=str(root.parent))
                rows = [foreign, saved("saved-1"), saved("saved-1")]
                if options.get("include_open"):
                    rows.append(saved(session_id if acp else thread_id))
                next_cursor = "history-page-2"
            if options.get("repeat_cursor"):
                next_cursor = "history-page-2"
            for row in rows:
                if options.get("wrong_root"):
                    row["cwd"] = str(root.parent)
                if row["id"] in options.get("active", []):
                    row["status"] = {"type": "active"}
                if row["id"] in options.get("other_provider", []):
                    row["modelProvider"] = "rode_local"
            rows = [row for row in rows if row["id"] not in deleted_history()]
            response(
                request,
                {
                    "sessions" if acp else "data": rows,
                    "nextCursor": next_cursor,
                },
            )
        elif method == "thread/read":
            row = saved(params["threadId"])
            options = history_options()
            if row["id"] in deleted_history() or options.get("read_error") == row["id"]:
                emit(
                    {"id": request["id"], "error": {"code": -32600, "message": "Thread not found"}}
                )
                continue
            if parent := options.get("parents", {}).get(row["id"]):
                row["parentThreadId"] = parent
            if "--wrong-history-root" in sys.argv:
                row["cwd"] = str(root.parent)
            if options.get("wrong_root"):
                row["cwd"] = str(root.parent)
            if row["id"] in options.get("active", []):
                row["status"] = {"type": "active"}
            response(request, {"thread": row})
        elif method in ("thread/delete", "session/delete"):
            options = history_options()
            identity = params["sessionId" if acp else "threadId"]
            if not options.get("delete_supported", True):
                emit(
                    {
                        "id": request["id"],
                        "error": {"code": -32601, "message": "Deletion unsupported"},
                    }
                )
            elif options.get("delete_error") == identity:
                emit({"id": request["id"], "error": {"code": -32000, "message": "Delete failed"}})
            else:
                removed = {identity}
                if options.get("cascade") and not acp:
                    parents = options.get("parents", {})
                    while (
                        children := {
                            child for child, parent in parents.items() if parent in removed
                        }
                        - removed
                    ):
                        removed |= children
                deleted = deleted_history() | removed
                metadata("history-deleted.json", json.dumps(sorted(deleted)))

                def deleted_notifications():
                    if not acp:
                        for removed_id in sorted(removed):
                            notify("thread/deleted", {"threadId": removed_id})
                            if options.get("duplicate_notifications"):
                                notify("thread/deleted", {"threadId": removed_id})

                if options.get("notifications_first"):
                    deleted_notifications()
                if options.get("missing_delete"):
                    emit(
                        {
                            "id": request["id"],
                            "error": {"code": -32002, "message": "Session not found"},
                        }
                    )
                else:
                    response(request, {})
                if not options.get("notifications_first"):
                    deleted_notifications()
        elif method == "thread/resume":
            thread_id = params["threadId"]
            response(request, {"thread": saved(thread_id)})
        elif method == "thread/turns/list":
            assert params["threadId"] == thread_id
            n = 1 if params.get("cursor") else 2
            response(
                request,
                {
                    "data": [
                        {
                            "id": f"saved-turn-{n}",
                            "items": [
                                {
                                    "id": f"user-{n}",
                                    "type": "userMessage",
                                    "content": [{"type": "text", "text": f"Earlier question {n}"}],
                                },
                                {
                                    "id": f"tool-{n}",
                                    "type": "commandExecution",
                                    "command": "pwd",
                                    "aggregatedOutput": str(root),
                                },
                                {
                                    "id": f"agent-{n}",
                                    "type": "agentMessage",
                                    "text": f"Earlier answer {n}",
                                },
                            ],
                        }
                    ],
                    "nextCursor": None if params.get("cursor") else "older-turn",
                },
            )
        elif method == "session/load":
            session_id = params["sessionId"]
            if "--wait-history" in sys.argv:
                continue
            for n in (1, 2):
                for kind, content in (
                    ("user_message_chunk", f"Earlier question {n}"),
                    ("agent_message_chunk", f"Earlier answer {n}"),
                ):
                    chunks = [content]
                    style = root / "saved-response-style.txt"
                    if kind == "user_message_chunk" and style.exists():
                        hint = style.read_text()
                        # Replayed text blocks can be coalesced and fragmented by ACP peers.
                        chunks += ["\n" + hint[: len(hint) // 2], hint[len(hint) // 2 :]]
                    for chunk in chunks:
                        notify(
                            "session/update",
                            {
                                "sessionId": session_id,
                                "update": {
                                    "sessionUpdate": kind,
                                    "messageId": f"{kind}-{n}",
                                    "content": {"type": "text", "text": chunk},
                                },
                            },
                        )
            response(request, {"configOptions": controls()})
        elif method == "model/list":
            models = (
                []
                if params.get("cursor")
                else [
                    {
                        "id": "fixture-fast",
                        "model": "fixture-fast",
                        "displayName": "Fixture Fast",
                        "isDefault": True,
                        "supportedReasoningEfforts": [
                            {"reasoningEffort": "low", "description": "Less reasoning"}
                        ],
                    },
                    {"id": "hidden", "model": "hidden", "hidden": True},
                ]
            )
            if params.get("cursor"):
                models = [
                    {
                        "id": "fixture-deep",
                        "model": "fixture-deep",
                        "displayName": "Fixture Deep",
                        "supportedReasoningEfforts": [
                            {"reasoningEffort": "low"},
                            {"reasoningEffort": "high"},
                        ],
                    }
                ]
            response(
                request,
                {"data": models, "nextCursor": None if params.get("cursor") else "second-page"},
            )
        elif method == "session/set_config_option":
            name = params["configId"]
            valid = next(option for option in controls() if option["id"] == name)
            if "--wait-controls" in sys.argv:
                continue
            if valid["type"] == "boolean":
                assert params["type"] == "boolean" and isinstance(params["value"], bool)
            else:
                assert params["value"] in [option["value"] for option in valid["options"]]
            if "--ignore-controls" not in sys.argv:
                option_values[name] = params["value"]
            response(request, {"configOptions": controls()})
        elif method == "session/set_mode":
            option_values["mode"] = params["modeId"]
            notify(
                "session/update",
                {
                    "sessionId": session_id,
                    "update": {
                        "sessionUpdate": "current_mode_update",
                        "currentModeId": params["modeId"],
                    },
                },
            )
            response(request, {})
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
            elif message == "queue-gated":
                (root / "queue-started").touch()
                while not (root / "queue-continue").exists():
                    time.sleep(0.05)
                finish("Queued gate released.")
            elif message == "approve":
                pending = "approval"
                emit(
                    {
                        "id": 400,
                        "method": "session/request_permission"
                        if acp
                        else "item/commandExecution/requestApproval",
                        "params": {
                            "sessionId": session_id,
                            "threadId": thread_id,
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
                            "threadId": thread_id,
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
            elif message == "history-burst" and not acp:
                # Evict earlier prompts from the bounded display transcript.
                for index in range(140):
                    notify(
                        "item/commandExecution/outputDelta",
                        {"itemId": f"history-tool-{index}", "delta": "Fixture tool output"},
                    )
                finish("History burst complete.")
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
