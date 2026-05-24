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
            "--fixed", "sigma,gamma,rho,k,p_detect,N0,I0",
            "--output", &out_tsv.to_string_lossy(),
            "--seed", "1",
        ])
        .status()
        .expect("spawn camdl profile pmmh");
    assert!(status.success(), "pmmh profile run failed");

    // Walk the seed_1 tree and check that every grid point has a
    // start_0/mle.toml + start_0/run.json with the PMMH algorithm
    // block.
    let profiles_dir = out_root.join("profiles");
    let entries: Vec<_> = std::fs::read_dir(&profiles_dir)
        .expect("profiles dir must exist")
        .filter_map(|e| e.ok())
        .collect();
    assert!(!entries.is_empty(), "no profile output written under {}",
        profiles_dir.display());

    // The umbrella is the only entry; under it sit replicates/seed_<n>/.
    let umbrella = entries.into_iter().next().unwrap().path();
    let seed_dirs: Vec<_> = std::fs::read_dir(umbrella.join("replicates"))
        .expect("replicates dir")
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .collect();
    assert_eq!(seed_dirs.len(), 1, "expected exactly one seed dir, got {:?}",
        seed_dirs);
    let seed_dir = &seed_dirs[0];

    let points_dir = seed_dir.join("points");
    let mut point_dirs: Vec<_> = std::fs::read_dir(&points_dir)
        .expect("points dir")
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .collect();
    point_dirs.sort();
    assert_eq!(point_dirs.len(), 2,
        "expected 2 grid points, got {:?}", point_dirs);

    for pd in &point_dirs {
        let start_dir = pd.join("start_0");
        let mle_toml = start_dir.join("mle.toml");
        assert!(mle_toml.exists(),
            "missing mle.toml under {}", start_dir.display());
        let body = std::fs::read_to_string(&mle_toml).unwrap();
        assert!(body.contains("final_loglik = "),
            "mle.toml missing final_loglik:\n{}", body);

        let run_json = start_dir.join("run.json");
        assert!(run_json.exists(),
            "missing run.json under {}", start_dir.display());
        let run_body = std::fs::read_to_string(&run_json).unwrap();
        let run: serde_json::Value = serde_json::from_str(&run_body)
            .expect("parse run.json");
        // Run.kind serializes as a tagged object: the FitStage payload
        // lives under run["kind"] with an internal `kind = "fit-stage"`
        // discriminator from `#[serde(tag = "kind", rename_all = "kebab-case")]`.
        let fs = run.get("kind").expect("run.kind block");
        assert_eq!(fs.get("kind").and_then(|v| v.as_str()),
            Some("fit-stage"),
            "expected kind.kind = fit-stage, got: {:?}", fs.get("kind"));
        assert_eq!(fs.get("method").and_then(|v| v.as_str()), Some("pmmh"),
            "method should be pmmh, got: {:?}", fs.get("method"));
        assert_eq!(fs.get("backend").and_then(|v| v.as_str()),
            Some("chain_binomial"),
            "backend should be chain_binomial, got: {:?}", fs.get("backend"));
        let alg = fs.get("algorithm").expect("algorithm block");
        assert_eq!(alg.get("steps").and_then(|v| v.as_u64()), Some(100),
            "algorithm.steps mismatch: {:?}", alg);
        assert_eq!(alg.get("particles").and_then(|v| v.as_u64()), Some(100),
            "algorithm.particles mismatch: {:?}", alg);
        let rho = alg.get("rho").expect("algorithm.rho");
        assert!(rho.is_f64() || rho.is_null(),
            "algorithm.rho expected float or null, got: {:?}", rho);
        let rho_v = rho.as_f64().expect("rho should be a finite float here");
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
            "--fixed", "sigma,gamma,rho,k,p_detect,N0,I0",
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
