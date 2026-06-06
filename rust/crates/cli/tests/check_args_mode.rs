//! Pins the contract of the hidden `camdl __check-args -- <argv...>`
//! parse-only mode that backs `make test-cli-docs`.
//!
//! The mode runs ONLY clap parsing against the real `Cli` command tree —
//! no file I/O, no compilation, no simulation — and exits:
//!   0  the surface parses (subcommand + flags + arg shape are real), OR
//!      clap wants help/version, OR a required positional is merely missing
//!      (an input concern, not surface drift).
//!   2  surface DRIFT: unknown subcommand / unrecognized flag / unexpected
//!      positional / bad arg count / invalid enum value.
//!
//! This is the binary-level half of the non-vacuous drift gate: it proves the
//! exit-code contract directly, independent of the shell extractor in
//! `scripts/check_cli_docs.sh` (whose `--selftest` proves the same contract
//! through the doc-extraction path).

use std::path::{Path, PathBuf};
use std::process::Command;

fn camdl_bin() -> PathBuf {
    let manifest = std::env::var("CARGO_MANIFEST_DIR")
        .expect("CARGO_MANIFEST_DIR set under cargo test");
    let p = Path::new(&manifest).join("../../target/release/camdl");
    assert!(
        p.exists(),
        "release camdl binary missing: {} - run `make build-rust` or `make test` (gh#105)",
        p.display()
    );
    p
}

/// Run `camdl __check-args -- <args>` and return the exit code.
fn check(args: &[&str]) -> i32 {
    let out = Command::new(camdl_bin())
        .arg("__check-args")
        .arg("--")
        .args(args)
        .output()
        .expect("spawn camdl __check-args");
    out.status.code().expect("process exited with a code")
}

// ── Valid surface → exit 0 (EXPECTED input failures are NOT drift) ───────────

#[test]
fn valid_surface_with_missing_inputs_is_ok() {
    // Every one of these references a real subcommand + real flags; the only
    // thing "wrong" is that the files don't exist. The parse-only check never
    // touches the filesystem, so these MUST parse clean.
    assert_eq!(check(&["simulate", "/no/such/model.camdl", "--seed", "1"]), 0);
    assert_eq!(check(&["fit", "run", "/no/such/fit.toml", "--seed", "1"]), 0);
    assert_eq!(check(&["fit", "summary", "/no/such/dir"]), 0);
    assert_eq!(check(&["pfilter", "m.camdl", "--params", "p.toml",
                       "--data", "d.tsv", "--particles", "5000"]), 0);
    assert_eq!(check(&["compare", "a", "b"]), 0);
    assert_eq!(check(&["survey", "m.camdl", "--fit", "f.toml", "--render"]), 0);
    assert_eq!(check(&["simulate", "m.camdl", "--draws", "prior",
                       "--fit", "f.toml", "-n", "200", "--obs", "ppc.tsv"]), 0);
}

#[test]
fn missing_required_positional_is_not_drift() {
    // The subcommand is recognized; a missing required positional is an input
    // concern, not surface drift. (E.g. a doc snippet that omits the path.)
    assert_eq!(check(&["fit", "summary"]), 0);
}

#[test]
fn camdlc_passthrough_subcommands_accept_their_own_flags() {
    // compile/check/inspect forward verbatim to camdlc, so clap accepts any
    // tail after them. Documented scope limit: those flags are camdlc's
    // surface, not camdl's. Must parse OK here.
    assert_eq!(check(&["check", "model.camdl"]), 0);
    assert_eq!(check(&["compile", "model.camdl", "--set", "beta=0.3",
                       "--json-errors"]), 0);
    assert_eq!(check(&["inspect", "model.camdl", "--tables"]), 0);
}

#[test]
fn subcommand_alias_resolves() {
    // `sim` is an alias for `simulate`; aliases are real surface.
    assert_eq!(check(&["sim", "m.camdl", "--seed", "1"]), 0);
}

// ── Surface DRIFT → exit 2 (the NON-VACUOUS negative test) ───────────────────

#[test]
fn unknown_subcommand_is_drift() {
    assert_eq!(check(&["frobnicate", "foo"]), 2);
}

#[test]
fn unrecognized_flag_is_drift() {
    assert_eq!(check(&["simulate", "model.camdl", "--no-such-flag"]), 2);
}

#[test]
fn unknown_fit_subcommand_is_drift() {
    assert_eq!(check(&["fit", "bogus", "fit.toml"]), 2);
}

#[test]
fn invalid_enum_value_is_drift() {
    // --backend takes a typed enum; a bogus value is a parse-layer rejection.
    assert_eq!(check(&["simulate", "model.camdl", "--backend", "not_a_backend"]), 2);
}

#[test]
fn flag_on_wrong_subcommand_is_drift() {
    // `--particles` belongs to pfilter/profile/survey, not simulate.
    assert_eq!(check(&["simulate", "model.camdl", "--particles", "10"]), 2);
}
