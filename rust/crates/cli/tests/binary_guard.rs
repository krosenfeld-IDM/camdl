//! gh#105: the e2e test helpers must FAIL LOUD when the release `camdl`
//! binary is missing, not silently `return` (which reports a green pass
//! for a test that never ran). This is correct now that `make test`
//! builds the release binary first (gh#178) — a bare `cargo test` with no
//! build SHOULD fail loudly rather than silently pass.
//!
//! This file pins the *contract* of that guard: the assertion that every
//! helper now performs — present path → returns it; missing path → panics
//! with a message naming the path and the fix. The ~30 e2e helpers
//! (`camdl_bin()` / `skip_if_missing_binary()`) inline this same
//! `assert!`, so this is the canonical regression test for its message
//! and behaviour.

use std::path::{Path, PathBuf};

/// The guard every e2e helper inlines: a missing release binary is a hard
/// error, not a skip.
fn require_release_binary(p: &Path) -> PathBuf {
    assert!(
        p.exists(),
        "release camdl binary missing: {} - run `make build-rust` or `make test` (gh#105)",
        p.display()
    );
    p.to_path_buf()
}

#[test]
fn present_binary_is_returned() {
    // A path that always exists (the manifest dir itself) stands in for a
    // present binary: the guard must return it without panicking.
    let here = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let got = require_release_binary(&here);
    assert_eq!(got, here);
}

#[test]
#[should_panic(expected = "make build-rust")]
fn missing_binary_fails_loud_not_silent() {
    // A guaranteed-nonexistent path: the guard must PANIC (fail loud), the
    // pre-gh#105 behaviour being a silent `return` that falsely reports ok.
    let missing = PathBuf::from("/nonexistent/camdl/target/release/camdl");
    assert!(!missing.exists(), "test precondition: path must not exist");
    let _ = require_release_binary(&missing);
}

#[test]
#[should_panic(expected = "gh#105")]
fn missing_binary_message_cites_the_issue() {
    // The message must cite gh#105 so a future reader knows why the skip
    // was removed.
    let missing = PathBuf::from("/nonexistent/camdl/target/release/camdl");
    let _ = require_release_binary(&missing);
}
