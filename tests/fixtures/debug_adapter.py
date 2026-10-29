#!/usr/bin/env python3
"""Deterministic DAP peer: deferred launch, reverse PTY, Unicode and late replies."""

import json
import os
import subprocess
import sys
import time
from pathlib import Path

seq = 0
launch = None
row = 3
late = None
source = str(Path("main.py").resolve())
log = Path("debug-rpc.jsonl")


def send(value):
    global seq
    seq += 1
    value["seq"] = seq
    data = json.dumps(value, ensure_ascii=False).encode()
    sys.stdout.buffer.write(f"Content-Length: {len(data)}\r\n\r\n".encode() + data)
    sys.stdout.buffer.flush()


def response(request, body=None):
    send(
        dict(
            type="response",
            request_seq=request["seq"],
            command=request["command"],
            success=True,
            body=body or {},
        )
    )


def event(name, **body):
    send(dict(type="event", event=name, body=body))


while True:
    header = sys.stdin.buffer.readline()
    if not header:
        break
    length = int(header.split(b":")[1])
    assert sys.stdin.buffer.readline() == b"\r\n"
    message = json.loads(sys.stdin.buffer.read(length))
    with log.open("a") as output:
        output.write(json.dumps(message) + "\n")
    if message["type"] == "response":
        assert message["success"], message
        Path("terminal-pid").write_text(str(message["body"]["processId"]))
        continue
    command, args = message["command"], message.get("arguments", {})
    if command == "initialize":
        response(
            message,
            dict(
                supportsConfigurationDoneRequest=True,
                supportsConditionalBreakpoints=True,
                supportsHitConditionalBreakpoints=True,
                supportsLogPoints=True,
                exceptionBreakpointFilters=[dict(filter="raised", label="Raised")],
            ),
        )
    elif command in ("launch", "attach"):
        launch = message
        # A detached grandchild must still be cleaned up with this session.
        child = subprocess.Popen(
            [sys.executable, "-c", "import time; time.sleep(600)"],
            start_new_session=True,
            stdout=subprocess.DEVNULL,
            stderr=subprocess.DEVNULL,
        )
        Path("adapter-child").write_text(str(child.pid))
        program = "import os; from pathlib import Path; print('CWD='+os.getcwd(),flush=True); print('TTY='+str(os.isatty(0)),flush=True); print('OUTSIDE='+str(Path('../private.txt').exists()),flush=True); value=input('Name: '); print('HELLO '+value,flush=True); Path('input-result').write_text(value)"
        if args.get("blockedInput"):
            program = (
                "import tty,time; tty.setraw(0); print('RAWREADY',flush=True); time.sleep(600)"
            )
        if args.get("flood"):
            program = (
                "import sys; sys.stdout.write(('output burst\\n')*250000); sys.stdout.flush(); "
                + program
            )
        send(
            dict(
                type="request",
                command="runInTerminal",
                arguments=dict(
                    kind="integrated",
                    args=[sys.executable, "-u", "-c", program],
                    cwd=os.getcwd(),
                    env={"RODE_DEBUG_TEST": "yes", "REMOVE_ME": None},
                ),
            )
        )
        event("initialized")
    elif command == "setBreakpoints":
        if launch["arguments"].get("slowReader"):
            time.sleep(0.003)
        response(
            message,
            dict(
                breakpoints=[
                    dict(
                        id=i + 1,
                        line=bp["line"],
                        verified=bp["line"] != 99,
                        message="No code here" if bp["line"] == 99 else "",
                    )
                    for i, bp in enumerate(args["breakpoints"])
                ]
            ),
        )
    elif command == "setExceptionBreakpoints":
        response(message)
    elif command == "configurationDone":
        response(message)
        response(launch)
        event("stopped", reason="breakpoint", threadId=7, allThreadsStopped=True)
    elif command == "threads":
        response(message, dict(threads=[dict(id=7, name="Main 🦀")]))
    elif command == "stackTrace":
        response(
            message,
            dict(
                stackFrames=[
                    dict(id=41, name="main λ", line=row, column=1, source=dict(path=source))
                ]
            ),
        )
    elif command == "scopes":
        response(message, dict(scopes=[dict(name="Locals", variablesReference=1, expensive=False)]))
    elif command == "variables":
        response(
            message,
            dict(
                variables=[
                    dict(
                        name="answer",
                        value="42 🦀",
                        type="str",
                        variablesReference=2 if args["variablesReference"] == 1 else 0,
                    )
                ]
            ),
        )
    elif command == "evaluate":
        if args["expression"] == "late":
            late = message
        else:
            response(message, dict(result="42 🦀", variablesReference=0))
    elif command in ("next", "stepIn", "stepOut", "pause"):
        response(message)
        event("continued", threadId=7, allThreadsContinued=True)
        row += 1
        event("stopped", reason="step", threadId=7, allThreadsStopped=True)
    elif command == "continue":
        response(message)
        event("continued", threadId=7, allThreadsContinued=True)
        if late:
            response(late, dict(result="STALE", variablesReference=999))
        event("output", output="RUNNING λ\n")
    elif command == "disconnect":
        Path("disconnected.json").write_text(json.dumps(args))
        response(message)
        break
    else:
        response(message)
