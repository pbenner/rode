"""Deterministic API v1 adapter used by local, SSH and UI regression tests."""

import json
import os
import sys
import time
from pathlib import Path

request = json.load(sys.stdin)
settings = request.get("settings", {})
if settings.get("marker"):
    Path(settings["marker"]).write_text(str(os.getpid()))
if settings.get("sleep"):
    time.sleep(settings["sleep"])
reply = {"api": 1}
if request["operation"] == "format":
    reply["text"] = request["text"].upper()
else:
    reply["output"] = f"Adapter on {request['root']} · {request['operation']}"
reply["diagnostics"] = [
    {
        "code": "DEMO",
        "message": "Adapter diagnostic",
        "row": 1,
        "column": 1,
        "end_row": 1,
        "end_column": 2,
        "severity": "warning",
    }
]
if settings.get("outside"):
    reply["output"] = str(Path(settings["outside"]).exists())
json.dump(reply, sys.stdout)
