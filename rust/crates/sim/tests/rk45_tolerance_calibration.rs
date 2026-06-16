//! gh#166 C8 — adaptive-tolerance calibration (recorded, on-demand).
//!
//! Run with:  cargo test -p sim --test rk45_tolerance_calibration -- --ignored --nocapture
//!
//! For each canonical model we treat fixed-RK4 at a VERY fine dt as ground truth,
//! then ask: for a candidate (atol, rtol), how much does rk45's incidence perturb
//! the LOGLIK relative to truth? The loglik is what an inference fit scores, so
//! "tight enough" means the integration-induced |Δ loglik| is ≪ 1 nat (well below
//! Monte-Carlo / obs noise). We report that for the ecosystem defaults and our
//! placeholder so the default is chosen on evidence, not vibes — and NOT overtuned
//! to one model.
//!
//! Loglik proxy: take data y_k = round(λ_k^truth) at each obs time and transition,
//! and score it under both incidence means. The Poisson Δ per cell is
//! `y·ln(λ_rk45/λ_truth) − (λ_rk45 − λ_truth)` (the ln(y!) cancels). Summed over
//! all obs × transitions, |Δ| is the total loglik error a fit would inherit.

use sim::{
    compiled_model::CompiledModel,
    config::{OdeConfig, SimConfig},
    simulate::Simulate,
    OdeSim,
};

fn oracle_model(name: &str) -> ir::Model {
    let path = format!(
        "{}/../../../tests/external/ode_oracle/models/{}.ir.json",
        env!("CARGO_MANIFEST_DIR"),
        name
    );
    ir::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap()
}

/// Per-(obs, transition) incidence at the output grid.
fn incidence(model: ir::Model, dt: f64, t_end: f64) -> Vec<Vec<f64>> {
    let compiled = CompiledModel::new(model).unwrap();
    let params = compiled.default_params.clone();
    let traj = OdeSim
        .run(&compiled, &params, 0, &SimConfig::Ode(OdeConfig { t_start: 0.0, t_end, dt }))
        .unwrap();
    traj.snapshots.iter().skip(1).map(|s| s.flows.as_real().to_vec()).collect()
}

#[test]
#[ignore]
fn rk45_tolerance_calibration() {
    let models = [("sir", 60.0), ("seir", 80.0), ("tb", 365.0)];
    // (label, atol, rtol)
    let candidates = [
        ("scipy/MATLAB 1e-6/1e-3", 1e-6, 1e-3),
        ("deSolve     1e-6/1e-6", 1e-6, 1e-6),
        ("placeholder 1e-8/1e-6", 1e-8, 1e-6),
        ("tight       1e-10/1e-8", 1e-10, 1e-8),
    ];
    println!("\n{:<10} {:<24} {:>14}", "model", "(atol/rtol)", "|Δ loglik| nat");
    println!("{}", "-".repeat(52));
    for (name, t_end) in models {
        // Ground truth: fixed RK4 at a very fine dt.
        let truth = incidence(oracle_model(name), 0.005, t_end);
        for (label, atol, rtol) in candidates {
            let mut m = oracle_model(name);
            m.simulation.integrator = ir::model::Integrator::Rk45 { atol: Some(atol), rtol: Some(rtol) };
            let got = incidence(m, 1.0, t_end);
            let mut dll = 0.0f64;
            for (gt_row, rk_row) in truth.iter().zip(&got) {
                for (&lam_t, &lam_r) in gt_row.iter().zip(rk_row) {
                    if lam_t <= 0.0 || lam_r <= 0.0 { continue; }
                    let y = lam_t.round();
                    dll += y * (lam_r / lam_t).ln() - (lam_r - lam_t);
                }
            }
            println!("{:<10} {:<24} {:>14.3e}", name, label, dll.abs());
        }
    }
    println!();
}
