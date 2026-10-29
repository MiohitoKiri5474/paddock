# Develop

Building and testing paddock. Back to the [README](../README.md).

```sh
cargo clippy --all-targets -- -D warnings
cargo test                          # unit + end-to-end tests on the tmux backend (real tmux on a temp socket)
scripts/test-native.sh              # the end-to-end tests on the native backend (the default)
claude plugin validate --strict .   # the plugin marketplace, in full (cargo test checks the basics)
claude plugin validate --strict plugins/paddock
gemini extensions validate .        # the Gemini extension (agy installs it too)
```

A release bumps the version in `Cargo.toml` (and `Cargo.lock`), `plugins/paddock/.claude-plugin/plugin.json` and `gemini-extension.json` together; `cargo test` fails while either manifest differs from `Cargo.toml`.

CI runs the unit tests, then the end-to-end tests on each backend (native, and tmux with its tmux-only tests), on macOS, then the unit tests and the native end-to-end tests on Linux (x86_64), and uploads a release binary when the unit tests pass. Pushing a `v*` tag that matches the version in `Cargo.toml` builds the tarballs (Apple Silicon macOS, and x86_64 and aarch64 Linux, each on its own architecture, Linux on Ubuntu 22.04 to keep the glibc floor at 2.34) and opens one draft pre-release with them. Running the Release workflow by hand (`workflow_dispatch`) builds, tests and packages them all as workflow artifacts, with no release.
