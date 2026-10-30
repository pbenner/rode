#!/usr/bin/env bash
# Build the small static Linux PTY helper for hosts without GUI/Rust packages.
set -euo pipefail
cd "$(dirname "$0")/.."
helper_arch=${1:-$(uname -m)}
case "$helper_arch" in
    x86_64|aarch64) ;;
    *) echo 'Supported Linux helper architectures: x86_64, aarch64' >&2; exit 2 ;;
esac
helper_target="$helper_arch-unknown-linux-musl"
"${RUSTUP:-rustup}" target add "$helper_target"
"${CARGO:-cargo}" build --locked -p rode-session --features standalone --bin rode-sessiond-standalone --release --target "$helper_target"
if [[ "$helper_arch" == "$(uname -m)" ]]; then
    mkdir -p target/release
    cp "target/$helper_target/release/rode-sessiond-standalone" target/release/rode-sessiond-static.tmp
    chmod 755 target/release/rode-sessiond-static.tmp
    mv target/release/rode-sessiond-static.tmp target/release/rode-sessiond-static
fi
