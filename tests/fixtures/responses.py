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
        response = {
            "id": f"resp_fixture_{Handler.sequence}",
            "object": "response",
            "created_at": 1,
            "status": "completed",
            "model": "fixture-local-model",
            "output": [item],
            "usage": {"input_tokens": 1, "output_tokens": 1, "total_tokens": 2},
        }
        for event in [
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
        ]:
            self.wfile.write(
                ("event: " + event["type"] + "\ndata: " + json.dumps(event) + "\n\n").encode()
            )
            self.wfile.flush()
        self.close_connection = True


server = HTTPServer(("127.0.0.1", 0), Handler)
(root / "port").write_text(str(server.server_port))
server.serve_forever()
