#!/usr/bin/env bash
# Test the real SSH transport without using or modifying ~/.ssh.
set -euo pipefail
cd "$(dirname "$0")/.."
test_dir=$(mktemp -d /tmp/rode-ssh-test.XXXXXX)
mkdir -m 700 "$test_dir/codex-home"
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
ssh-keygen -q -t ed25519 -N '' -f "$test_dir/client"
test_port=$(python3 -c 'import socket; s=socket.socket(); s.bind(("127.0.0.1",0)); print(s.getsockname()[1]); s.close()')
test_user=$(id -un)
cat > "$test_dir/sshd_config" <<EOF
Port $test_port
ListenAddress 127.0.0.1
HostKey $test_dir/host
PidFile $test_dir/sshd.pid
AuthorizedKeysFile $test_dir/client.pub
StrictModes no
PasswordAuthentication no
KbdInteractiveAuthentication no
UsePAM no
AllowUsers $test_user
LogLevel VERBOSE
AcceptEnv CODEX_HOME RODE_CODEX_CACHE
# The fixture runs on this host; expose the same CI-installed tools to its child shells.
SetEnv "PATH=$PATH"
EOF
read -r key_type key_data _ < "$test_dir/host.pub"
printf '[127.0.0.1]:%s %s %s\n' "$test_port" "$key_type" "$key_data" > "$test_dir/known_hosts"
cat > "$test_dir/ssh_config" <<EOF
Host rode-test rode-jump
    HostName 127.0.0.1
    User $test_user
    Port $test_port
    IdentityFile $test_dir/client
    IdentitiesOnly yes
    UserKnownHostsFile $test_dir/known_hosts
    StrictHostKeyChecking yes
    BatchMode yes
    SetEnv CODEX_HOME=$test_dir/codex-home RODE_CODEX_CACHE=$test_dir/codex-cache
EOF
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
export RODE_TEST_SSH_CONFIG="$test_dir/ssh_config"
export RODE_TEST_CODEX_REMOTE_HOME="$test_dir/codex-home"
case "${RODE_SSH_RUN_TRANSPORT:-1}" in
    1)
        if [[ -n "${RODE_SSH_TEST_BINARY:-}" ]]; then
            transport=("$RODE_SSH_TEST_BINARY" "${RODE_SSH_TEST_FILTER:-}" --ignored --nocapture)
        else
            transport=("${CARGO:-cargo}" test --locked)
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
    RODE_TEST_SSH_CONFIG="$test_dir/ssh_config" python3 scripts/test-project-ui.py
fi
if [[ "${RODE_FILE_BROWSER_UI:-0}" == "1" ]]; then
    RODE_TEST_SSH_CONFIG="$test_dir/ssh_config" python3 scripts/test-file-browser-ui.py
fi
if [[ "${RODE_WORKBENCH_UI:-0}" == "1" ]]; then
    RODE_TEST_SSH_CONFIG="$test_dir/ssh_config" python3 scripts/test-workbench-ui.py
fi
if [[ "${RODE_IMAGES_UI:-0}" == "1" ]]; then
    RODE_TEST_SSH_CONFIG="$test_dir/ssh_config" python3 scripts/test-images-ui.py
fi
if [[ "${RODE_MARKDOWN_UI:-0}" == "1" ]]; then
    RODE_TEST_SSH_CONFIG="$test_dir/ssh_config" python3 scripts/test-markdown-ui.py
fi

if [[ "${RODE_PDF_UI:-0}" == "1" ]]; then
    RODE_TEST_SSH_CONFIG="$test_dir/ssh_config" python3 scripts/test-pdf-ui.py
fi

if [[ "${RODE_BINARY_UI:-0}" == "1" ]]; then
    RODE_TEST_SSH_CONFIG="$test_dir/ssh_config" python3 scripts/test-binary-ui.py
fi

if [[ "${RODE_NOTEBOOK_UI:-0}" == "1" ]]; then
    RODE_TEST_SSH_CONFIG="$test_dir/ssh_config" python3 scripts/test-notebooks-ui.py
fi

if [[ "${RODE_MARP_UI:-0}" == "1" ]]; then
    RODE_TEST_SSH_CONFIG="$test_dir/ssh_config" python3 scripts/test-marp-ui.py
fi
if [[ "${RODE_LATEX_UI:-0}" == "1" ]]; then
    RODE_TEST_SSH_CONFIG="$test_dir/ssh_config" python3 scripts/test-latex-ui.py
fi
if [[ "${RODE_VAULT_UI:-0}" == "1" ]]; then
    RODE_TEST_SSH_CONFIG="$test_dir/ssh_config" python3 scripts/test-vaults-ui.py
fi

if [[ "${RODE_DEBUGGER_UI:-0}" == "1" ]]; then
    RODE_TEST_SSH_CONFIG="$test_dir/ssh_config" python3 scripts/test-debugger-ui.py
fi

if [[ "${RODE_NAVIGATION_UI:-0}" == "1" ]]; then
    RODE_TEST_SSH_CONFIG="$test_dir/ssh_config" python3 scripts/test-navigation-ui.py
fi

if [[ "${RODE_SHORTCUT_STYLES_UI:-0}" == "1" ]]; then
    RODE_TEST_SSH_CONFIG="$test_dir/ssh_config" python3 scripts/test-shortcut-styles-ui.py
fi
