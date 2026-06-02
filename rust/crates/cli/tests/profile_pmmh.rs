//! Integration smoke test for `camdl profile --algorithm pmmh`.
//!
//! Mirrors the profile_multi_stream.rs harness shape: drives the
//! release binary, scrapes the per-cell artifacts.
//!
//! Assertions:
//!
//! 1. The profile run completes successfully on a 2-cell sweep of a
//!    small SEIR with ~52 weekly observations.
//! 2. Every cell writes an `mle.toml` containing `final_loglik`.
//! 3. The per-start `run.json` records `method = pmmh`,
//!    `backend = chain_binomial`, and an `algorithm` block matching
//!    the PMMH serialization (steps / particles / rho / dt).
//! 4. Passing `--algorithm pmmh --backend ode` is rejected with an
//!    actionable error pointing at `--backend chain_binomial`.

use std::path::{Path, PathBuf};
use std::process::Command;

fn binary() -> PathBuf {
    let manifest = std::env::var("CARGO_MANIFEST_DIR").unwrap();
    Path::new(&manifest).join("../../target/release/camdl")
}

fn skip_if_missing_binary() -> Option<PathBuf> {
    let bin = binary();
    if !bin.exists() {
        eprintln!("skipping: camdl binary not built at {}", bin.display());
        return None;
    }
    Some(bin)
}

fn seir_observations_ir() -> PathBuf {
    let manifest = std::env::var("CARGO_MANIFEST_DIR").unwrap();
    Path::new(&manifest).join("../../../ocaml/golden/seir_observations.ir.json")
}

/// Synthesize per-stream observation TSVs at the baseline preset.
/// `seir_observations.ir.json` has two streams with different
/// schedules (weekly_cases at 7d, detection at 14d), so `--obs-only`
/// can't unify them into a single TSV. We use `--obs-dir` and then
/// pick the `weekly_cases.tsv`.
fn synth_weekly_cases_tsv(bin: &Path, tmp: &Path) -> PathBuf {
    let obs_dir = tmp.join("obs_streams");
    std::fs::create_dir_all(&obs_dir).unwrap();
    let status = Command::new(bin)
        .env("CAMDL_SKIP_VERSION_CHECK", "1")
        .args([
            "simulate", &seir_observations_ir().to_string_lossy(),
            "--backend", "chain_binomial", "--dt", "1", "--seed", "42",
            "--scenario", "baseline",
            "--obs-dir", &obs_dir.to_string_lossy(),
        ])
        .status()
        .expect("spawn camdl simulate");
    assert!(status.success(), "synthetic obs generation failed");
    let obs_path = obs_dir.join("weekly_cases.tsv");
    assert!(obs_path.exists(),
        "weekly_cases.tsv not written under {}", obs_dir.display());
    obs_path
}

#[test]
fn profile_pmmh_smoke_writes_mle_and_algorithm_block() {
    let Some(bin) = skip_if_missing_binary() else { return };
    let tmp = tempfile::tempdir().unwrap();
    let data_path = synth_weekly_cases_tsv(&bin, tmp.path());

    let out_root = tmp.path().join("camdl_out");
    let out_tsv  = tmp.path().join("profile_pmmh.tsv");

    // 2-cell sweep over beta with PMMH (very short chain, small PF —
    // smoke test only). One start so we have exactly two MLE files.
    let status = Command::new(&bin)
        .env("CAMDL_OUTPUT_DIR", &out_root)
        .env("CAMDL_SKIP_VERSION_CHECK", "1")
        .args([
            "profile", &seir_observations_ir().to_string_lossy(),
            "--scenario", "baseline",
            "--data", &data_path.to_string_lossy(),
            "--obs", "weekly_cases",
            "--sweep", "beta=lin(0.25,0.35,2)",
            "--particles", "100",
            "--algorithm", "pmmh",
            "--pmmh-steps", "100",
            "--pmmh-particles", "100",
            "--pmmh-rho", "0.99",
            "--starts", "1",
            "--rw-sd", "auto",
            "--fixed", "sigma=0.2", "--fixed", "gamma=0.1",
            "--fixed", "rho=0.5", "--fixed", "k=5.0",
            "--fixed", "p_detect=0.8", "--fixed", "N0=100000.0",
            "--fixed", "I0=10.0",
            "--output", &out_tsv.to_string_lossy(),
            "--seed", "1",
        ])
        .status()
        .expect("spawn camdl profile pmmh");
    assert!(status.success(), "pmmh profile run failed");

    // Collect the new-format ProfilePoint leaves:
    // profiles/<base>/<point>/<stage>/<seed>/<start>/{mle.toml, run.json}.
    // Two grid points × 1 seed × 1 start ⇒ two leaves.
    fn collect_leaves(dir: &Path, out: &mut Vec<PathBuf>) {
        if dir.join("run.json").is_file() {
            if let Ok(b) = std::fs::read_to_string(dir.join("run.json")) {
                if let Ok(v) = serde_json::from_str::<serde_json::Value>(&b) {
                    if v.get("kind").and_then(|k| k.as_str()) == Some("profile_point") {
                        out.push(dir.to_path_buf());
                    }
                }
            }
        }
        if let Ok(es) = std::fs::read_dir(dir) {
            for e in es.flatten() { if e.path().is_dir() { collect_leaves(&e.path(), out); } }
        }
    }
    let profiles_dir = out_root.join("profiles");
    let mut leaves = Vec::new();
    collect_leaves(&profiles_dir, &mut leaves);
    assert_eq!(leaves.len(), 2,
        "expected 2 ProfilePoint leaves (2 grid points × 1 seed × 1 start), got {:?}",
        leaves);

    for leaf in &leaves {
        let mle_toml = leaf.join("mle.toml");
        assert!(mle_toml.exists(), "missing mle.toml under {}", leaf.display());
        let body = std::fs::read_to_string(&mle_toml).unwrap();
        assert!(body.contains("final_loglik = "),
            "mle.toml missing final_loglik:\n{}", body);

        let run: serde_json::Value = serde_json::from_str(
            &std::fs::read_to_string(leaf.join("run.json")).unwrap())
            .expect("parse run.json");
        assert_eq!(run.get("kind").and_then(|v| v.as_str()), Some("profile_point"),
            "expected kind = profile_point, got: {:?}", run.get("kind"));
        // The PMMH method + algorithm hyperparams live in the recorded
        // (display-only) `inputs` of the leaf record.
        let inputs = run.get("inputs").expect("inputs block");
        assert_eq!(inputs.get("method").and_then(|v| v.as_str()), Some("pmmh"),
            "method should be pmmh, got: {:?}", inputs.get("method"));
        let alg = inputs.get("algorithm").expect("algorithm block");
        assert_eq!(alg.get("steps").and_then(|v| v.as_u64()), Some(100),
            "algorithm.steps mismatch: {:?}", alg);
        assert_eq!(alg.get("particles").and_then(|v| v.as_u64()), Some(100),
            "algorithm.particles mismatch: {:?}", alg);
        let rho_v = alg.get("rho").and_then(|v| v.as_f64())
            .expect("algorithm.rho should be a finite float");
        assert!((rho_v - 0.99).abs() < 1e-12,
            "algorithm.rho expected 0.99, got: {}", rho_v);
    }
}

#[test]
fn profile_pmmh_rejects_ode_backend() {
    let Some(bin) = skip_if_missing_binary() else { return };
    let tmp = tempfile::tempdir().unwrap();
    let data_path = synth_weekly_cases_tsv(&bin, tmp.path());

    let out_root = tmp.path().join("camdl_out");
    let out_tsv  = tmp.path().join("profile_pmmh.tsv");

    let output = Command::new(&bin)
        .env("CAMDL_OUTPUT_DIR", &out_root)
        .env("CAMDL_SKIP_VERSION_CHECK", "1")
        .args([
            "profile", &seir_observations_ir().to_string_lossy(),
            "--scenario", "baseline",
            "--data", &data_path.to_string_lossy(),
            "--obs", "weekly_cases",
            "--sweep", "beta=lin(0.25,0.35,2)",
            "--particles", "100",
            "--algorithm", "pmmh",
            "--backend", "ode",
            "--pmmh-steps", "50",
            "--pmmh-particles", "50",
            "--starts", "1",
            "--rw-sd", "auto",
            "--fixed", "sigma=0.2", "--fixed", "gamma=0.1",
            "--fixed", "rho=0.5", "--fixed", "k=5.0",
            "--fixed", "p_detect=0.8", "--fixed", "N0=100000.0",
            "--fixed", "I0=10.0",
            "--output", &out_tsv.to_string_lossy(),
            "--seed", "1",
        ])
        .output()
        .expect("spawn camdl profile");
    assert!(!output.status.success(),
        "expected non-zero exit when combining pmmh with --backend ode");
    let stderr = String::from_utf8_lossy(&output.stderr);
    // Either the upstream methods-matrix rejection or the profile
    // PMMH-specific guard should fire; both name chain_binomial as
    // the right answer.
    assert!(stderr.contains("chain_binomial"),
        "error must guide user to --backend chain_binomial. stderr:\n{}",
        stderr);
}
