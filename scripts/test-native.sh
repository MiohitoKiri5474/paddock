#!/bin/sh
# The app-level suite, as CI runs it: every test file but the unit tests,
# on paddock's own server. Arguments go to cargo test, before its `--`.
set -eu
cd "$(dirname "$0")/.."
exec cargo test --test '*' "$@"
