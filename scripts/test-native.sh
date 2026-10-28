#!/bin/sh
# The app-level suite on the native backend: every test file but the unit
# tests, without the tmux-only tests (tmux_only_*) and those that wait for
# ticket 42 (tests/harness/native_skip.rs). Arguments go to cargo test,
# before its `--`.
set -eu
cd "$(dirname "$0")/.."
skips=$(sed -n 's/.*"[a-z0-9_]*::\([a-z0-9_]*\)".*/--skip \1/p' tests/harness/native_skip.rs)
# One word per argument: the names hold no spaces.
# shellcheck disable=SC2086
PADDOCK_TEST_BACKEND=native exec cargo test --test '*' "$@" -- --skip tmux_only $skips
