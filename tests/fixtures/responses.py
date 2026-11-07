#!/usr/bin/env python3
"""Loopback Responses fixture for the optional real Codex test. No model is loaded."""

import json
import sys
from http.server import BaseHTTPRequestHandler, HTTPServer
from pathlib import Path

root = Path(sys.argv[1])


class Handler(BaseHTTPRequestHandler):
    sequence = 0

    def log_message(self, *_args):
        pass

    def do_POST(self):
        Handler.sequence += 1
        body = self.rfile.read(int(self.headers.get("Content-Length", "0")))
        (root / "request.json").write_bytes(body)
        self.send_response(200)
        self.send_header("Content-Type", "text/event-stream")
        self.send_header("Connection", "close")
        self.end_headers()
        item = {
            "id": f"msg_fixture_{Handler.sequence}",
            "type": "message",
            "role": "assistant",
            "status": "completed",
            "content": [{"type": "output_text", "text": "Local Responses fixture reply."}],
        }
        # Opt-in real-tool regression: one code-mode call, then a normal reply.
        # No model/account is used; the actual Codex runtime must execute it.
        tool = (root / "tool-mode").exists() and not (root / "tool-replied").exists()
        if tool:
            (root / "tool-replied").touch()
            item = dict(
                id="tool_fixture",
                type="custom_tool_call",
                name="exec",
                call_id="call_fixture",
                input='const r = await tools.exec_command({cmd: "printf RODE_TOOL_OK > rode-tool-result.txt; cat rode-tool-result.txt", max_output_tokens: 1000}); text(r);',
            )
        file_tool = (root / "file-mode").exists() and not (root / "file-replied").exists()
        if file_tool:
            (root / "file-replied").touch()
            item = dict(
                id="file_fixture",
                type="function_call",
                name="rode_write_file",
                call_id="file_fixture",
                arguments=json.dumps(
                    {
                        "path": "native-file.txt",
                        "text": "Native file-only edit.\n",
                        "revision": None,
                    }
                ),
            )
        elif (root / "file-attack").exists() and not (root / "patch-replied").exists():
            (root / "patch-replied").touch()
            file_tool = True
            item = dict(
                id="patch_fixture",
                type="custom_tool_call",
                name="apply_patch",
                call_id="patch_fixture",
                input="*** Begin Patch\n*** Add File: ../outside-patch.txt\n+must be blocked\n*** Add File: builtin-patch.txt\n+must also be blocked\n*** End Patch",
            )
        offline_tool = (root / "offline-mode").exists() and not (root / "offline-replied").exists()
        if offline_tool:
            (root / "offline-replied").touch()
            item = dict(
                id="offline_fixture",
                type="function_call",
                name="rode_run_command",
                call_id="offline_fixture",
                arguments=json.dumps(
                    {
                        "command": "/usr/bin/python3 -c 'import socket; s=socket.socket()' > network.txt 2>&1; result=$?; test \"$result\" -ne 0 && printf OFFLINE_BUILD_OK > offline-build.txt"
                    }
                ),
            )
        question = (root / "question-mode").exists() and not (root / "question-replied").exists()
        if question:
            (root / "question-replied").touch()
            item = dict(
                id="question_fixture",
                type="function_call",
                name="request_user_input",
                call_id="question_fixture",
                arguments=json.dumps(
                    {
                        "questions": [
                            {
                                "id": "test_scope",
                                "header": "Tests",
                                "question": "Which tests should I run?",
                                "options": [
                                    {"label": "All", "description": "Run the full suite"},
                                    {"label": "Focused", "description": "Run the changed tests"},
                                ],
                            }
                        ]
                    }
                ),
            )
        async_question = (root / "async-question-mode").exists() and not (
            root / "async-question-replied"
        ).exists()
        if async_question:
            (root / "async-question-replied").touch()
            item = dict(
                id="async_question_fixture",
                type="function_call",
                name="request_user_input_async",
                call_id="async_question_fixture",
                arguments=json.dumps(
                    {
                        "questions": [
                            {
                                "title": "Is network access enabled?",
                                "options": ["Enabled", "Disabled"],
                            },
                            {"title": "Which tests should I run?"},
                        ]
                    }
                ),
            )
        response = {
            "id": f"resp_fixture_{Handler.sequence}",
            "object": "response",
            "created_at": 1,
            "status": "completed",
            "model": "fixture-local-model",
            "output": [item],
            "usage": {"input_tokens": 1, "output_tokens": 1, "total_tokens": 2},
        }
        events = [
            {
                "type": "response.created",
                "response": dict(response, status="in_progress", output=[]),
            },
            {"type": "response.output_item.added", "output_index": 0, "item": item},
            {
                "type": "response.output_text.delta",
                "item_id": item["id"],
                "output_index": 0,
                "content_index": 0,
                "delta": "Local Responses fixture reply.",
            },
            {"type": "response.output_item.done", "output_index": 0, "item": item},
            {"type": "response.completed", "response": response},
        ]
        if tool or file_tool or offline_tool or question or async_question:
            events = [event for event in events if event["type"] != "response.output_text.delta"]
        for event in events:
            self.wfile.write(
                ("event: " + event["type"] + "\ndata: " + json.dumps(event) + "\n\n").encode()
            )
            self.wfile.flush()
        self.close_connection = True


server = HTTPServer(("127.0.0.1", 0), Handler)
(root / ".port.tmp").write_text(str(server.server_port))
(root / ".port.tmp").replace(root / "port")
server.serve_forever()
