#!/usr/bin/python3
"""Deterministic Marp-shaped CLI for snapshot, bounds and lifetime checks."""

import html
import json
import os
import signal
import subprocess
import sys
import time
from pathlib import Path

args = sys.argv[1:]
source = sys.stdin.read()
if "RODE_TEST_SCOPE=" in source:
    outside = source.split("RODE_TEST_SCOPE=", 1)[1].splitlines()[0].strip()
    source += f"\nOutside visible: {Path(outside).exists()}"
output = Path(args[args.index("--output") + 1])
config = json.loads(Path(args[args.index("--config-file") + 1]).read_text())
assert args[0] == "-"
assert all(flag in args for flag in ("--no-watch", "--no-server", "--no-preview", "--no-html"))
assert not config.get("html", False)
if "RODE_TEST_SLEEP" in source:
    # Puppeteer owns a separate browser process group and closes it from its
    # SIGINT handler; a blanket SIGKILL of just the CLI would orphan it.
    child = subprocess.Popen(
        [sys.executable, "-c", "import time;time.sleep(60)"], start_new_session=True
    )

    def stop_browser(_signal, _frame):
        os.killpg(child.pid, signal.SIGKILL)
        child.wait()
        Path("marp-stopped").write_text("browser stopped")
        raise SystemExit(130)

    signal.signal(signal.SIGINT, stop_browser)
    Path("marp-pids.json").write_text(json.dumps([os.getpid(), child.pid]))
    time.sleep(60)
if "RODE_TEST_CONFLICT" in source:
    Path("deck.html").write_text("external change")
if "RODE_TEST_RECORD" in source:
    Path("marp-snapshot.json").write_text(
        json.dumps({"text": source, "cwd": os.getcwd(), "config": config, "args": args})
    )
if "RODE_TEST_OVERSIZE" in source:
    output.write_bytes(b"x" * (9 * 1024 * 1024))
elif "--pdf" in args:
    output.write_bytes(Path(__file__).with_name("pages.pdf").read_bytes())
elif "--pptx" in args:
    output.write_bytes(b"PK\x03\x04fixture-pptx")
else:
    output.write_text("<!doctype html><html><body>" + html.escape(source) + "</body></html>")
print("Converted snapshot", file=sys.stderr)
