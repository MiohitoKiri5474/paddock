//! tmux-only: raw tmux calls against the sandbox's server, for tests of
//! behaviour only the tmux backend has (its server's options and
//! environment, sessions made by an older paddock or by hand, zoom from
//! outside paddock). Every other test asks paddock (`Sandbox::state`), so
//! it holds for any backend; a test that calls this is tmux-only.

use super::Sandbox;

/// tmux-only: run tmux against the sandbox's server; its output.
pub fn tmux(sandbox: &Sandbox, args: &[&str]) -> String {
    sandbox.tmux(args)
}

/// tmux-only: socket files tmux created in the sandbox.
pub fn sockets(sandbox: &Sandbox) -> Vec<String> {
    let Ok(dirs) = std::fs::read_dir(sandbox.tmux_tmpdir()) else {
        return vec![];
    };
    dirs.flatten()
        .filter_map(|d| std::fs::read_dir(d.path()).ok())
        .flatten()
        .flatten()
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .collect()
}
