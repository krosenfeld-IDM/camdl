//! Integration tests for the `camdl profile` prior-resolution surface
//! (gh#73). Each test drives the release binary end-to-end and asserts
//! observable behaviour: warning text, `run.json` provenance fields,
//! CAS-dir hashes. The unit-level counterpart in
//! `crate::profile_priors::tests` exercises the precedence helper on
//! synthetic `ir::Model` fixtures.
//!
//! Assertions covered:
//!
//!   1. `profile_pmmh_with_neither_warns_and_uses_flat`: model has no
//!      `~` priors, no `--fit`; warning fires on stderr naming every
//!      estimated parameter; `run.json` records every estimated param
//!      as `flat_fallback`.
//!   2. `profile_pmmh_with_fit_toml_silences_flat_warning`: same
//!      model, but a fit toml supplies priors for every estimated
//!      param; no warning; `run.json` records source = `fit_toml` per
//!      param.
//!   3. `run_json_records_resolved_prior_sources`: explicit shape
//!      assertion on the `resolved_priors` array.
//!   4. `same_model_different_fit_toml_different_cas_dir`: hash
//!      provenance — two distinct fit tomls produce two distinct CAS
//!      dirs (different `fit_toml_hash` → different inner_hash).
//!   5. `same_model_no_fit_vs_with_fit_different_cas_dir`: same
//!      provenance with the "no fit" baseline as one of the variants.
//!
//! Skipped when the release binary or the `camdlc` compiler isn't
//! present (mirrors the rest of the integration suite).

use std::path::{Path, PathBuf};
use std::process::Command;

fn camdl_bin() -> Option<PathBuf> {
    let manifest = std::env::var("CARGO_MANIFEST_DIR").ok()?;
    let p = Path::new(&manifest).join("../../target/release/camdl");
    if p.exists() { Some(p) } else { None }
}

fn camdlc_bin() -> Option<PathBuf> {
    let manifest = std::env::var("CARGO_MANIFEST_DIR").ok()?;
    let p = Path::new(&manifest).join("../../../ocaml/_build/default/bin/camdlc.exe");
    if p.exists() { Some(p) } else { None }
}

struct Tmp(PathBuf);
impl Tmp { fn path(&self) -> &Path { &self.0 } }
impl Drop for Tmp { fn drop(&mut self) { let _ = std::fs::remove_dir_all(&self.0); } }
fn tempdir(tag: &str) -> Tmp {
    let ns = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos();
    let base = std::env::temp_dir().join(format!(
        "camdl_profile_priors_{}_{}_{}", tag, std::process::id(), ns));
    std::fs::create_dir_all(&base).unwrap();
    Tmp(base)
}

/// Build a tiny SIR-with-Poisson-cases fixture. Two estimated params
/// (`beta`, `gamma`); `N0` is fixed via the toml or CLI as needed.
/// No `~` priors in the model file so the resolver's flat-fallback
/// case fires when `--fit` is absent. Matches the `survey_top_k_pmmh`
/// fixture shape so the fit toml schema is well-trodden.
fn write_fixture(dir: &Path) -> (PathBuf, PathBuf) {
    let camdlc = camdlc_bin().expect("camdlc.exe present");
    // Defaults supplied via a `baseline` preset block so the
    // `no --fit / no --params` test path has values to start from
    // (the simulator validates that every parameter has a value).
    // The `--fit toml` path overrides via [estimate].start; the
    // resolver's prior-source assertion is what each test cares
    // about, not the starting value.
    let src = r#"
time_unit = 'days
compartments { S, I, R }
parameters {
  beta  : rate  in [0.001, 5.0]
  gamma : rate  in [0.01, 1.0]
  N0    : count in [100, 10000]
}
transitions {
  infection : S --> I @ beta * S * I / N0
  recovery  : I --> R @ gamma * I
}
observations {
  cases : {
    projected  = prevalence(I)
    every      = 1 'days
    likelihood = poisson(rate = projected)
  }
}
scenarios {
  baseline {
    set = {
      beta  = 0.3
      gamma = 0.1
      N0    = 1000
    }
  }
}
init { S = 999  I = 1 }
simulate { from = 0 'days  to = 6 'days }
"#;
    let model_path = dir.join("sir.camdl");
    std::fs::write(&model_path, src).unwrap();
    let ir_path = dir.join("sir.ir.json");
    let out = Command::new(&camdlc).arg(&model_path).output().unwrap();
    assert!(out.status.success(),
        "camdlc failed: {}", String::from_utf8_lossy(&out.stderr));
    std::fs::write(&ir_path, &out.stdout).unwrap();

    let data_path = dir.join("cases.tsv");
    std::fs::write(&data_path,
        "time\tcases\n1\t2\n2\t4\n3\t8\n4\t6\n5\t4\n6\t2\n").unwrap();

    (ir_path, data_path)
}

/// Write a fit toml with [estimate] containing log_normal priors for
/// every estimated param. The `[stages.dummy]` block satisfies
/// `FitConfigV2::load`'s schema check — profile never *runs* the
/// stages, it only reads `[estimate]` and `[fixed]` for prior /
/// bounds resolution (the v2 schema requires at least one stage to
/// be declared; we treat that as a fixable schema burden rather than
/// an excuse to fork the loader).
fn write_fit_toml_with_priors(dir: &Path, ir: &Path, data: &Path, name: &str) -> PathBuf {
    let toml = format!(r#"
output_dir = "{out}"
[model]
camdl = "{ir}"
[data.observations]
cases = "{data}"
[config]
backend = "chain_binomial"
dt = 1.0
[estimate]
beta  = {{ bounds = [0.01, 5.0], prior = {{ log_normal = {{ mu = -0.3, sigma = 0.5 }} }}, start = 0.3 }}
gamma = {{ bounds = [0.01, 1.0], prior = {{ log_normal = {{ mu = -1.2, sigma = 0.5 }} }}, start = 0.1 }}
[fixed]
N0 = 1000
[stages.dummy]
algorithm  = "if2"
backend    = "chain_binomial"
chains     = 1
particles  = 10
iterations = 1
cooling    = 0.5
"#,
        out  = dir.join("results").display(),
        ir   = ir.display(),
        data = data.display(),
    );
    let p = dir.join(format!("{}.toml", name));
    std::fs::write(&p, toml).unwrap();
    p
}

/// Variant: distinct prior parameters from the baseline fit toml.
/// Used by the CAS-hash test — same model + data + bounds, different
/// priors must produce a different CAS dir.
fn write_fit_toml_with_priors_variant(dir: &Path, ir: &Path, data: &Path) -> PathBuf {
    let toml = format!(r#"
output_dir = "{out}"
[model]
camdl = "{ir}"
[data.observations]
cases = "{data}"
[config]
backend = "chain_binomial"
dt = 1.0
[estimate]
# Same shape, different mu/sigma → different fit_toml_hash.
beta  = {{ bounds = [0.01, 5.0], prior = {{ log_normal = {{ mu = -1.0, sigma = 0.3 }} }}, start = 0.3 }}
gamma = {{ bounds = [0.01, 1.0], prior = {{ log_normal = {{ mu = -1.5, sigma = 0.3 }} }}, start = 0.1 }}
[fixed]
N0 = 1000
[stages.dummy]
algorithm  = "if2"
backend    = "chain_binomial"
chains     = 1
particles  = 10
iterations = 1
cooling    = 0.5
"#,
        out  = dir.join("results").display(),
        ir   = ir.display(),
        data = data.display(),
    );
    let p = dir.join("fit_variant.toml");
    std::fs::write(&p, toml).unwrap();
    p
}

/// Find the umbrella profile directory under `<out_root>/profiles/`.
/// Returns the single profile dir written by the run.
fn find_umbrella(out_root: &Path) -> PathBuf {
    let profiles = out_root.join("profiles");
    let entries: Vec<_> = std::fs::read_dir(&profiles)
        .unwrap_or_else(|e| panic!("read_dir {}: {}", profiles.display(), e))
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .collect();
    assert_eq!(entries.len(), 1,
        "expected exactly one umbrella dir under {}, found {:?}",
        profiles.display(), entries);
    entries.into_iter().next().unwrap()
}

/// Read the per-seed `run.json` payload (the kind = "profile" record)
/// for the single-seed test runs.
fn read_seed_run_json(umbrella: &Path) -> serde_json::Value {
    let replicates = umbrella.join("replicates");
    let seed_dirs: Vec<_> = std::fs::read_dir(&replicates).unwrap()
        .filter_map(|e| e.ok()).map(|e| e.path()).collect();
    assert_eq!(seed_dirs.len(), 1, "expected one seed dir");
    let body = std::fs::read_to_string(seed_dirs[0].join("run.json")).unwrap();
    serde_json::from_str::<serde_json::Value>(&body).unwrap()
}

fn run_profile(
    bin: &Path,
    out_root: &Path,
    ir: &Path,
    data: &Path,
    extra_args: &[&str],
) -> std::process::Output {
    let out_tsv = out_root.join("profile.tsv");
    let mut args: Vec<String> = vec![
        "profile".into(), ir.to_string_lossy().into_owned(),
        // Pulls defaults for beta/gamma/N0 from the baseline preset
        // so the validate_parameter_values step doesn't reject the
        // run with "no value for 'beta'". This is the same way
        // `camdl profile` is invoked everywhere else in the test
        // suite (see profile_pmmh.rs).
        "--scenario".into(), "baseline".into(),
        "--data".into(), data.to_string_lossy().into_owned(),
        "--obs".into(), "cases".into(),
        "--sweep".into(), "beta=lin(0.2,0.4,2)".into(),
        "--algorithm".into(), "pmmh".into(),
        "--pmmh-steps".into(), "20".into(),
        "--pmmh-particles".into(), "30".into(),
        "--pmmh-rho".into(), "0.99".into(),
        "--particles".into(), "30".into(),
        "--iterations".into(), "5".into(),
        "--starts".into(), "1".into(),
        "--rw-sd".into(), "auto".into(),
        "--output".into(), out_tsv.to_string_lossy().into_owned(),
        "--seed".into(), "1".into(),
    ];
    for a in extra_args { args.push((*a).into()); }
    Command::new(bin)
        .env("CAMDL_OUTPUT_DIR", out_root)
        .env("CAMDL_SKIP_VERSION_CHECK", "1")
        .args(args.iter().map(|s| s.as_str()))
        .output()
        .expect("spawn camdl profile")
}

// ─── Tests ───────────────────────────────────────────────────────────────────

#[test]
fn profile_pmmh_with_neither_warns_and_uses_flat() {
    let Some(bin) = camdl_bin() else { return };
    if camdlc_bin().is_none() { return }
    let tmp = tempdir("flat_warning");
    let (ir, data) = write_fixture(tmp.path());
    let out_root = tmp.path().join("out_flat");

    let output = run_profile(&bin, &out_root, &ir, &data, &["--fixed", "N0=1000"]);
    assert!(output.status.success(),
        "profile run failed: stderr=\n{}",
        String::from_utf8_lossy(&output.stderr));
    let stderr = String::from_utf8_lossy(&output.stderr);

    // Warning must fire.
    assert!(stderr.contains("flat priors"),
        "expected 'flat priors' wording in stderr, got:\n{}", stderr);
    // Warning must name every estimated parameter (gamma in this
    // fixture; beta is the focal sweep param so it's excluded).
    assert!(stderr.contains("gamma"),
        "expected 'gamma' named in warning, got:\n{}", stderr);
    // Remediation lines must surface --fit + model-file paths.
    assert!(stderr.contains("--fit"),
        "expected --fit remedy in warning, got:\n{}", stderr);
    assert!(stderr.contains("model file"),
        "expected 'model file' remedy in warning, got:\n{}", stderr);

    // run.json must record sources = "flat_fallback" for the
    // estimated params.
    let run = read_seed_run_json(&find_umbrella(&out_root));
    let kind = run.get("kind").expect("kind");
    let resolved = kind.get("resolved_priors").expect("resolved_priors");
    let arr = resolved.as_array().expect("resolved_priors array");
    assert!(!arr.is_empty(), "resolved_priors must have at least one entry");
    let gamma_entry = arr.iter().find(|e| {
        e.get("param").and_then(|p| p.as_str()) == Some("gamma")
    }).expect("gamma must appear in resolved_priors");
    assert_eq!(
        gamma_entry.get("source").and_then(|s| s.as_str()),
        Some("flat_fallback"),
        "gamma should be flat_fallback when neither model-IR nor --fit \
         declares a prior. Got: {}", gamma_entry);
    // fit_toml_hash should be absent (None → omitted by serde
    // skip_serializing_if).
    assert!(kind.get("fit_toml_hash").is_none(),
        "fit_toml_hash should be absent without --fit. Got: {}", kind);
}

#[test]
fn profile_pmmh_with_fit_toml_silences_flat_warning() {
    let Some(bin) = camdl_bin() else { return };
    if camdlc_bin().is_none() { return }
    let tmp = tempdir("fit_priors");
    let (ir, data) = write_fixture(tmp.path());
    let fit_toml = write_fit_toml_with_priors(tmp.path(), &ir, &data, "fit");
    let out_root = tmp.path().join("out_fit");

    let output = run_profile(&bin, &out_root, &ir, &data,
        &["--fit", &fit_toml.to_string_lossy()]);
    assert!(output.status.success(),
        "profile run with --fit failed: stderr=\n{}",
        String::from_utf8_lossy(&output.stderr));
    let stderr = String::from_utf8_lossy(&output.stderr);

    // No flat-priors warning when every param has a prior.
    assert!(!stderr.contains("flat priors"),
        "warning must NOT fire when fit toml supplies priors. \
         Got stderr:\n{}", stderr);

    // run.json: gamma resolved from fit_toml (beta is the swept focal).
    let run = read_seed_run_json(&find_umbrella(&out_root));
    let kind = run.get("kind").expect("kind");
    let resolved = kind.get("resolved_priors").expect("resolved_priors");
    let gamma_entry = resolved.as_array().unwrap().iter().find(|e| {
        e.get("param").and_then(|p| p.as_str()) == Some("gamma")
    }).expect("gamma in resolved_priors");
    assert_eq!(
        gamma_entry.get("source").and_then(|s| s.as_str()),
        Some("fit_toml"),
        "gamma must be sourced from fit_toml. Got: {}", gamma_entry);

    // fit_toml_hash must be present and a 64-char hex string.
    let hash = kind.get("fit_toml_hash").and_then(|h| h.as_str())
        .expect("fit_toml_hash must be present when --fit is supplied");
    assert_eq!(hash.len(), 64, "fit_toml_hash must be SHA-256 hex (64 chars), got: {}", hash);
    assert!(hash.chars().all(|c| c.is_ascii_hexdigit()),
        "fit_toml_hash must be hex, got: {}", hash);
}

#[test]
fn same_model_different_fit_toml_different_cas_dir() {
    // Hash provenance: two profile runs with the same model + data
    // but different fit tomls (different priors) must produce
    // different umbrella CAS dirs.
    let Some(bin) = camdl_bin() else { return };
    if camdlc_bin().is_none() { return }
    let tmp = tempdir("hash_two_fits");
    let (ir, data) = write_fixture(tmp.path());
    let fit_a = write_fit_toml_with_priors(tmp.path(), &ir, &data, "fit_a");
    let fit_b = write_fit_toml_with_priors_variant(tmp.path(), &ir, &data);

    let out_a = tmp.path().join("out_a");
    let out_b = tmp.path().join("out_b");

    let a = run_profile(&bin, &out_a, &ir, &data,
        &["--fit", &fit_a.to_string_lossy()]);
    let b = run_profile(&bin, &out_b, &ir, &data,
        &["--fit", &fit_b.to_string_lossy()]);
    assert!(a.status.success(), "run A failed:\n{}",
        String::from_utf8_lossy(&a.stderr));
    assert!(b.status.success(), "run B failed:\n{}",
        String::from_utf8_lossy(&b.stderr));

    let umbrella_a = find_umbrella(&out_a);
    let umbrella_b = find_umbrella(&out_b);
    assert_ne!(umbrella_a.file_name().unwrap(),
               umbrella_b.file_name().unwrap(),
        "two distinct fit tomls must produce two distinct CAS dirs. \
         A={}, B={}", umbrella_a.display(), umbrella_b.display());
}

#[test]
fn same_model_no_fit_vs_with_fit_different_cas_dir() {
    let Some(bin) = camdl_bin() else { return };
    if camdlc_bin().is_none() { return }
    let tmp = tempdir("hash_fit_vs_none");
    let (ir, data) = write_fixture(tmp.path());
    let fit_toml = write_fit_toml_with_priors(tmp.path(), &ir, &data, "fit");

    let out_no  = tmp.path().join("out_nofit");
    let out_yes = tmp.path().join("out_yesfit");

    let no  = run_profile(&bin, &out_no,  &ir, &data, &["--fixed", "N0=1000"]);
    let yes = run_profile(&bin, &out_yes, &ir, &data,
        &["--fit", &fit_toml.to_string_lossy()]);
    assert!(no.status.success(),
        "no-fit run failed:\n{}", String::from_utf8_lossy(&no.stderr));
    assert!(yes.status.success(),
        "with-fit run failed:\n{}", String::from_utf8_lossy(&yes.stderr));

    let umbrella_no  = find_umbrella(&out_no);
    let umbrella_yes = find_umbrella(&out_yes);
    assert_ne!(umbrella_no.file_name().unwrap(),
               umbrella_yes.file_name().unwrap(),
        "no-fit and with-fit runs must produce distinct CAS dirs");
}

#[test]
fn focal_param_in_fixed_errors_clearly() {
    // Spec §2 rule: a parameter cannot simultaneously be the sweep
    // axis and in --fixed. The error must name the conflict source.
    let Some(bin) = camdl_bin() else { return };
    if camdlc_bin().is_none() { return }
    let tmp = tempdir("focal_conflict");
    let (ir, data) = write_fixture(tmp.path());
    let out_root = tmp.path().join("out");

    let output = run_profile(&bin, &out_root, &ir, &data,
        // beta is both swept and fixed.
        &["--fixed", "beta=0.3", "--fixed", "gamma=0.1", "--fixed", "N0=1000"]);
    assert!(!output.status.success(),
        "swept+fixed conflict must be a hard error");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("--sweep") && stderr.contains("--fixed"),
        "error must name both --sweep and --fixed in the conflict \
         message. Got:\n{}", stderr);
}
