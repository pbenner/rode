#!/usr/bin/env bash
# Install only the tools used by this CI job; never invoked by the application.
set -euo pipefail
cd "$(dirname "$0")/.."
suite=${1:?Expected build, lint or a UI suite name}
packages=(libxkbcommon-x11-0 libegl1 libegl-mesa0 libglx-mesa0 libgl1-mesa-dri)
case "$suite" in
    build|lint)
        packages+=(libxkbcommon-dev libwayland-dev libgl1-mesa-dev)
        ;;
    *)
        packages+=(xvfb xauth x11-utils xclip imagemagick libxtst6 bubblewrap poppler-utils)
        ;;
esac
if [[ "$suite" == build ]]; then packages+=(musl-tools poppler-utils bubblewrap); fi
case "$suite" in
    ssh|remote|runtimes) packages+=(openssh-server) ;;
esac
if [[ "$suite" == ssh ]]; then packages+=(krb5-kdc krb5-admin-server krb5-user); fi
case "$suite" in
    workspace|ssh) packages+=(openssl) ;; # HTTPS Git authentication fixtures.
esac
case "$suite" in
    tools|runtimes)
        packages+=(clang-format shfmt)
        # mise-action provisions Go before this script. Do not fall back to
        # Ubuntu's large SDK/source packages when a runner's PATH is incomplete.
        if ! command -v go >/dev/null 2>&1 || ! command -v gofmt >/dev/null 2>&1; then
            echo 'Missing Go or gofmt. Run mise install --locked go, then run this script through mise exec.' >&2
            exit 1
        fi
        go version
        ;;
esac
case "$suite" in
    documents|remote|runtimes) packages+=(gocryptfs) ;;
esac
if [[ "$suite" == runtimes ]]; then packages+=(lldb-19); fi
# Bound stalled connections/reads inside the existing 600-second CI deadline.
# APT retries individual downloads, retaining completed archives; do not retry
# dpkg installation or silently continue with incomplete package indexes.
apt_options=(
    -o Acquire::Retries=3
    -o Acquire::http::Timeout=30
    -o Acquire::https::Timeout=30
    -o DPkg::Lock::Timeout=60
    -o APT::Update::Error-Mode=any
)
sudo env DEBIAN_FRONTEND=noninteractive apt-get "${apt_options[@]}" update
sudo env DEBIAN_FRONTEND=noninteractive apt-get "${apt_options[@]}" install -y --no-install-recommends "${packages[@]}"
if [[ "$suite" == tools || "$suite" == runtimes ]]; then
    npm install --prefix target/language-tools --ignore-scripts --no-audit --no-fund prettier@3.6.2
fi
if [[ "$suite" == runtimes ]]; then
    scripts/uvx.sh --from ruff==0.17.3 python - <<'PY'
from pathlib import Path
import shutil
destination = Path('target/test-tools/bin/ruff')
destination.parent.mkdir(parents=True, exist_ok=True)
shutil.copy2(shutil.which('ruff'), destination)
PY
    uv venv --python python3 target/notebook-env
    uv pip install --python target/notebook-env/bin/python \
        jupyter-client==8.10.0 ipykernel==7.4.0 matplotlib==3.11.2
fi
# Export optional tools only when they are installed, so missing coverage is
# visible in the ordinary lane and exercised explicitly in the runtime lane.
if [[ -n "${GITHUB_ENV:-}" ]]; then
    if [[ "$suite" == tools || "$suite" == runtimes ]]; then
        echo "RODE_TEST_PRETTIER=$PWD/target/language-tools/node_modules/.bin/prettier" >> "$GITHUB_ENV"
    fi
    if [[ "$suite" == runtimes ]]; then
        echo "RODE_TEST_RUFF=$PWD/target/test-tools/bin/ruff" >> "$GITHUB_ENV"
        echo "RODE_TEST_LLDB=/usr/bin/lldb-dap-19" >> "$GITHUB_ENV"
        echo "RODE_TEST_JUPYTER=$PWD/target/notebook-env/bin/python" >> "$GITHUB_ENV"
    fi
fi
