//! End-to-end tests for the dated-data loader (2026-05-22 calendar-time,
//! phase 2). Exercised through the `camdl` CLI so the whole boundary —
//! column detection, date→internal-time conversion, the distinct-substep
//! check, and the origin-missing / mixed-column errors — runs in the
//! production data path, not just the unit-tested core.
//!
//! The model is a copy of `crates/sim/tests/fixtures/seed_timing.ir.json`
//! with an `origin` injected at runtime (the committed fixture has none, so
//! the same file also serves the origin-missing test).
//!
//! Silent-skip if the release `camdl` binary is not built (mirrors
//! seed_timing_e2e.rs). `CAMDL_SKIP_VERSION_CHECK=1` avoids a stale globally
//! installed `camdlc` making the test flaky.

use std::path::{Path, PathBuf};
use std::process::Command;

fn camdl_bin() -> Option<PathBuf> {
    let manifest = std::env::var("CARGO_MANIFEST_DIR").ok()?;
    let p = Path::new(&manifest).join("../../target/release/camdl");
    p.exists().then_some(p)
}

fn fixtures() -> PathBuf {
    let manifest = std::env::var("CARGO_MANIFEST_DIR").unwrap();
    Path::new(&manifest).join("tests/fixtures")
}

fn seed_timing_ir() -> PathBuf {
    let manifest = std::env::var("CARGO_MANIFEST_DIR").unwrap();
    Path::new(&manifest).join("../sim/tests/fixtures/seed_timing.ir.json")
}

fn tempdir(tag: &str) -> PathBuf {
    let ns = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let p = std::env::temp_dir().join(format!("camdl_dated_{}_{}_{}", tag, std::process::id(), ns));
    std::fs::create_dir_all(&p).unwrap();
    p
}

fn run(camdl: &Path, args: &[&str]) -> std::process::Output {
    Command::new(camdl)
        .args(args)
        .env("CAMDL_SKIP_VERSION_CHECK", "1")
        .output()
        .expect("camdl must invoke")
}

/// Write a copy of the seed_timing IR with an `origin` field injected.
fn model_with_origin(dir: &Path, origin: &str) -> PathBuf {
    let src = std::fs::read_to_string(seed_timing_ir()).unwrap();
    // Inject `"origin": "..."` right after the time_unit line.
    let injected = src.replacen(
        "\"time_unit\": \"days\",",
        &format!("\"time_unit\": \"days\",\n    \"origin\": \"{origin}\","),
        1,
    );
    assert!(injected.contains("\"origin\""), "origin injection failed");
    let p = dir.join("seed_timing_origin.ir.json");
    std::fs::write(&p, injected).unwrap();
    p
}

const BASE_PARAMS: &[&str] = &[
    "--param", "beta=0.6",
    "--param", "gamma=0.2",
    "--param", "lambda=2.0",
    "--param", "w=3.0",
    "--param", "N0=5000",
    "--param", "rho=0.5",
    "--param", "k=20",
    "--param", "tau=30",
];

fn pfilter_loglik(camdl: &Path, model: &Path, data: &Path, extra: &[&str]) -> std::process::Output {
    let mut args = vec![
        "pfilter", model.to_str().unwrap(),
        "--particles", "500", "--dt", "1", "--seed", "5",
        "--data", data.to_str().unwrap(),
    ];
    args.extend_from_slice(BASE_PARAMS);
    args.extend_from_slice(extra);
    run(camdl, &args)
}

fn parse_loglik(out: &std::process::Output) -> f64 {
    let stdout = String::from_utf8_lossy(&out.stdout);
    stdout
        .lines()
        .rev()
        .find_map(|l| l.trim().parse::<f64>().ok())
        .unwrap_or_else(|| panic!("no loglik in output:\nSTDOUT:{stdout}\nSTDERR:{}",
            String::from_utf8_lossy(&out.stderr)))
}

/// §9.4 byte-identity: a dated TSV yields the same pfilter loglik as the same
/// data hand-converted to day-numbers against the origin.
#[test]
fn dated_loglik_matches_numeric() {
    let Some(camdl) = camdl_bin() else {
        eprintln!("skipping: release camdl binary not built");
        return;
    };
    let tmp = tempdir("byteid");
    let model = model_with_origin(&tmp, "2020-02-28");

    // origin = 2020-02-28. Dates → day-numbers:
    //   2020-03-01 → 2, 2020-03-08 → 9, 2020-03-15 → 16, 2020-03-22 → 23
    let dated = tmp.join("dated.tsv");
    std::fs::write(&dated,
        "time\tcases\n2020-03-01\t3\n2020-03-08\t40\n2020-03-15\t120\n2020-03-22\t60\n").unwrap();
    let numeric = tmp.join("numeric.tsv");
    std::fs::write(&numeric, "time\tcases\n2\t3\n9\t40\n16\t120\n23\t60\n").unwrap();

    let ll_dated = parse_loglik(&pfilter_loglik(&camdl, &model, &dated, &[]));
    let ll_numeric = parse_loglik(&pfilter_loglik(&camdl, &model, &numeric, &[]));
    assert_eq!(ll_dated, ll_numeric,
        "dated and hand-converted-numeric logliks must be bit-identical");

    let _ = std::fs::remove_dir_all(&tmp);
}

/// §9.4 origin-missing: dated cells against a model with no origin → error.
#[test]
fn dated_without_origin_errors() {
    let Some(camdl) = camdl_bin() else { return };
    let tmp = tempdir("noorigin");
    // seed_timing.ir.json (committed) declares no origin.
    let model = seed_timing_ir();
    let dated = tmp.join("dated.tsv");
    std::fs::write(&dated, "time\tcases\n2020-03-01\t3\n2020-03-08\t40\n").unwrap();

    let out = pfilter_loglik(&camdl, &model, &dated, &[]);
    assert!(!out.status.success(), "must fail when dated data has no origin");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("origin"), "error should mention origin: {stderr}");

    let _ = std::fs::remove_dir_all(&tmp);
}

/// §9.4 mixed column: numeric + date in one column → hard error naming rows.
#[test]
fn mixed_column_errors() {
    let Some(camdl) = camdl_bin() else { return };
    let tmp = tempdir("mixed");
    let model = model_with_origin(&tmp, "2020-02-28");
    let mixed = tmp.join("mixed.tsv");
    std::fs::write(&mixed, "time\tcases\n2\t3\n2020-03-08\t40\n").unwrap();

    let out = pfilter_loglik(&camdl, &model, &mixed, &[]);
    assert!(!out.status.success(), "must fail on a mixed time column");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("mixed"), "error should flag mixed column: {stderr}");

    let _ = std::fs::remove_dir_all(&tmp);
}

/// §9.4 distinct-substep collision: two distinct times within dt → error.
#[test]
fn distinct_substep_collision_errors() {
    let Some(camdl) = camdl_bin() else { return };
    let tmp = tempdir("collide");
    let model = model_with_origin(&tmp, "2020-02-28");
    // Numeric times 10.0 and 10.4 at dt=1 both round to step 10.
    let data = tmp.join("collide.tsv");
    std::fs::write(&data, "time\tcases\n10.0\t3\n10.4\t40\n20\t60\n").unwrap();

    let out = pfilter_loglik(&camdl, &model, &data, &[]);
    assert!(!out.status.success(), "must fail on a sub-dt observation collision");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("same integrator substep"),
        "error should flag the substep collision: {stderr}");

    let _ = std::fs::remove_dir_all(&tmp);
}

/// §9.4 `--time-format numeric` forbids date cells.
#[test]
fn time_format_numeric_forbids_dates() {
    let Some(camdl) = camdl_bin() else { return };
    let tmp = tempdir("fmtnum");
    let model = model_with_origin(&tmp, "2020-02-28");
    let dated = tmp.join("dated.tsv");
    std::fs::write(&dated, "time\tcases\n2020-03-01\t3\n2020-03-08\t40\n").unwrap();

    let out = pfilter_loglik(&camdl, &model, &dated, &["--time-format", "numeric"]);
    assert!(!out.status.success(), "--time-format numeric must reject date cells");

    let _ = std::fs::remove_dir_all(&tmp);
}

/// Write the seed_timing IR with `simulation.t_start` set to `c`.
fn model_with_t_start(dir: &Path, c: f64) -> PathBuf {
    let src = std::fs::read_to_string(seed_timing_ir()).unwrap();
    let injected = src.replacen("\"t_start\": 0.0,", &format!("\"t_start\": {c},"), 1);
    assert!(injected.contains(&format!("\"t_start\": {c},")), "t_start injection failed");
    let p = dir.join(format!("seed_timing_tstart_{c}.ir.json"));
    std::fs::write(&p, injected).unwrap();
    p
}

/// §9.0.1 shift-invariance (numeric engine): shifting `(t_start, data times,
/// the time-typed param tau)` together by `c` leaves the pfilter loglik
/// bit-identical. This is the property the dated loader relies on (a change
/// of origin is exactly such a shift). Includes a negative shift (origin
/// after the first obs → negative internal times).
#[test]
fn numeric_shift_invariance() {
    let Some(camdl) = camdl_bin() else { return };
    let tmp = tempdir("shift");

    // Baseline data at tau=30: numeric daily cases (chosen to give a real
    // epidemic so the loglik is non-degenerate).
    let base_rows: &[(f64, i64)] =
        &[(20.0, 2), (30.0, 25), (40.0, 110), (50.0, 70), (60.0, 20)];

    let loglik_at_shift = |c: f64| -> f64 {
        let model = model_with_t_start(&tmp, c);
        let data = tmp.join(format!("shift_{c}.tsv"));
        let mut s = String::from("time\tcases\n");
        for (t, v) in base_rows {
            s.push_str(&format!("{}\t{}\n", t + c, v));
        }
        std::fs::write(&data, s).unwrap();
        let tau = format!("tau={}", 30.0 + c);
        // All BASE_PARAMS except tau, then the shifted tau.
        let params: Vec<&str> = BASE_PARAMS
            .iter()
            .copied()
            .take(BASE_PARAMS.len() - 2) // drop the trailing "--param", "tau=30"
            .collect();
        let mut args = vec![
            "pfilter", model.to_str().unwrap(),
            "--particles", "1000", "--dt", "1", "--seed", "7",
            "--data", data.to_str().unwrap(),
        ];
        args.extend_from_slice(&params);
        args.push("--param");
        args.push(&tau);
        let out = run(&camdl, &args);
        assert!(out.status.success(), "pfilter (shift {c}) failed: {}",
            String::from_utf8_lossy(&out.stderr));
        parse_loglik(&out)
    };

    let base = loglik_at_shift(0.0);
    for c in [-20.0, -11.0, 11.0, 20.0] {
        let ll = loglik_at_shift(c);
        assert_eq!(ll, base,
            "loglik must be shift-invariant: c={c} gave {ll}, c=0 gave {base}");
    }

    let _ = std::fs::remove_dir_all(&tmp);
}

/// §9.7 multi-timezone civil-date alignment: a TSV whose cells carry mixed
/// trailing offsets loads to the *same* internal time as the offset-stripped
/// sibling — the offset is discarded, every row maps to the same civil date.
#[test]
fn multitz_offsets_collapse_to_civil_date() {
    let Some(camdl) = camdl_bin() else { return };
    let tmp = tempdir("multitz");
    let model = model_with_origin(&tmp, "2020-02-28");

    let with_tz = fixtures().join("dated_multitz.tsv");
    let naive = fixtures().join("dated_multitz_naive.tsv");

    // All 5 rows are 2020-03-15 (same civil date) → identical internal time.
    // Multi-stream-style equal times are accepted; the loglik is well-defined
    // and must be identical whether or not the offsets were present.
    let ll_tz = parse_loglik(&pfilter_loglik(&camdl, &model, &with_tz, &[]));
    let ll_naive = parse_loglik(&pfilter_loglik(&camdl, &model, &naive, &[]));
    assert_eq!(ll_tz, ll_naive,
        "offset-bearing dates must collapse to the same civil date as the naive sibling");

    let _ = std::fs::remove_dir_all(&tmp);
}
