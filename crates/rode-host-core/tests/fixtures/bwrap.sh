#!/bin/sh
# Deterministic argument-file/stdin peer shared by stdio and PTY tests.
set -eu
cat /proc/$$/cmdline > "$ARGUMENT_LOG"
if [ "$1" = --bwrap-launch ]; then shift; fi
[ "$1" = --args ]
cat "/proc/self/fd/$2"
shift 2
printf '%s\000' "$@"
cat
