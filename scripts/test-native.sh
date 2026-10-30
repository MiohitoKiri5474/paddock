#!/bin/sh
# The app-level suite: every test file but the unit tests, without those
# that wait for a ticket the native backend still needs
# (tests/harness/native_skip.rs, empty since ticket 42). Arguments go to
# cargo test, before its `--`.
set -eu
cd "$(dirname "$0")/.."
skips=$(sed -n 's/.*"[a-z0-9_]*::\([a-z0-9_]*\)".*/--skip \1/p' tests/harness/native_skip.rs)
# One word per argument: the names hold no spaces.
# shellcheck disable=SC2086
exec cargo test --test '*' "$@" -- $skips
