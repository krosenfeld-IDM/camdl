//! gh#268: `camdl pfilter --save-prequential` must record the real observed
//! value as each step's `y_obs`, not a hardcoded `0.0`.
//!
//! The bug: the prequential time axis was built as `Observation { time, value:
//! 0.0 }` (a never-scored placeholder) and that zero was read into the
//! prequential trace's `y_obs`. The predictive samples are then scored against
//! zeros — silent garbage `log_score`/`crps`/`pit`, inherited by `camdl
//! compare`. Regressed by PR #218 (the sparse/multi-cadence union axis), so it
//! affects single-stream AND multi-stream models.
//!
//! These tests pin: `y_obs[step]` equals the bound observed value(s) — the
//! per-stream sum across all bound streams on the union axis (matching the
//! joint, cross-stream predictive sample the score is computed against).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Command;

fn binary() -> PathBuf {
    let manifest = std::env::var("CARGO_MANIFEST_DIR").unwrap();
    Path::new(&manifest).join("../../target/release/camdl")
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

fn multi_block_model() -> PathBuf {
    let manifest = std::env::var("CARGO_MANIFEST_DIR").unwrap();
    Path::new(&manifest).join("../../../ir/golden/seir_spatial_5_inference.ir.json")
}

fn single_block_model() -> PathBuf {
    let manifest = std::env::var("CARGO_MANIFEST_DIR").unwrap();
    Path::new(&manifest).join("../../../ir/golden/sir_vaccination.ir.json")
}

fn synth_obs(bin: &Path, model: &Path, tmp: &Path, extra_args: &[&str]) -> PathBuf {
    let obs_path = tmp.join("obs.tsv");
    let mut cmd = Command::new(bin);
    cmd.env("CAMDL_SKIP_VERSION_CHECK", "1").args([
        "simulate",
        &model.to_string_lossy(),
        "--backend",
        "chain_binomial",
        "--dt",
        "1",
        "--seed",
        "42",
        "--obs-only",
        &obs_path.to_string_lossy(),
    ]);
    cmd.args(extra_args);
    let status = cmd.status().expect("spawn simulate");
    assert!(status.success(), "synthetic obs generation failed");
    obs_path
}

/// Parse a TSV with a header row into time -> value maps for the named columns.
/// Returns a map keyed by the (rounded) time column for robust alignment.
fn read_tsv_columns(path: &Path, time_col: &str, value_cols: &[&str]) -> BTreeMap<i64, Vec<f64>> {
    let text = std::fs::read_to_string(path)
        .unwrap_or_else(|e| panic!("read {}: {}", path.display(), e));
    let mut lines = text.lines();
    let header: Vec<&str> = lines.next().expect("header").split('\t').collect();
    let tidx = header
        .iter()
        .position(|h| *h == time_col)
        .unwrap_or_else(|| panic!("no '{}' column in {:?}", time_col, header));
    let vidxs: Vec<usize> = value_cols
        .iter()
        .map(|c| {
            header
                .iter()
                .position(|h| h == c)
                .unwrap_or_else(|| panic!("no '{}' column in {:?}", c, header))
        })
        .collect();
    let mut out = BTreeMap::new();
    for line in lines {
        if line.trim().is_empty() {
            continue;
        }
        let f: Vec<&str> = line.split('\t').collect();
        let t: f64 = f[tidx].parse().expect("time parse");
        let vals: Vec<f64> = vidxs.iter().map(|&i| f[i].parse().expect("value parse")).collect();
        out.insert(t.round() as i64, vals);
    }
    out
}

#[test]
fn prequential_y_obs_single_stream_equals_data() {
    let bin = skip_if_missing_binary();
    let tmp = tempfile::tempdir().unwrap();
    let obs = synth_obs(&bin, &single_block_model(), tmp.path(), &[]);
    let stem = tmp.path().join("preq");

    let out = Command::new(&bin)
        .env("CAMDL_SKIP_VERSION_CHECK", "1")
        .args([
            "pfilter",
            &single_block_model().to_string_lossy(),
            "--data",
            &obs.to_string_lossy(),
            "--particles",
            "200",
            "--dt",
            "1",
            "--seed",
            "1",
            "--save-prequential",
            &stem.to_string_lossy(),
        ])
        .output()
        .expect("spawn pfilter");
    assert!(
        out.status.success(),
        "pfilter --save-prequential failed:\nstdout={}\nstderr={}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );

    let preq = read_tsv_columns(&stem.with_extension("tsv"), "t", &["y_obs"]);
    let data = read_tsv_columns(&obs, "time", &["reported_cases"]);
    assert!(!preq.is_empty(), "prequential trace empty");
    // The data must contain some nonzero observation, else the all-zero bug
    // could pass vacuously.
    assert!(
        data.values().any(|v| v[0] != 0.0),
        "synthetic obs are all zero — test would not detect the bug"
    );
    for (t, y) in &preq {
        let expected = data.get(t).unwrap_or_else(|| panic!("no data at t={}", t))[0];
        assert_eq!(
            y[0], expected,
            "prequential y_obs at t={} = {} but bound data = {} (the hardcoded-0 bug)",
            t, y[0], expected
        );
    }
}

#[test]
fn prequential_y_obs_multi_stream_is_cross_stream_sum() {
    let bin = skip_if_missing_binary();
    let tmp = tempfile::tempdir().unwrap();
    let obs = synth_obs(&bin, &multi_block_model(), tmp.path(), &["--scenario", "true_params"]);
    let stem = tmp.path().join("preq");

    // Bind two streams (their own distinct columns of the same wide obs file).
    let out = Command::new(&bin)
        .env("CAMDL_SKIP_VERSION_CHECK", "1")
        .args([
            "pfilter",
            &multi_block_model().to_string_lossy(),
            "--scenario",
            "true_params",
            "--data",
            &format!("cases_p1={}", obs.to_string_lossy()),
            "--data",
            &format!("cases_p2={}", obs.to_string_lossy()),
            "--particles",
            "200",
            "--dt",
            "1",
            "--seed",
            "1",
            "--save-prequential",
            &stem.to_string_lossy(),
        ])
        .output()
        .expect("spawn pfilter");
    assert!(
        out.status.success(),
        "multi-stream pfilter --save-prequential failed:\nstdout={}\nstderr={}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );

    let preq = read_tsv_columns(&stem.with_extension("tsv"), "t", &["y_obs"]);
    let data = read_tsv_columns(&obs, "time", &["cases_p1", "cases_p2"]);
    assert!(!preq.is_empty(), "prequential trace empty");
    assert!(
        data.values().any(|v| v[0] + v[1] != 0.0),
        "synthetic obs are all zero — test would not detect the bug"
    );
    for (t, y) in &preq {
        let d = data.get(t).unwrap_or_else(|| panic!("no data at t={}", t));
        let expected = d[0] + d[1];
        assert_eq!(
            y[0], expected,
            "joint prequential y_obs at t={} = {} but cases_p1+cases_p2 = {} \
             (the hardcoded-0 bug)",
            t, y[0], expected
        );
    }
}
