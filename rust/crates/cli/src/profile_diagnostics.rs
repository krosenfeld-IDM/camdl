//! gh#74 Option B: per-cell convergence diagnostics for `camdl profile`.
//!
//! Each profile grid cell runs K independent `--starts` chains under
//! one of three algorithms (PMMH, IF2, NLopt). Pre-gh#74 the diagnostic
//! data each chain produced — acceptance rates, per-step loglik traces,
//! cooling-schedule end-states — was computed inside the inference
//! engine, used to decide the cell's MLE winner, and then thrown away.
//! Users (and coding agents) had no way to tell whether a jagged
//! profile was MCMC noise that more compute would smooth, genuine
//! multimodality, or per-cell optimization failures.
//!
//! This module owns the per-start → per-cell aggregation:
//!
//! * `PerStartDiagnostics` is the per-start record. The profile driver
//!   populates one per (cell × start) and serializes it into that
//!   start's `mle.toml` under a `[diagnostics]` block. Persisting it
//!   on disk (rather than threading it through the parallel loop) is
//!   what lets the per-cell rollup phase — which already scans every
//!   start's `mle.toml` — compute aggregates without changing the
//!   job-scheduling shape.
//!
//! * `CellDiagnostics` is the per-cell aggregate. The rollup reads
//!   each `mle.toml`'s `[diagnostics]` block, builds the per-cell
//!   diagnostic, and emits it as new columns in the per-seed
//!   `profile.tsv` and the umbrella `summary.tsv`.
//!
//! The aggregate columns are documented in `docs/inference.md`
//! ("Per-cell diagnostics") and in the `camdl profile --help` OUTPUT
//! section. Schema is stable per-run; algorithms that don't supply a
//! given column write `NaN` (capital N — matches camdl's existing TSV
//! NaN convention).

use serde::{Deserialize, Serialize};

/// Algorithm tag carried per-start. Determines which fields of
/// `PerStartDiagnostics` are populated.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DiagAlgo {
    /// IF2 — exposes `iterations_used`, `cooling_final`, and the per-iter
    /// (perturbed) loglik trace. `acc_rate` is `None` (no MH step).
    If2,
    /// PMMH — exposes `acc_rate` and the per-step loglik trace.
    Pmmh,
    /// NLopt deterministic optimizer — exposes only the final loglik
    /// (no trace, no acceptance rate, no cooling).
    Nlopt,
}

impl DiagAlgo {
    pub fn as_str(self) -> &'static str {
        match self {
            DiagAlgo::If2   => "if2",
            DiagAlgo::Pmmh  => "pmmh",
            DiagAlgo::Nlopt => "nlopt",
        }
    }
}

/// Per-start convergence record. Persisted into each `mle.toml`'s
/// `[diagnostics]` block; the rollup parses it back and aggregates
/// across the K starts at one cell.
#[derive(Clone, Debug, Default)]
pub struct PerStartDiagnostics {
    /// Which algorithm produced this start. Drives the aggregate
    /// column population at rollup time.
    pub algo: Option<DiagAlgo>,
    /// Whether the start ran to completion. False when the inference
    /// engine returned `Err` (treated as -Inf loglik by the profile
    /// driver) or when `final_loglik` is non-finite.
    pub completed: bool,
    /// MH acceptance rate (PMMH only; `None` for IF2 / NLopt).
    pub acc_rate: Option<f64>,
    /// Final cooling-step index reached (IF2 only).
    pub iterations_used: Option<usize>,
    /// Final perturbation SD applied at the last cooling step (IF2
    /// only). Mean across estimated params at the final iteration —
    /// scalar summary of "how cool we got."
    pub cooling_final: Option<f64>,
    /// Per-step (PMMH) or per-iteration (IF2) loglik trace. Used by
    /// the rollup to compute Gelman-Rubin Rhat across the K starts.
    /// Empty for NLopt (no trace surface). For IF2 the trace carries
    /// `if2_perturbed_loglik` — a diagnostic, not the model loglik —
    /// matching how the IF2 engine docs describe it; Rhat is a chain-
    /// level agreement metric, so the perturbed trace is a fine
    /// proxy for "are the starts wandering the same basin."
    pub loglik_trace: Vec<f64>,
}

impl PerStartDiagnostics {
    /// Render to a TOML fragment for embedding inside the per-start
    /// `mle.toml`. Caller writes the result under a `[diagnostics]`
    /// section header. Trace arrays use the inline-array form so
    /// they survive a round-trip through any TOML 1.0 parser.
    pub fn to_toml_fragment(&self) -> String {
        let mut body = String::new();
        if let Some(algo) = self.algo {
            body.push_str(&format!("algorithm = \"{}\"\n", algo.as_str()));
        }
        body.push_str(&format!("completed = {}\n", self.completed));
        if let Some(v) = self.acc_rate {
            body.push_str(&format!("acc_rate = {}\n", v));
        }
        if let Some(v) = self.iterations_used {
            body.push_str(&format!("iterations_used = {}\n", v));
        }
        if let Some(v) = self.cooling_final {
            body.push_str(&format!("cooling_final = {}\n", v));
        }
        if !self.loglik_trace.is_empty() {
            body.push_str("loglik_trace = [");
            for (i, v) in self.loglik_trace.iter().enumerate() {
                if i > 0 { body.push_str(", "); }
                if v.is_finite() {
                    body.push_str(&format!("{}", v));
                } else if v.is_nan() {
                    body.push_str("nan");
                } else if *v == f64::INFINITY {
                    body.push_str("inf");
                } else {
                    body.push_str("-inf");
                }
            }
            body.push_str("]\n");
        }
        body
    }

    /// Parse the `[diagnostics]` table out of a parsed `mle.toml`
    /// document. Missing block → an all-default record (algo = None,
    /// completed = false). Older `mle.toml` files (pre-gh#74, no
    /// `[diagnostics]` block) round-trip as "no diagnostics
    /// available" rather than an error.
    pub fn from_toml(doc: &toml::Value) -> Self {
        let Some(t) = doc.get("diagnostics").and_then(|v| v.as_table()) else {
            return Self::default();
        };
        let algo = t.get("algorithm").and_then(|v| v.as_str())
            .and_then(|s| match s {
                "if2"   => Some(DiagAlgo::If2),
                "pmmh"  => Some(DiagAlgo::Pmmh),
                "nlopt" => Some(DiagAlgo::Nlopt),
                _       => None,
            });
        let completed = t.get("completed").and_then(|v| v.as_bool()).unwrap_or(false);
        let acc_rate = t.get("acc_rate").and_then(toml_as_f64);
        let iterations_used = t.get("iterations_used")
            .and_then(|v| v.as_integer())
            .and_then(|i| usize::try_from(i).ok());
        let cooling_final = t.get("cooling_final").and_then(toml_as_f64);
        let loglik_trace: Vec<f64> = t.get("loglik_trace")
            .and_then(|v| v.as_array())
            .map(|a| a.iter().filter_map(toml_as_f64).collect())
            .unwrap_or_default();
        Self { algo, completed, acc_rate, iterations_used, cooling_final, loglik_trace }
    }
}

fn toml_as_f64(v: &toml::Value) -> Option<f64> {
    match v {
        toml::Value::Float(f) => Some(*f),
        toml::Value::Integer(i) => Some(*i as f64),
        // toml-rs 0.5 parses `nan` / `inf` as Float; nothing else to do.
        _ => None,
    }
}

/// Per-cell aggregate. Derived from the K per-start records at one
/// grid point. NaN values mean "the algorithm in use doesn't supply
/// this column" or "fewer than the minimum starts ran" — both
/// surface as `NaN` in the output TSV per camdl's convention.
#[derive(Clone, Debug)]
pub struct CellDiagnostics {
    pub acc_rate_avg:        f64,
    pub acc_rate_min:        f64,
    pub loglik_spread_starts: f64,
    pub loglik_rhat_starts:   f64,
    pub starts_n_completed:   usize,
    pub iterations_used:      f64,
    pub cooling_final:        f64,
}

/// Names of the per-cell diagnostic columns, in the order they appear
/// in the TSV. The header is fixed per run — algorithm-specific
/// columns are still emitted as columns; rows from algorithms that
/// don't supply them write `NaN`. This keeps the schema "fixed per
/// run" (gh#74 schema rule 2) while leaving room to expand later.
pub const DIAG_COLUMNS: &[&str] = &[
    "acc_rate_avg",
    "acc_rate_min",
    "loglik_spread_starts",
    "loglik_rhat_starts",
    "starts_n_completed",
    "iterations_used",
    "cooling_final",
];

impl CellDiagnostics {
    /// Format the aggregate as the seven trailing columns of one
    /// `profile.tsv` / `summary.tsv` row, in the order declared by
    /// `DIAG_COLUMNS`. No leading tab — caller appends after the
    /// existing schema's last column.
    pub fn render_tsv_row(&self) -> String {
        format!(
            "{}\t{}\t{}\t{}\t{}\t{}\t{}",
            format_diag_f64(self.acc_rate_avg),
            format_diag_f64(self.acc_rate_min),
            format_diag_f64(self.loglik_spread_starts),
            format_diag_f64(self.loglik_rhat_starts),
            self.starts_n_completed,
            format_diag_f64(self.iterations_used),
            format_diag_f64(self.cooling_final),
        )
    }

    /// Aggregate K per-start records into one cell-level diagnostic.
    ///
    /// Aggregation rules (gh#74 Option B):
    ///
    /// * `acc_rate_avg` / `acc_rate_min`: mean / min over the K
    ///   `acc_rate` values. NaN if no start supplied an acc_rate
    ///   (i.e. not PMMH).
    /// * `loglik_spread_starts`: max − min of per-start final logliks
    ///   (taken from the last entry of each start's loglik_trace, or
    ///   from the trace's max if the algorithm reports trace).
    ///   NaN if fewer than 2 finite final-logliks.
    /// * `loglik_rhat_starts`: Gelman-Rubin Rhat across the K trace
    ///   sequences. NaN at K<3 (the K<3 rule — Rhat is undefined for
    ///   K=1 and unstable for K=2).
    /// * `starts_n_completed`: count of starts with `completed == true`.
    /// * `iterations_used`: mean across starts (IF2 only). NaN
    ///   otherwise.
    /// * `cooling_final`: mean across starts (IF2 only). NaN
    ///   otherwise.
    pub fn aggregate(
        starts: &[PerStartDiagnostics],
        per_start_final_loglik: &[f64],
    ) -> Self {
        let k = starts.len();
        debug_assert_eq!(k, per_start_final_loglik.len());

        // PMMH-only: acc_rate aggregate.
        let acc_rates: Vec<f64> = starts.iter()
            .filter_map(|d| d.acc_rate)
            .filter(|x| x.is_finite())
            .collect();
        let (acc_rate_avg, acc_rate_min) = if acc_rates.is_empty() {
            (f64::NAN, f64::NAN)
        } else {
            let avg = acc_rates.iter().sum::<f64>() / acc_rates.len() as f64;
            let min = acc_rates.iter().copied().fold(f64::INFINITY, f64::min);
            (avg, min)
        };

        // Spread across starts. Uses the canonical per-start final
        // loglik supplied by the rollup (already the same value used
        // for cell winner selection).
        let finite_finals: Vec<f64> = per_start_final_loglik.iter().copied()
            .filter(|x| x.is_finite()).collect();
        let loglik_spread_starts = if finite_finals.len() < 2 {
            f64::NAN
        } else {
            let mx = finite_finals.iter().copied().fold(f64::NEG_INFINITY, f64::max);
            let mn = finite_finals.iter().copied().fold(f64::INFINITY, f64::min);
            mx - mn
        };

        // R-hat over the K traces. Skip when K < 3 — Gelman-Rubin is
        // undefined at K=1 and high-variance at K=2.
        let traces: Vec<&[f64]> = starts.iter()
            .filter(|d| d.completed && !d.loglik_trace.is_empty())
            .map(|d| d.loglik_trace.as_slice())
            .collect();
        let loglik_rhat_starts = if traces.len() < 3 {
            f64::NAN
        } else {
            gelman_rubin_rhat(&traces)
        };

        let starts_n_completed = starts.iter().filter(|d| d.completed).count();

        // IF2-only aggregates. Mean across completed starts. NaN if
        // no start supplied a value.
        let iters: Vec<f64> = starts.iter()
            .filter_map(|d| d.iterations_used.map(|n| n as f64))
            .collect();
        let iterations_used = if iters.is_empty() {
            f64::NAN
        } else {
            iters.iter().sum::<f64>() / iters.len() as f64
        };
        let cools: Vec<f64> = starts.iter()
            .filter_map(|d| d.cooling_final)
            .filter(|x| x.is_finite())
            .collect();
        let cooling_final = if cools.is_empty() {
            f64::NAN
        } else {
            cools.iter().sum::<f64>() / cools.len() as f64
        };

        CellDiagnostics {
            acc_rate_avg, acc_rate_min,
            loglik_spread_starts, loglik_rhat_starts,
            starts_n_completed,
            iterations_used, cooling_final,
        }
    }

    /// Combine N per-seed `CellDiagnostics` (one per seed at the same
    /// grid cell) into one cross-seed aggregate for the umbrella
    /// `summary.tsv`. Per-seed values are averaged where defined,
    /// `starts_n_completed` sums across seeds (total completed starts
    /// at this cell across replicates), and NaN cells are dropped
    /// from the average.
    pub fn average_across_seeds(per_seed: &[CellDiagnostics]) -> CellDiagnostics {
        fn mean_finite(xs: impl Iterator<Item = f64>) -> f64 {
            let v: Vec<f64> = xs.filter(|x| x.is_finite()).collect();
            if v.is_empty() { f64::NAN } else { v.iter().sum::<f64>() / v.len() as f64 }
        }
        CellDiagnostics {
            acc_rate_avg: mean_finite(per_seed.iter().map(|c| c.acc_rate_avg)),
            acc_rate_min: mean_finite(per_seed.iter().map(|c| c.acc_rate_min)),
            loglik_spread_starts: mean_finite(per_seed.iter().map(|c| c.loglik_spread_starts)),
            loglik_rhat_starts: mean_finite(per_seed.iter().map(|c| c.loglik_rhat_starts)),
            starts_n_completed: per_seed.iter().map(|c| c.starts_n_completed).sum(),
            iterations_used: mean_finite(per_seed.iter().map(|c| c.iterations_used)),
            cooling_final: mean_finite(per_seed.iter().map(|c| c.cooling_final)),
        }
    }
}

/// Format a diagnostic float for TSV output. NaN renders as `NaN`,
/// Inf / -Inf as `Inf` / `-Inf` — matches `format_float_for_tsv` in
/// `fit/init.rs` (the established camdl convention).
fn format_diag_f64(v: f64) -> String {
    if v.is_nan() { "NaN".into() }
    else if v == f64::INFINITY { "Inf".into() }
    else if v == f64::NEG_INFINITY { "-Inf".into() }
    else { format!("{}", v) }
}

/// Compute Gelman-Rubin potential-scale-reduction-factor Rhat across
/// K chains of equal-or-unequal length. The diagnostic compares
/// between-chain variance B to within-chain variance W: Rhat near 1.0
/// means the chains agree; > 1.05 is the conventional "not yet
/// converged" threshold.
///
/// Formula (Gelman & Rubin 1992, Statist. Sci. 7(4); Brooks & Gelman
/// 1998 corrected variant):
///
/// ```text
/// W = (1/K) Σ_k s²_k              (mean within-chain variance)
/// B = (n/(K-1)) Σ_k (x̄_k - x̄)²    (between-chain variance, scaled)
/// V̂ = ((n-1)/n) W + (1/n) B
/// Rhat = sqrt(V̂ / W)
/// ```
///
/// where `n` is the per-chain sample count. We require all chains to
/// have the same length (the typical PMMH / IF2 case); on unequal
/// lengths we truncate to the minimum so the formula's variance
/// estimators stay unbiased. Caller should guard `K >= 3` at the
/// call site — this function returns NaN when the inputs are
/// degenerate but doesn't apply the K<3 policy.
fn gelman_rubin_rhat(chains: &[&[f64]]) -> f64 {
    let k = chains.len();
    if k < 2 { return f64::NAN; }
    // Truncate to the shortest finite-only prefix; drop any chain
    // that has fewer than 2 finite samples.
    let finite: Vec<Vec<f64>> = chains.iter()
        .map(|c| c.iter().copied().filter(|x| x.is_finite()).collect::<Vec<_>>())
        .filter(|v| v.len() >= 2)
        .collect();
    if finite.len() < 2 { return f64::NAN; }
    let n = finite.iter().map(|v| v.len()).min().unwrap_or(0);
    if n < 2 { return f64::NAN; }
    let truncated: Vec<&[f64]> = finite.iter().map(|v| &v[..n]).collect();
    let k = truncated.len() as f64;

    let chain_means: Vec<f64> = truncated.iter()
        .map(|c| c.iter().sum::<f64>() / c.len() as f64)
        .collect();
    let grand_mean = chain_means.iter().sum::<f64>() / k;
    let n_f = n as f64;

    // Within-chain variance: mean of per-chain sample variances
    // (Bessel-corrected, n-1 denominator).
    let within: f64 = truncated.iter().zip(&chain_means)
        .map(|(c, &mu)| {
            let ss: f64 = c.iter().map(|x| (x - mu).powi(2)).sum();
            ss / (n_f - 1.0)
        })
        .sum::<f64>() / k;
    // Between-chain variance.
    let between: f64 = chain_means.iter()
        .map(|m| (m - grand_mean).powi(2))
        .sum::<f64>() * n_f / (k - 1.0);

    if !within.is_finite() || within <= 0.0 {
        // Constant within every chain: Rhat is ill-defined; report
        // 1.0 when the chains also agree on the constant, NaN
        // otherwise. The typical cause is a trace of repeated -Inf
        // values that got filtered out — in which case `finite`
        // would have been < 2 and we'd have returned NaN above. So
        // by here `within == 0` means every chain reported the same
        // finite constant trace, which is "perfectly converged."
        return if between == 0.0 { 1.0 } else { f64::NAN };
    }

    let var_hat = (n_f - 1.0) / n_f * within + between / n_f;
    (var_hat / within).sqrt()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rhat_identical_chains_under_one() {
        // Identical chains have between-chain variance B = 0 but
        // non-zero within-chain variance W. Standard Gelman-Rubin
        // gives Rhat = sqrt((n-1)/n) → just below 1.0 as n grows.
        // (Brooks & Gelman 1998 corrected variant gives the same
        // value; the bias is intrinsic to the unmodified formula.)
        // This test pins the convention: identical chains land
        // strictly below 1 by the sqrt((n-1)/n) factor, which is
        // close enough to "converged" for the diagnostic surface to
        // be useful — operationally users compare against the
        // conventional 1.05 threshold, not against 1.0 exactly.
        let n = 5usize;
        let c: Vec<f64> = (0..n).map(|i| i as f64).collect();
        let chains: Vec<&[f64]> = vec![&c, &c, &c];
        let r = gelman_rubin_rhat(&chains);
        let expected = ((n as f64 - 1.0) / n as f64).sqrt();
        assert!((r - expected).abs() < 1e-9,
            "identical chains Rhat must equal sqrt((n-1)/n) = {}, got {}",
            expected, r);
        assert!(r < 1.0,
            "identical-chain Rhat must be strictly below 1.0, got {}", r);
    }

    #[test]
    fn rhat_constant_chains_returns_one() {
        // Edge case: every chain is the same *constant* value (no
        // within-chain variance, no between-chain variance). Without
        // the W = 0 guard the standard formula divides by zero.
        // Per the helper's contract: perfect agreement → Rhat = 1.
        let c = vec![2.5_f64; 8];
        let chains: Vec<&[f64]> = vec![&c, &c, &c];
        let r = gelman_rubin_rhat(&chains);
        assert!((r - 1.0).abs() < 1e-9,
            "constant chains with no within-variance should report Rhat = 1, got {}", r);
    }

    #[test]
    fn rhat_finite_for_distinct_chains() {
        // Three distinct constant-offset chains — between > 0,
        // within > 0, so Rhat is finite and > 1.
        let a = vec![1.0, 1.1, 0.9, 1.05, 0.95];
        let b = vec![2.0, 2.1, 1.9, 2.05, 1.95];
        let c = vec![3.0, 3.1, 2.9, 3.05, 2.95];
        let chains: Vec<&[f64]> = vec![&a, &b, &c];
        let r = gelman_rubin_rhat(&chains);
        assert!(r.is_finite() && r > 1.0,
            "distinct chains should give finite Rhat > 1, got {}", r);
    }

    #[test]
    fn rhat_nan_when_fewer_than_two_chains_have_data() {
        let chains: Vec<&[f64]> = vec![&[1.0_f64, 2.0]];
        assert!(gelman_rubin_rhat(&chains).is_nan());
    }

    #[test]
    fn aggregate_k_lt_3_makes_rhat_nan() {
        // K=2 with valid traces: per the K<3 rule the rollup must
        // emit NaN — the policy is at the aggregate level, not the
        // raw Rhat helper. Per-start traces are present and would
        // otherwise produce a finite Rhat.
        let starts = vec![
            PerStartDiagnostics {
                algo: Some(DiagAlgo::Pmmh),
                completed: true,
                acc_rate: Some(0.3),
                loglik_trace: vec![-10.0, -9.5, -9.0],
                ..Default::default()
            },
            PerStartDiagnostics {
                algo: Some(DiagAlgo::Pmmh),
                completed: true,
                acc_rate: Some(0.4),
                loglik_trace: vec![-11.0, -10.5, -10.0],
                ..Default::default()
            },
        ];
        let agg = CellDiagnostics::aggregate(&starts, &[-9.0, -10.0]);
        assert!(agg.loglik_rhat_starts.is_nan(),
            "K<3 rule: loglik_rhat_starts must be NaN at K=2, got {}",
            agg.loglik_rhat_starts);
        assert!((agg.acc_rate_avg - 0.35).abs() < 1e-9);
        assert!((agg.acc_rate_min - 0.30).abs() < 1e-9);
        assert!((agg.loglik_spread_starts - 1.0).abs() < 1e-9);
        assert_eq!(agg.starts_n_completed, 2);
    }

    #[test]
    fn aggregate_k_ge_3_returns_finite_rhat() {
        let starts: Vec<PerStartDiagnostics> = (0..3).map(|i| {
            let off = i as f64;
            PerStartDiagnostics {
                algo: Some(DiagAlgo::Pmmh),
                completed: true,
                acc_rate: Some(0.3 + 0.05 * i as f64),
                loglik_trace: vec![-10.0 + off, -9.5 + off, -9.0 + off, -9.2 + off],
                ..Default::default()
            }
        }).collect();
        let finals = vec![-9.0, -8.0, -7.0];
        let agg = CellDiagnostics::aggregate(&starts, &finals);
        assert!(agg.loglik_rhat_starts.is_finite(),
            "K=3 must produce a finite Rhat, got {}", agg.loglik_rhat_starts);
        assert_eq!(agg.starts_n_completed, 3);
        assert!((agg.acc_rate_avg - 0.35).abs() < 1e-9);
        assert!((agg.loglik_spread_starts - 2.0).abs() < 1e-9);
    }

    #[test]
    fn aggregate_handles_diverged_chains() {
        // One start diverged (completed = false, NEG_INFINITY final
        // loglik). Spread excludes it; n_completed reports < K.
        let starts = vec![
            PerStartDiagnostics {
                algo: Some(DiagAlgo::Pmmh),
                completed: true,
                acc_rate: Some(0.3),
                loglik_trace: vec![-10.0, -9.5],
                ..Default::default()
            },
            PerStartDiagnostics {
                algo: Some(DiagAlgo::Pmmh),
                completed: true,
                acc_rate: Some(0.4),
                loglik_trace: vec![-11.0, -10.5],
                ..Default::default()
            },
            PerStartDiagnostics {
                algo: Some(DiagAlgo::Pmmh),
                completed: false,
                acc_rate: None,
                loglik_trace: vec![],
                ..Default::default()
            },
        ];
        let agg = CellDiagnostics::aggregate(&starts, &[-9.5, -10.5, f64::NEG_INFINITY]);
        assert_eq!(agg.starts_n_completed, 2);
        // -Inf gets filtered out of spread.
        assert!((agg.loglik_spread_starts - 1.0).abs() < 1e-9);
        // Acc-rate aggregate uses the two completed chains.
        assert!((agg.acc_rate_avg - 0.35).abs() < 1e-9);
    }

    #[test]
    fn toml_roundtrip() {
        let d = PerStartDiagnostics {
            algo: Some(DiagAlgo::Pmmh),
            completed: true,
            acc_rate: Some(0.42),
            iterations_used: None,
            cooling_final: None,
            loglik_trace: vec![-10.0, -9.5, -9.0],
        };
        let body = format!(
            "final_loglik = -9.0\n[diagnostics]\n{}",
            d.to_toml_fragment(),
        );
        let doc: toml::Value = toml::from_str(&body)
            .expect("diagnostics fragment must round-trip");
        let back = PerStartDiagnostics::from_toml(&doc);
        assert_eq!(back.algo, Some(DiagAlgo::Pmmh));
        assert!(back.completed);
        assert_eq!(back.acc_rate, Some(0.42));
        assert_eq!(back.loglik_trace.len(), 3);
    }

    #[test]
    fn from_toml_handles_missing_block() {
        // Pre-gh#74 mle.toml files have no [diagnostics] table.
        // Parsing returns the default record so the rollup doesn't
        // crash on cached-from-old-version content.
        let doc: toml::Value = toml::from_str(
            "final_loglik = -9.0\n[focal]\nx = 1.0\n[mle]\nbeta = 0.3\n"
        ).unwrap();
        let d = PerStartDiagnostics::from_toml(&doc);
        assert!(d.algo.is_none());
        assert!(!d.completed);
        assert!(d.acc_rate.is_none());
        assert!(d.loglik_trace.is_empty());
    }

    #[test]
    fn nlopt_path_produces_nan_diagnostics_but_finite_completed() {
        // NLopt doesn't supply acc_rate, iterations_used, cooling_final,
        // or a loglik_trace. The aggregate must still produce a row;
        // algorithm-irrelevant columns render as NaN.
        let starts: Vec<PerStartDiagnostics> = (0..4).map(|_| PerStartDiagnostics {
            algo: Some(DiagAlgo::Nlopt),
            completed: true,
            ..Default::default()
        }).collect();
        let agg = CellDiagnostics::aggregate(&starts, &[-9.0, -9.1, -9.2, -9.05]);
        assert!(agg.acc_rate_avg.is_nan());
        assert!(agg.acc_rate_min.is_nan());
        assert!(agg.loglik_rhat_starts.is_nan(),
            "no traces → Rhat NaN");
        assert!(agg.iterations_used.is_nan());
        assert!(agg.cooling_final.is_nan());
        assert!(agg.loglik_spread_starts.is_finite());
        assert_eq!(agg.starts_n_completed, 4);
    }

    #[test]
    fn diag_columns_match_render_tsv_row_width() {
        let agg = CellDiagnostics {
            acc_rate_avg: 0.3,
            acc_rate_min: 0.2,
            loglik_spread_starts: 1.5,
            loglik_rhat_starts: 1.02,
            starts_n_completed: 3,
            iterations_used: f64::NAN,
            cooling_final: f64::NAN,
        };
        let row = agg.render_tsv_row();
        let cols: Vec<&str> = row.split('\t').collect();
        assert_eq!(cols.len(), DIAG_COLUMNS.len(),
            "DIAG_COLUMNS width must equal render_tsv_row column count");
    }
}
