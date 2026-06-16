//! gh#208 regression: a transition rate that goes NEGATIVE on the Gillespie
//! *sparse* propensity-update path must raise `SimError::NegativePropensity`,
//! consistent with the full `eval_propensities` path and every other backend —
//! it must NOT be silently clamped to 0 by `eval_one`'s `.max(0.0)`.
//!
//! Fixture: `tests/fixtures/regression/gh208_sparse_negative_rate.ir.json`
//! (SIR + waning; `recover` rate `gamma * I * (cap - I) / N` crosses zero once
//! `I > cap = 9`). The model is non-absorbing at t=0, so the initial full
//! propensity evaluation does not catch the negative rate — only the incremental
//! sparse update does, where the clamp lives. `wane` keeps `lambda_total > 0`
//! across the crossing so the absorbing-state safety-net recompute does not fire
//! on every seed, making the silent clamp observable (seed-dependent).
//!
//! On current code, seeds 6 and 16 complete silently (the negative `recover`
//! rate is clamped to 0, the transition vanishes, `I` parks near the cap); the
//! other seeds happen to drive `lambda_total <= 0` and get caught by the
//! safety-net full recompute. After the fix, the sparse path errors on every
//! crossing seed. (Process post-mortem:
//! docs/dev/incidents/2026-06-16-gillespie-silent-wrong-test-sidestep.md.)

use std::path::PathBuf;

use sim::{
    compiled_model::CompiledModel,
    config::{GillespieConfig, SimConfig},
    error::SimError,
    simulate::Simulate,
    GillespieSim,
};

/// Seeds that complete *silently* on the buggy code (the negative `recover` rate
/// is clamped on the sparse update rather than erroring). These are the RED
/// seeds: `is_err()` is false today, true after the fix.
const SILENT_SEEDS: &[u64] = &[6, 16];

fn fixture_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../../tests/fixtures/regression/ir/gh208_sparse_negative_rate.ir.json")
}

fn load() -> ir::Model {
    let path = fixture_path();
    let contents = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("read {:?}: {}", path, e));
    ir::from_str(&contents).unwrap_or_else(|e| panic!("parse gh208 fixture: {}", e))
}

fn gillespie_config(model: &ir::Model) -> SimConfig {
    SimConfig::Gillespie(GillespieConfig {
        t_start: model.simulation.t_start,
        t_end: model.simulation.t_end,
        output_dt: None,
    })
}

/// Override a parameter's baked value in place (mirrors `apply_baseline` in
/// gillespie_invariants.rs). Used to build the negative control.
fn set_param(model: &mut ir::Model, name: &str, v: f64) {
    let p = model
        .parameters
        .iter_mut()
        .find(|p| p.name == name)
        .unwrap_or_else(|| panic!("no parameter {name}"));
    p.value = p.value.with_value(v);
}

#[test]
fn sparse_negative_rate_errors_not_clamped() {
    let compiled = CompiledModel::new(load()).expect("compile gh208 fixture");
    let params = compiled.default_params.clone();
    let config = gillespie_config(&compiled.model);

    for &seed in SILENT_SEEDS {
        let result = GillespieSim.run(&compiled, &params, seed, &config);
        match result {
            Err(SimError::NegativePropensity { transition, value, .. }) => {
                assert_eq!(
                    transition, "recover",
                    "seed {seed}: errored on the wrong transition ({transition})"
                );
                assert!(
                    value < 0.0,
                    "seed {seed}: NegativePropensity value should be negative, got {value}"
                );
            }
            Err(other) => panic!(
                "seed {seed}: expected NegativePropensity, got a different error: {other:?}"
            ),
            Ok(_) => panic!(
                "seed {seed}: gillespie completed silently — the `recover` rate went \
                 negative on the sparse update and was clamped to 0 instead of \
                 raising NegativePropensity (gh#208)."
            ),
        }
    }
}

/// Negative control: with `cap` large enough that `(cap - I)` never goes
/// negative, the same model has all-non-negative rates and must complete
/// normally on the very seeds that error above. Without this the test could
/// pass vacuously (e.g. if `eval_one` started erroring unconditionally).
#[test]
fn non_crossing_rate_still_completes() {
    let mut model = load();
    set_param(&mut model, "cap", 1000.0);
    let compiled = CompiledModel::new(model).expect("compile control");
    let params = compiled.default_params.clone();
    let config = gillespie_config(&compiled.model);

    for &seed in SILENT_SEEDS {
        let result = GillespieSim.run(&compiled, &params, seed, &config);
        assert!(
            result.is_ok(),
            "control seed {seed}: rates stay >= 0 (cap=1000), the run must complete, \
             got {:?}",
            result.err()
        );
    }
}
