//! Integration round-trip for `camdl mre fit` (gh#212).
//!
//! Pack the committed fixture into a bundle, unpack it in an ISOLATED temp dir
//! with no path back to the repo, run the fit there, and assert it reproduces
//! the in-place run by `run_id`.
//!
//! `run_id` is a content hash of (model, data, config). Equality proves the
//! bundle's input closure is byte-identical and COMPLETE — in particular the
//! model's compile-time `read()` covariate table (`model/pop.tsv`), which
//! appears nowhere in `fit.toml` and is the file a hand-built bundle forgets.
//! A missing or altered read() file would shift the IR digest and diverge the
//! `run_id`; this test would catch it. (It is also the red→green proof for the
//! single-file `[data]` obs-extraction fix that made the bundled fit run at
//! all — without it both runs error with "model declares no observation
//! streams" and the test fails at the in-place run.)
//!
//! CLI-level: shells out to the built release `camdl` (which spawns `camdlc`),
//! so it runs under `make test` / `make test-rust` (camdlc on PATH). It
//! asserts (rather than silently skips) when the release binary is missing,
//! matching the other acceptance tests.

use std::path::{Path, PathBuf};
use std::process::Command;

fn binary() -> PathBuf {
    let manifest = std::env::var("CARGO_MANIFEST_DIR").unwrap();
    Path::new(&manifest).join("../../target/release/camdl")
}

fn fixture_dir() -> PathBuf {
    let manifest = std::env::var("CARGO_MANIFEST_DIR").unwrap();
    Path::new(&manifest)
        .join("../../../tests/fixtures/mre")
        .canonicalize()
        .expect("mre fixture dir")
}

fn skip_if_missing_binary() -> PathBuf {
    let bin = binary();
    assert!(
        bin.exists(),
        "release camdl binary missing: {} - run `make build-rust` or `make test`",
        bin.display()
    );
    bin
}

/// Every `run_id` found in a `run.json` under `root`, sorted (so two runs are
/// compared as sets, robust to extra umbrella leaves).
fn run_ids(root: &Path) -> Vec<String> {
    let mut ids = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let rj = dir.join("run.json");
        if rj.is_file() {
            if let Ok(txt) = std::fs::read_to_string(&rj) {
                if let Ok(v) = serde_json::from_str::<serde_json::Value>(&txt) {
                    if let Some(id) = v.get("run_id").and_then(|x| x.as_str()) {
                        ids.push(id.to_string());
                    }
                }
            }
        }
        if let Ok(es) = std::fs::read_dir(&dir) {
            for e in es.flatten() {
                let p = e.path();
                if p.is_dir() {
                    stack.push(p);
                }
            }
        }
    }
    ids.sort();
    ids
}

fn tar_list(bundle: &Path) -> Vec<String> {
    let out = Command::new("tar")
        .arg("tzf")
        .arg(bundle)
        .output()
        .expect("tar tzf");
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .map(str::to_string)
        .collect()
}

#[test]
fn mre_bundle_reproduces_fit_by_run_id() {
    let camdl = skip_if_missing_binary();
    let fixture = fixture_dir();
    let work = tempfile::tempdir().expect("tempdir");
    let bundle = work.path().join("b.tar.gz");

    // 1. pack the fixture
    let pack = Command::new(&camdl)
        .args(["mre", "fit"])
        .arg(fixture.join("fit.toml"))
        .arg("-b")
        .arg(&bundle)
        .output()
        .expect("spawn `mre fit`");
    assert!(
        pack.status.success(),
        "`mre fit` failed:\n{}",
        String::from_utf8_lossy(&pack.stderr)
    );
    assert!(bundle.exists(), "bundle not written");

    // The headline guarantee: the model's compile-time read() covariate table
    // is captured (it is named nowhere in fit.toml).
    let listing = tar_list(&bundle);
    assert!(
        listing.iter().any(|p| p.ends_with("model/pop.tsv")),
        "bundle missing the model's read() table (model/pop.tsv); contents:\n{}",
        listing.join("\n")
    );

    // 2. run the fit in place (from the project dir). Both this and the
    //    bundled run below use the canonical `cd <project> && fit run fit.toml`
    //    invocation — the `run_id` folds in the fit.toml-relative path *string*
    //    of the model/data, so addressing the same fit via a different relative
    //    path would change the id. The natural usage (run from the project
    //    dir) is location-independent, which is what makes the bundle
    //    reproduce; the test mirrors that.
    let out_a = work.path().join("out_a");
    let run_a = Command::new(&camdl)
        .args(["fit", "run", "fit.toml", "--seed", "1"])
        .current_dir(&fixture)
        .env("CAMDL_OUTPUT_DIR", &out_a)
        .output()
        .expect("spawn in-place `fit run`");
    assert!(
        run_a.status.success(),
        "in-place fit failed:\n{}",
        String::from_utf8_lossy(&run_a.stderr)
    );
    let ids_a = run_ids(&out_a);
    assert!(!ids_a.is_empty(), "in-place run produced no run.json");

    // 3. unpack the bundle into an ISOLATED dir (no path back to the repo)
    let iso = work.path().join("iso");
    std::fs::create_dir_all(&iso).unwrap();
    let untar = Command::new("tar")
        .arg("xzf")
        .arg(&bundle)
        .arg("-C")
        .arg(&iso)
        .output()
        .expect("untar");
    assert!(untar.status.success(), "untar failed");
    let bundle_root = iso.join("b"); // bundle_name = stem of `b.tar.gz`
    assert!(
        bundle_root.join("fit.toml").exists(),
        "unpacked fit.toml missing under {}",
        bundle_root.display()
    );

    // 4. run the fit from inside the unpacked bundle
    let out_b = work.path().join("out_b");
    let run_b = Command::new(&camdl)
        .args(["fit", "run", "fit.toml", "--seed", "1"])
        .current_dir(&bundle_root)
        .env("CAMDL_OUTPUT_DIR", &out_b)
        .output()
        .expect("spawn bundled `fit run`");
    assert!(
        run_b.status.success(),
        "bundled fit failed — bundle is NOT self-contained:\n{}",
        String::from_utf8_lossy(&run_b.stderr)
    );
    let ids_b = run_ids(&out_b);

    // 5. identity-faithful + complete closure
    assert_eq!(
        ids_a, ids_b,
        "bundle reproduced a DIFFERENT run_id — the input closure differs \
         (a missing or altered read() table would do exactly this)"
    );
}
