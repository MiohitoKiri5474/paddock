//! tmux-only: raw tmux calls against the sandbox's server, for tests of
//! behaviour only the tmux backend has (its server's options and
//! environment, sessions made by an older paddock or by hand, zoom from
//! outside paddock). Every other test asks paddock (`Sandbox::state`), so
//! it holds for any backend.
//!
//! A tmux-only test's name starts with `tmux_only_`, so another backend's
//! run skips them all with `cargo test -- --skip tmux_only`. [`tmux`]
//! panics in a test not named so.

use super::Sandbox;

/// tmux-only: run tmux against the sandbox's server; its output. Only a
/// test whose name holds `tmux_only` may call it.
pub fn tmux(sandbox: &Sandbox, args: &[&str]) -> String {
    let thread = std::thread::current();
    // The test harness names each test's thread after it; with one test
    // thread it may run on `main` instead.
    let test = thread.name().unwrap_or("main");
    assert!(
        test == "main" || test.contains("tmux_only"),
        "{test} calls tmux: name it tmux_only_…"
    );
    sandbox.tmux(args)
}
