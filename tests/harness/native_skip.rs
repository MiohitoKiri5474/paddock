//! The tests the native backend cannot pass yet, each with the tickets
//! whose part of the backend it needs; 41, 42 and 43 have landed, so none
//! is left. A ticket that lands takes its number off every entry, and
//! deletes the entries left with none.
//!
//! A native run skips them: `scripts/test-native.sh` turns each into a
//! `--skip` (libtest matches it as a substring of the test's name, in
//! every test binary; `tests/native_server.rs` checks no name here
//! matches a test not listed). A listed test run on native anyway fails
//! at once, naming its tickets ([`check`]).

/// `file::test` (the file under `tests/`), and the tickets it needs.
pub const NATIVE_NEEDS: &[(&str, &[&str])] = &[];

/// Fail the running test at once if it is listed: it needs tickets the
/// native backend does not have yet.
pub fn check() {
    // libtest names each test's thread after the test, with one test
    // thread too (`--test-threads=1`, Rust 1.93; checked by
    // `tests/native_server.rs`). Without a name this check cannot say,
    // and only the script's skips hold.
    let Some(test) = std::thread::current().name().map(str::to_owned) else {
        return;
    };
    if test == "main" {
        return;
    }
    let path = format!("{}::{test}", env!("CARGO_CRATE_NAME"));
    if let Some((_, tickets)) = NATIVE_NEEDS.iter().find(|(p, _)| *p == path) {
        panic!(
            "{path} needs ticket {} on the native backend: run the native suite with \
             scripts/test-native.sh, which skips it (tests/harness/native_skip.rs)",
            tickets.join(" and ")
        );
    }
}
