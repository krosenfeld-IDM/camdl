//! Cross-validate camdl's Fourier forcing evaluator against numpy.
//!
//! Math is direct (`Σ_k a_k cos(2π k t/T) + b_k sin(...)`) so the
//! risk is transcription error rather than algorithm choice. The
//! numpy oracle still catches index-off-by-one, harmonic-shift, and
//! period-sign mistakes that a hand-computed test would miss.

use sim::propensity::fourier_value;
use std::fs::File;
use std::io::{BufRead, BufReader};
use std::path::Path;

fn read_tsv_pairs(path: &Path) -> Vec<(f64, f64)> {
    let f = File::open(path).unwrap_or_else(|e|
        panic!("could not open {}: {}", path.display(), e));
    let mut out = Vec::new();
    for line in BufReader::new(f).lines() {
        let line = line.unwrap();
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') || trimmed.starts_with("t\t") {
            continue;
        }
        let cols: Vec<&str> = trimmed.split('\t').collect();
        assert_eq!(cols.len(), 2, "expected 2 columns");
        out.push((cols[0].parse().unwrap(), cols[1].parse().unwrap()));
    }
    out
}

#[test]
fn fourier_matches_numpy() {
    let path = Path::new("tests/fixtures/fourier_numpy.tsv");
    let pairs = read_tsv_pairs(path);
    assert!(!pairs.is_empty(), "fourier fixture is empty");

    // Must match the fixture generator's parameters
    // (scripts/gen_fourier_numpy_fixture.py).
    let harmonics = [(0.2, 0.1), (0.05, -0.07), (0.03, 0.02)];
    let period_inv = 1.0 / 365.25;

    let mut max_diff: f64 = 0.0;
    for (t, expected) in &pairs {
        let actual = fourier_value(period_inv, &harmonics, *t);
        let diff = (actual - expected).abs();
        if diff > max_diff { max_diff = diff; }
    }
    assert!(
        max_diff < 1e-12,
        "camdl Fourier vs numpy max |diff| = {:.3e}; threshold 1e-12",
        max_diff
    );
}
