#!/usr/bin/env bash
# Test the real SSH transport without using or modifying ~/.ssh.
set -euo pipefail
cd "$(dirname "$0")/.."
test_dir=$(mktemp -d /tmp/rode-ssh-test.XXXXXX)
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
ssh-keygen -q -t ed25519 -N '' -f "$test_dir/client"
test_port=$(python3 -c 'import socket; s=socket.socket(); s.bind(("127.0.0.1",0)); print(s.getsockname()[1]); s.close()')
test_user=$(id -un)
test_path=$PATH
test_shell_export=
# Keep sshd's X11 cookies in the fixture, even though its account database points
# at the real user's home. The forced command changes HOME only after xauth ran.
xauth_bin=$(command -v xauth || true)
if [[ -z "$xauth_bin" && "${RODE_TEST_REQUIRE_X11:-0}" == 1 ]]; then
    echo 'Install xauth for required X11 forwarding coverage.' >&2
    exit 1
fi
x11_config='X11Forwarding no'
client_xauth=
if [[ -n "$xauth_bin" ]]; then
    client_xauth="XAuthLocation \"$xauth_bin\""
    cat > "$test_dir/xauth" <<EOF
#!/bin/sh
exec '$xauth_bin' -f '$test_dir/home/.Xauthority' "\$@"
EOF
    chmod 700 "$test_dir/xauth"
    x11_config="X11Forwarding ${RODE_SSH_TEST_X11:-yes}
X11UseLocalhost yes
XAuthLocation $test_dir/xauth"
fi
if [[ "${RODE_SSH_TEST_NO_PYTHON:-0}" == 1 ]]; then
    mkdir -m 700 "$test_dir/native-path"
    for program in sh uname id stat sha256sum wc head mktemp ln chmod cut mkdir rm rmdir readlink git bwrap true env; do
        if [[ "$program" == bwrap && "${RODE_SSH_TEST_NO_BWRAP:-0}" == 1 ]]; then continue; fi
        program_path=$(command -v "$program" || true)
        if [[ "$program_path" == /* ]]; then ln -s "$program_path" "$test_dir/native-path/$program"; fi
    done
    test_path="$test_dir/native-path"
    # Login profiles belong to the machine, not this fixture, and can invoke
    # Python or consume terminal input. Use a private interactive shell with no
    # profile loading; the Rust sandbox fixture grants this exact script read-only.
    test_shell="$test_dir/project-shell"
    cat > "$test_shell" <<'EOF'
#!/bin/sh
unset ENV
exec /bin/sh -i
EOF
    chmod 700 "$test_shell"
    export RODE_TEST_PROJECT_SHELL="$test_shell"
    test_shell_export="export SHELL='$test_shell'"
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
fi
cat > "$test_dir/host-shell" <<EOF
#!/bin/sh
# Apply the fixture home after the account's initial shell startup files.
export HOME='$test_dir/home'
export PATH='$test_path'
export XAUTHORITY='$test_dir/home/.Xauthority'
$test_shell_export
if [ -n "\${SSH_ORIGINAL_COMMAND:-}" ]; then
    exec /bin/sh -c "\$SSH_ORIGINAL_COMMAND"
fi
exec /bin/sh
EOF
chmod 700 "$test_dir/host-shell"
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
# Parallel direct/ProxyJump tests exceed sshd's default ten-login throttle.
# Keep a bounded limit on this private loopback daemon without random drops.
MaxStartups 64
$x11_config
AcceptEnv CODEX_HOME RODE_CODEX_CACHE
# The fixture runs on this host; expose the same CI-installed tools to its child shells.
SetEnv "PATH=$test_path"
# Keep helper deployment and tool preferences out of the real user's home.
ForceCommand $test_dir/host-shell
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
    $client_xauth
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
export RODE_TEST_SSH_HOME="$test_dir/home"
export RODE_TEST_SSH_X11="${RODE_SSH_TEST_X11:-yes}"
if [[ -z "$xauth_bin" ]]; then export RODE_TEST_SSH_X11=no; fi
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
