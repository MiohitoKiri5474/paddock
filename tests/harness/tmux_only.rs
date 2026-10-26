//! tmux-only: raw tmux calls against the sandbox's server, for tests of
//! behaviour only the tmux backend has (its server's options and
//! environment, sessions made by an older paddock or by hand, zoom from
//! outside paddock). Every other test asks paddock (`Sandbox::state`), so
//! it holds for any backend.
//!
//! A tmux-only test's name starts with `tmux_only_`, so another backend's
//! run skips them all with `cargo test -- --skip tmux_only`. A test that
//! calls [`tmux`] without that name fails in that run, as no tmux server
//! runs there: that is what catches an unmarked caller.

use super::Sandbox;

/// tmux-only: run tmux against the sandbox's server; its output. Only a
/// test named `tmux_only_…` calls it.
pub fn tmux(sandbox: &Sandbox, args: &[&str]) -> String {
    sandbox.tmux(args)
}
