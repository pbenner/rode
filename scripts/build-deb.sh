#!/usr/bin/env bash
# Build a rootless Debian binary package, including the static terminal helper.
set -euo pipefail
cd "$(dirname "$0")/.."
exec python3 scripts/build-deb.py "$@"
