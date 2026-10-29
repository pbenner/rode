#!/bin/sh
# Keep the tool registry writable even when ~/.local is mounted read-only.
set -eu
rode_root=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
export UV_TOOL_DIR="${UV_TOOL_DIR:-$rode_root/target/uv-tools}"
exec uvx "$@"
