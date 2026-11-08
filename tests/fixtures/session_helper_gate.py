#!/usr/bin/env python3
"""Hold the current helper's readiness check during notebook shutdown tests."""

import os
import sys
import time
from pathlib import Path

root = Path(os.environ["RODE_TEST_HELPER_GATE"])
if sys.argv[1:] == ["protocol"] and (root / "hold").exists():
    (root / "entered").touch()
    deadline = time.monotonic() + 10
    while not (root / "release").exists():
        if time.monotonic() >= deadline:
            sys.exit(2)
        time.sleep(0.005)
    if (root / "fail").exists():
        sys.exit(2)

program = os.environ["RODE_TEST_REAL_SESSION_HELPER"]
os.execv(program, [program, *sys.argv[1:]])
