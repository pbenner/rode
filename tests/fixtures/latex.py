#!/usr/bin/env python3
"""Deterministic installed-tool-shaped peer; no TeX installation required."""

import json
import os
import subprocess
import sys
import time
from pathlib import Path

if "-g=/dev/null" in sys.argv:
    assert sys.argv[1:] == ["-g=/dev/null", "-l", "-"]
    print(sys.stdin.read().replace("UNFORMATTED", "formatted"), end="")
elif "-I0" in sys.argv:
    assert sys.argv[1:] == ["-q", "-I0", "-f", "%l:%c:%d:%k:%n:%m\n"]
    text = sys.stdin.read()
    if "ISSUE" in text:
        print("2:6:5:Warning:8:An example: with a colon")
        sys.exit(2)
else:
    source, output = Path(sys.argv[1]), Path(sys.argv[2])
    text = source.read_text()
    (output / "invocation.json").write_text(
        json.dumps(dict(arguments=sys.argv[1:], cwd=os.getcwd()))
    )
    if "ASSERT_HIDDEN=" in text:
        hidden = text.split("ASSERT_HIDDEN=", 1)[1].splitlines()[0]
        assert not Path(hidden).exists()
    if "SLOW" in text:
        child = subprocess.Popen([sys.executable, "-c", "import time; time.sleep(300)"])
        (output / "pids.json").write_text(json.dumps([os.getpid(), child.pid]))
        time.sleep(300)
    if "ERROR" in text:
        (output / (source.stem + ".log")).write_text("Undefined control sequence.\n")
        print(f"{source}:2:5: Undefined control sequence.")
        sys.exit(1)
    if "NO_PDF" not in text:
        pdf = Path(__file__).with_name("pages.pdf").read_bytes()
        if "BLUE_BUILD" in text:
            pdf = pdf.replace(b"0.9 0.15 0.15 rg", b"0.15 0.15 0.9 rg")
        (output / (source.stem + ".pdf")).write_bytes(pdf)
    print("LATEX_BUILD_OK")
