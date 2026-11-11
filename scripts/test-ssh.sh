#!/usr/bin/env bash
# Test the real SSH transport without using or modifying ~/.ssh.
set -euo pipefail
# All fixture caches and credentials must stay private under permissive caller umasks.
umask 077
cd "$(dirname "$0")/.."
# Direct test binaries live in deps/ or tests/, away from the packaged helper.
# Use the supplied artifact rather than a helper installed on the invoking host.
if [[ -n "${RODE_TEST_SESSION_HELPER:-}" && -z "${RODE_HOST_HELPER+x}" && -z "${RODE_SESSION_HELPER+x}" ]]; then
    if [[ ! -x "$RODE_TEST_SESSION_HELPER" ]]; then
        echo "Missing executable RODE_TEST_SESSION_HELPER=$RODE_TEST_SESSION_HELPER" >&2
        exit 1
    fi
    export RODE_SESSION_HELPER="$RODE_TEST_SESSION_HELPER"
fi
test_dir=$(mktemp -d "${TMPDIR:-/tmp}/rode-ssh-test.XXXXXX")
test_dir=$(cd "$test_dir" && pwd -P)
mkdir -m 700 "$test_dir/codex-home" "$test_dir/home"
sshd_pid=
cleanup() {
    if [[ -n "$sshd_pid" ]]; then kill "$sshd_pid" 2>/dev/null || true; wait "$sshd_pid" 2>/dev/null || true; fi
    if [[ -n "${RODE_CI_SSH_LOG_DIR:-}" && -f "$test_dir/sshd.log" ]]; then
        mkdir -p "$RODE_CI_SSH_LOG_DIR"
        cp "$test_dir/sshd.log" "$RODE_CI_SSH_LOG_DIR/sshd.log"
    fi
    rm -rf "$test_dir"
}
trap cleanup EXIT
trap 'exit 143' TERM
sshd_bin=$(command -v sshd || true)
if [[ -z "$sshd_bin" && -x /usr/sbin/sshd ]]; then sshd_bin=/usr/sbin/sshd; fi
if [[ -z "$sshd_bin" ]]; then echo 'Install openssh-server to run the isolated SSH integration test.' >&2; exit 1; fi
ssh-keygen -q -t ed25519 -N '' -f "$test_dir/host"
# Offer Ed25519 as well as the ECDSA key recorded in known_hosts. Clients must
# negotiate the trusted type instead of reporting an unrelated key as changed.
ssh-keygen -q -t ecdsa -b 256 -N '' -f "$test_dir/host-known"
ssh-keygen -q -t ed25519 -N '' -f "$test_dir/client"
test_port=$(python3 -c 'import socket; s=socket.socket(); s.bind(("127.0.0.1",0)); print(s.getsockname()[1]); s.close()')
test_user=$(id -un)
# A developer's installed helper must not outrank the private-home fixture.
test_path=$(python3 scripts/ssh_fixture.py path "$test_dir/path" "$PATH")
# Pin sshd's X11 cookies to the fixture independently of shell environment
# setup, even though the account database points at the real user's home.
xauth_bin=$(command -v xauth || true)
if [[ -z "$xauth_bin" && "${RODE_TEST_REQUIRE_X11:-0}" == 1 ]]; then
    echo 'Install xauth for required X11 forwarding coverage.' >&2
    exit 1
fi
mkdir -p "$test_dir/home/.local/bin"
test_shell="$test_dir/home/.local/bin/project-shell"
cat > "$test_shell" <<'EOF'
#!/bin/sh
unset ENV BASH_ENV
export PS1='RODE_TEST_SHELL_READY> '
exec /bin/sh -i
EOF
chmod 700 "$test_shell"
export RODE_TEST_PROJECT_SHELL="$test_shell"
if [[ "${RODE_SSH_TEST_NO_PYTHON:-0}" == 1 ]]; then
    mkdir -m 700 "$test_dir/native-path"
    for program in sh uname id stat sha256sum wc head mktemp ln chmod cut mkdir rm rmdir readlink git bwrap true env; do
        if [[ "$program" == bwrap && "${RODE_SSH_TEST_NO_BWRAP:-0}" == 1 ]]; then continue; fi
        program_path=$(command -v "$program" || true)
        if [[ "$program_path" == /* ]]; then ln -s "$program_path" "$test_dir/native-path/$program"; fi
    done
    test_path="$test_dir/native-path"
else
    # The sandbox exposes this fixture's home, not the invoking user's home.
    # Copy the installed uv tools together so default Ruff can find uvx and its
    # sibling uv without granting access to the real home or installing tools.
    uv_path=$(command -v uv || true)
    uvx_path=$(command -v uvx || true)
    if [[ "$uv_path" == /* && "$uvx_path" == /* ]]; then
        mkdir -p "$test_dir/home/.local/bin"
        cp -L "$uv_path" "$test_dir/home/.local/bin/uv"
        cp -L "$uvx_path" "$test_dir/home/.local/bin/uvx"
        chmod 700 "$test_dir/home/.local/bin/uv" "$test_dir/home/.local/bin/uvx"
        test_path="$test_dir/home/.local/bin:$test_path"
    fi
    if [[ "${RODE_SSH_TEST_NO_BWRAP:-0}" == 1 ]]; then
        # Exercise unavailable sandboxing even on development hosts that allow
        # user namespaces. Keep all other tools available to the UI fixture.
        mkdir -m 700 "$test_dir/no-bwrap"
        cat > "$test_dir/no-bwrap/bwrap" <<'EOF'
#!/bin/sh
printf '%s\n' 'bwrap: sandbox unavailable in SSH test fixture' >&2
exit 1
EOF
        chmod 700 "$test_dir/no-bwrap/bwrap"
        test_path="$test_dir/no-bwrap:$test_path"
    fi
fi
# Quote shell exports and OpenSSH tokens separately; TMPDIR may contain spaces
# or quotes, and the remote host uses the canonical private fixture directory.
python3 scripts/ssh_fixture.py configure "$test_dir" "$test_path" "$test_shell" "$xauth_bin" \
    "$test_port" "$test_user" "${RODE_SSH_TEST_X11:-yes}"
"$sshd_bin" -D -e -f "$test_dir/sshd_config" > "$test_dir/sshd.log" 2>&1 &
sshd_pid=$!
if ! python3 - "$test_port" <<'PY'
import socket
import sys
import time

deadline = time.monotonic() + 5
while time.monotonic() < deadline:
    try:
        with socket.create_connection(("127.0.0.1", int(sys.argv[1])), timeout=0.2):
            break
    except OSError:
        time.sleep(0.05)
else:
    sys.exit("Fixture sshd did not start listening")
PY
then
    cat "$test_dir/sshd.log" >&2
    exit 1
fi
# OpenSSH builds a shell command for ProxyJump and cannot quote every -F path.
# Keep a live directory handle so its config path has no shell metacharacters,
# regardless of the caller's temporary directory. This fixture is Linux-only.
exec {fixture_dir_fd}<"$test_dir"
export RODE_TEST_SSH_CONFIG="/proc/$$/fd/$fixture_dir_fd/ssh_config"
export RODE_TEST_SSH_HOME="$test_dir/home"
export RODE_TEST_SSH_X11="${RODE_SSH_TEST_X11:-yes}"
if [[ -z "$xauth_bin" ]]; then export RODE_TEST_SSH_X11=no; fi
export RODE_TEST_CODEX_REMOTE_HOME="$test_dir/codex-home"
# A non-root sshd can accept commands yet lack permission to create login-recorded
# PTYs in a container. Detect that fixture limitation independently of RODE.
if timeout 10 ssh -F "$RODE_TEST_SSH_CONFIG" -tt rode-test true </dev/null >"$test_dir/pty-probe.log" 2>&1; then
    export RODE_TEST_SSH_PTY=1
else
    export RODE_TEST_SSH_PTY=0
    if [[ "${RODE_TEST_REQUIRE_SSH_PTY:-0}" == 1 ]]; then
        cat "$test_dir/pty-probe.log" >&2
        exit 1
    fi
    echo 'Fixture SSH PTY unavailable; native session PTYs are still exercised.' >&2
fi
case "${RODE_SSH_RUN_TRANSPORT:-1}" in
    1)
        if [[ -n "${RODE_SSH_TEST_BINARY:-}" ]]; then
            transport=(python3 scripts/rust_tests.py --runner "$RODE_SSH_TEST_BINARY" "${RODE_SSH_TEST_FILTER:-}" --ignored --nocapture)
        else
            transport=(python3 scripts/rust_tests.py --runner "${CARGO:-cargo}" test --locked)
            if [[ -n "${RODE_TEST_CARGO_PROFILE:-}" ]]; then transport+=(--profile "$RODE_TEST_CARGO_PROFILE"); fi
            transport+=(--test ssh_roundtrip "${RODE_SSH_TEST_FILTER:-}" -- --ignored --nocapture)
        fi
        if ! "${transport[@]}"; then
            # Keep the Rust assertion visible in the bounded CI output. The
            # complete daemon log is retained by cleanup in its own artifact.
            tail -n 80 "$test_dir/sshd.log" >&2
            exit 1
        fi
        ;;
    0)
        if [[ $# == 0 ]]; then
            echo 'Skipping transport requires an explicit command to run in the SSH fixture.' >&2
            exit 2
        fi
        ;;
    *) echo 'RODE_SSH_RUN_TRANSPORT must be 0 or 1.' >&2; exit 2 ;;
esac
# Batch a shard's UI cases under this one private SSH daemon. Existing flags
# below remain supported for standalone developer invocations.
if [[ $# != 0 ]]; then "$@"; fi
if [[ "${RODE_SSH_UI:-0}" == "1" ]]; then
    python3 scripts/test-project-ui.py
fi
if [[ "${RODE_FILE_BROWSER_UI:-0}" == "1" ]]; then
    python3 scripts/test-file-browser-ui.py
fi
if [[ "${RODE_WORKBENCH_UI:-0}" == "1" ]]; then
    python3 scripts/test-workbench-ui.py
fi
if [[ "${RODE_IMAGES_UI:-0}" == "1" ]]; then
    python3 scripts/test-images-ui.py
fi
if [[ "${RODE_MARKDOWN_UI:-0}" == "1" ]]; then
    python3 scripts/test-markdown-ui.py
fi

if [[ "${RODE_PDF_UI:-0}" == "1" ]]; then
    python3 scripts/test-pdf-ui.py
fi

if [[ "${RODE_BINARY_UI:-0}" == "1" ]]; then
    python3 scripts/test-binary-ui.py
fi

if [[ "${RODE_NOTEBOOK_UI:-0}" == "1" ]]; then
    python3 scripts/test-notebooks-ui.py
fi

if [[ "${RODE_MARP_UI:-0}" == "1" ]]; then
    python3 scripts/test-marp-ui.py
fi
if [[ "${RODE_LATEX_UI:-0}" == "1" ]]; then
    python3 scripts/test-latex-ui.py
fi
if [[ "${RODE_VAULT_UI:-0}" == "1" ]]; then
    python3 scripts/test-vaults-ui.py
fi

if [[ "${RODE_DEBUGGER_UI:-0}" == "1" ]]; then
    python3 scripts/test-debugger-ui.py
fi

if [[ "${RODE_NAVIGATION_UI:-0}" == "1" ]]; then
    python3 scripts/test-navigation-ui.py
fi

if [[ "${RODE_SHORTCUT_STYLES_UI:-0}" == "1" ]]; then
    python3 scripts/test-shortcut-styles-ui.py
fi
