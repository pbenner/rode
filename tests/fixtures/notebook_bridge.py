"""Minimal stateful-kernel peer for runtime ownership tests, without Jupyter."""

import json
import sys

for line in sys.stdin:
    message = json.loads(line)
    if message["cmd"] == "start":
        print(
            json.dumps(
                dict(
                    kind="ready", language_info={"name": "python"}, python="python3", host="fixture"
                )
            ),
            flush=True,
        )
    elif message["cmd"] == "shutdown":
        break
