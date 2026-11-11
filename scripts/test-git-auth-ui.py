#!/usr/bin/env python3
"""Real HTTPS Git credentials: cancel, retry, no persistence, and local/SSH execution."""

import base64
import http.server
import json
import os
import shlex
import shutil
import ssl
import subprocess
import threading
import time
from pathlib import Path
from urllib.parse import urlsplit

remote = os.environ.get("RODE_GIT_AUTH_UI_REMOTE") == "1"
os.environ["RODE_UI_ARTIFACTS"] = str(
    Path(__file__).resolve().parent.parent / "target/git-auth-ui" / ("ssh" if remote else "local")
)
from ui_harness import (
    ARTIFACTS,
    BINARY,
    assert_running,
    click_button,
    close_window,
    fixture_directory,
    fixture_environment,
    focus_window,
    key,
    text,
    wait_for_button,
    wait_for_project,
    wait_for_ui_state,
    wait_for_window,
    write_python_launcher,
)

with fixture_directory(prefix="rode-git-auth-ui-") as base:
    root = base / "project"
    root.mkdir()
    home = base / "home"
    home.mkdir(mode=0o700)
    real_git = shutil.which("git")
    username, password = "rode-private-user-6842", "rode-private-token-9753"
    wrong_password = "rode-private-rejected-3865"
    forbidden = [
        username,
        password,
        wrong_password,
        base64.b64encode(f"{username}:{wrong_password}".encode()).decode(),
        base64.b64encode(f"{username}:{password}".encode()).decode(),
    ]
    environment = fixture_environment(
        base,
        HOME=str(home),
        XDG_CONFIG_HOME=str(base / "config"),
        XDG_DATA_HOME=str(base / "data"),
        GIT_CONFIG_GLOBAL=str(base / "global.gitconfig"),
        GIT_CONFIG_NOSYSTEM="1",
        RODE_UI_TEST_STATE=str(base / "ui-state.json"),
        GIT_TRACE=str(base / "trace.log"),
    )
    tools = base / "bin"
    tools.mkdir()
    wrapper = tools / "git"
    # Inspect inherited credentials in memory, without recording the user's
    # ambient environment or writing a credential into the test wrapper itself.
    write_python_launcher(
        wrapper,
        f"""import json,os,shlex,sys
try:
    spec=next(arg.removeprefix('credential.helper=!') for arg in sys.argv if arg.startswith('credential.helper=!'))
    descriptor=int(shlex.split(spec)[-1])
    credentials=json.loads(os.pread(descriptor,262144,0))
except (OSError,ValueError,StopIteration):
    credentials=[]
for credential in credentials:
    for value in [credential['username'],credential['password']]:
        assert all(value not in item for item in [*sys.argv,*os.environ.values()]), 'Git credentials leaked into argv/environment'
with open({str(base / "commands.jsonl")!r},'a') as out:
    out.write(json.dumps(dict(argv=sys.argv,credential_fd=bool(credentials)))+'\\n')
os.execv({real_git!r},[{real_git!r},*sys.argv[1:]])
""",
        python=shutil.which("python3"),
    )
    environment["PATH"] = str(tools) + os.pathsep + environment["PATH"]

    def git(*args):
        return subprocess.check_output(
            [real_git, "-C", str(root), *args], env=environment, text=True
        ).strip()

    git("init", "-b", "main")
    server_root = base / "server"
    server_root.mkdir()
    subprocess.run(
        [real_git, "init", "--bare", "-b", "main", str(server_root / "repo.git")],
        check=True,
        capture_output=True,
    )
    git(
        "-c",
        "user.name=Fixture",
        "-c",
        "user.email=fixture@example.invalid",
        "commit",
        "--allow-empty",
        "-m",
        "Fixture",
    )
    git("push", str(server_root / "repo.git"), "main")
    subprocess.run(
        [
            real_git,
            "--git-dir",
            str(server_root / "repo.git"),
            "config",
            "http.receivepack",
            "true",
        ],
        check=True,
        env=environment,
    )
    certificate, private_key = base / "certificate.pem", base / "key.pem"
    subprocess.run(
        [
            "openssl",
            "req",
            "-x509",
            "-newkey",
            "rsa:2048",
            "-nodes",
            "-days",
            "1",
            "-subj",
            "/CN=localhost",
            "-addext",
            "subjectAltName=IP:127.0.0.1,DNS:localhost",
            "-keyout",
            str(private_key),
            "-out",
            str(certificate),
        ],
        check=True,
        capture_output=True,
    )
    accepted = []
    rejected = []

    class Handler(http.server.BaseHTTPRequestHandler):
        def log_message(self, *_args):
            pass

        def handle_git(self):
            expected = "Basic " + forbidden[-1]
            if self.headers.get("Authorization") != expected:
                if self.headers.get("Authorization"):
                    rejected.append(True)
                self.send_response(401)
                self.send_header("WWW-Authenticate", 'Basic realm="Rode fixture"')
                self.send_header("Content-Length", "0")
                self.end_headers()
                return
            accepted.append(self.path)
            url = urlsplit(self.path)
            body = self.rfile.read(int(self.headers.get("Content-Length", "0")))
            env = dict(
                environment,
                GIT_PROJECT_ROOT=str(server_root),
                GIT_HTTP_EXPORT_ALL="1",
                PATH_INFO=url.path,
                QUERY_STRING=url.query,
                REQUEST_METHOD=self.command,
                CONTENT_TYPE=self.headers.get("Content-Type", ""),
                CONTENT_LENGTH=str(len(body)),
                REMOTE_USER="authenticated",
            )
            result = subprocess.run(
                [real_git, "http-backend"], input=body, capture_output=True, env=env, check=True
            )
            headers, payload = result.stdout.split(b"\r\n\r\n", 1)
            lines = [line.decode().split(":", 1) for line in headers.split(b"\r\n")]
            status = next(
                (
                    int(value.strip().split()[0])
                    for name, value in lines
                    if name.lower() == "status"
                ),
                200,
            )
            self.send_response(status)
            for name, value in lines:
                if name.lower() != "status":
                    self.send_header(name, value.strip())
            self.send_header("Content-Length", str(len(payload)))
            self.end_headers()
            self.wfile.write(payload)

        do_GET = handle_git
        do_POST = handle_git

    server = http.server.ThreadingHTTPServer(("127.0.0.1", 0), Handler)
    tls = ssl.SSLContext(ssl.PROTOCOL_TLS_SERVER)
    tls.load_cert_chain(certificate, private_key)
    server.socket = tls.wrap_socket(server.socket, server_side=True)
    thread = threading.Thread(target=server.serve_forever, daemon=True)
    thread.start()
    origin = f"https://127.0.0.1:{server.server_port}"
    git("remote", "add", "origin", origin + "/repo.git")
    git("config", "http.sslCAInfo", str(certificate))
    git("config", "trace2.eventTarget", str(base / "trace-events.jsonl"))
    store = base / "credential-store.log"
    store_helper = base / "store-helper"
    store_helper.write_text(
        f"#!/bin/sh\nprintf '%s\\n' \"$1\" >> {shlex.quote(str(store))}\n"
        f"cat >> {shlex.quote(str(store))}\n"
    )
    store_helper.chmod(0o700)
    helper_command = "!" + shlex.quote(str(store_helper))
    git("config", "credential.helper", helper_command)
    git("config", f"credential.{origin}.helper", helper_command)
    args = [str(BINARY), "--theme", "studio-neutral"]
    args += (
        [
            "--ssh",
            "rode-test",
            "--ssh-config",
            os.environ["RODE_TEST_SSH_CONFIG"],
            "--remote-path",
            str(root),
        ]
        if remote
        else [str(root)]
    )
    state = base / "ui-state.json"
    log_path = ARTIFACTS / "auth.log"
    with log_path.open("w") as log:
        process = subprocess.Popen(args, env=environment, stdout=log, stderr=log)
        window = None
        try:
            window = wait_for_window(process, log_path)
            wait_for_project(window, process, log_path, "project")
            focus_window(window)
            key("g", ("Control_L", "Shift_L"))

            def click(name):
                click_button(window, process, log_path, state, name)

            def enter(name, value):
                click(name)
                wait_for_button(window, process, log_path, state, name, focused=True)
                before = json.loads(state.read_text())["pass"]
                text(value)
                wait_for_button(window, process, log_path, state, name, changed_after=before)
                # A first changed frame can precede the final queued characters.
                # Moving to the next control acknowledges later native input
                # without changing focus or publishing secret lengths/values.
                # Tab could steal trailing text if a slow frame batches events.
                next_control = {
                    "git-auth-username": "git-auth-password",
                    "git-auth-password": "git-auth-submit",
                }[name]
                wait_for_button(window, process, log_path, state, next_control, hovered=True)

            def form(previous=None):
                snapshot = wait_for_ui_state(
                    process,
                    log_path,
                    state,
                    lambda snapshot: any(
                        button["name"].startswith("git-auth-form:")
                        and button["enabled"]
                        and button["name"] != previous
                        for button in snapshot["buttons"]
                    ),
                    "A fresh authentication challenge did not become interactive",
                    timeout=20,
                )
                return next(
                    button["name"]
                    for button in snapshot["buttons"]
                    if button["name"].startswith("git-auth-form:") and button["enabled"]
                )

            # Cancel discards both entries. A later operation must ask afresh.
            click("git-network-Fetch")
            form()
            enter("git-auth-username", username)
            enter("git-auth-password", password)
            click("git-auth-cancel")
            wait_for_ui_state(
                process,
                log_path,
                state,
                lambda snapshot: all(
                    not button["name"].startswith("git-auth-form:")
                    for button in snapshot["buttons"]
                ),
                "Cancelling authentication did not close the form",
            )
            click("git-network-Fetch")
            rejected_form = form()
            enter("git-auth-username", username)
            enter("git-auth-password", wrong_password)
            click("git-auth-submit")
            deadline = time.monotonic() + 20
            while not rejected and time.monotonic() < deadline:
                assert_running(process, log_path)
                time.sleep(0.05)
            assert rejected, "Rejected credentials did not reach the HTTPS fixture"
            form(rejected_form)
            buttons = {item["name"]: item for item in json.loads(state.read_text())["buttons"]}
            assert not buttons["git-auth-submit"]["enabled"], (
                "Rejected credential entries were retained in the form"
            )
            enter("git-auth-username", username)
            enter("git-auth-password", password)
            submitted_pass = json.loads(state.read_text())["pass"]
            click("git-auth-submit")
            deadline = time.monotonic() + 20
            while time.monotonic() < deadline:
                assert_running(process, log_path)
                result = subprocess.run(
                    [
                        real_git,
                        "-C",
                        str(root),
                        "rev-parse",
                        "--verify",
                        "refs/remotes/origin/main",
                    ],
                    capture_output=True,
                    env=environment,
                )
                if result.returncode == 0:
                    break
                time.sleep(0.05)
            else:
                raise AssertionError(
                    f"Authenticated fetch did not update the remote tracking branch; accepted={len(accepted)}, rejected={len(rejected)}; layout={state.read_text()}"
                )
            assert accepted
            # Ref publication precedes the desktop's fetch reply and subsequent
            # status refresh. A repository mutation needs that completion too.
            wait_for_button(
                window, process, log_path, state, "git-network-Push", after_pass=submitted_pass
            )
            git("branch", "--set-upstream-to", "origin/main", "main")
            git(
                "-c",
                "user.name=Fixture",
                "-c",
                "user.email=fixture@example.invalid",
                "commit",
                "--allow-empty",
                "-m",
                "Authenticated push fixture",
            )
            expected_commit = git("rev-parse", "HEAD")
            click("git-network-Push")
            form()
            enter("git-auth-username", username)
            enter("git-auth-password", password)
            click("git-auth-submit")
            deadline = time.monotonic() + 20
            while time.monotonic() < deadline:
                assert_running(process, log_path)
                actual = subprocess.check_output(
                    [
                        real_git,
                        "--git-dir",
                        str(server_root / "repo.git"),
                        "rev-parse",
                        "refs/heads/main",
                    ],
                    env=environment,
                    text=True,
                ).strip()
                if actual == expected_commit:
                    break
                time.sleep(0.05)
            else:
                raise AssertionError("Authenticated push did not update the HTTPS repository")
            # A successful operation must not install a reusable credential cache.
            click("git-network-Fetch")
            wait_for_button(window, process, log_path, state, "git-auth-username")
            click("git-auth-cancel")
            close_window(window, process)
            if not remote:
                commands = [
                    json.loads(line) for line in (base / "commands.jsonl").read_text().splitlines()
                ]
                assert any(command["credential_fd"] for command in commands), (
                    "Git did not receive the anonymous credential descriptor"
                )
            for path in [log_path, *base.rglob("*")]:
                if path.is_file() and path != private_key:
                    data = path.read_bytes()
                    assert all(value.encode() not in data for value in forbidden), (
                        f"Git credentials were persisted in {path.name}"
                    )
            print(
                f"Git HTTPS ({'SSH' if remote else 'local'}): cancel, rejection, authenticated fetch/push, fresh prompts and no saved credentials passed"
            )
        finally:
            if process.poll() is None:
                process.terminate()
                process.wait(timeout=10)
            server.shutdown()
            server.server_close()
            thread.join(timeout=5)
