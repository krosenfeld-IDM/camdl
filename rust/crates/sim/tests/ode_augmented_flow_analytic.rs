//! gh#166 Phase B (proposal gate #6): analytic correctness of the augmented
//! ODE flow.
//!
//! The fixture `ode_linear_rate_flow` has one transition whose rate is
//! `beta * time` — linear in `t` and INDEPENDENT of state. The cumulative flow
//! over `[a, b]` is therefore the closed form `∫ beta·t dt = beta·(b² − a²)/2`.
//!
//! RK4 integrates a linear integrand EXACTLY (its Simpson-rule weights are exact
//! for polynomials up to cubic), so the augmented flow must equal the analytic
//! value to machine precision. The old left-rectangle Euler flow — `rate(a)·dt`
//! = `beta·a·dt` — is wrong by `beta·dt/2` per unit step, i.e. O(dt). This test
//! pins that the augmented flow is the high-order one (and that it is NOT the
//! Euler value, so the assertion is not vacuous).

use sim::{
    compiled_model::CompiledModel,
    config::{OdeConfig, SimConfig},
    simulate::Simulate,
    OdeSim,
};

const SEED: u64 = 7;

fn load_linear_rate() -> (CompiledModel, Vec<f64>, f64) {
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../../tests/fixtures/ode_flow/ode_linear_rate_flow.ir.json"
    );
    let json = std::fs::read_to_string(path).unwrap_or_else(|e| panic!("read {path}: {e}"));
    let mut model = ir::from_str(&json).expect("parse ode_linear_rate_flow IR");
    // Apply the baseline scenario (beta = 2.0, Src0 = 1e6) into the estimated
    // params, exactly as the golden gates do.
    let preset = model.presets.first().cloned().expect("a baseline scenario");
    let beta = *preset.params.get("beta").expect("beta in preset");
    for p in &mut model.parameters {
        if let Some(&v) = preset.params.get(&p.name) {
            p.value = p.value.with_value(v);
        }
    }
    let compiled = CompiledModel::new(model).expect("compile ode_linear_rate_flow");
    let params = compiled.default_params.clone();
    (compiled, params, beta)
}

#[test]
fn augmented_flow_matches_analytic_linear_integral() {
    let (compiled, params, beta) = load_linear_rate();

    // Single "accumulate" transition.
    assert_eq!(compiled.model.transitions[0].name, "accumulate");
    // No Expr::Dt in the rate → augmented (RK4) flow path, not the Euler path.
    assert!(
        !compiled
            .required_capabilities()
            .contains(sim::Capabilities::RUNTIME_DT),
        "linear-rate model must NOT be RUNTIME_DT (so it uses the augmented flow path)"
    );

    // dt = 1.0 == the output step: one substep per output interval. RK4 is exact
    // for the linear integrand regardless of dt, so this is the cleanest setting.
    let cfg = SimConfig::Ode(OdeConfig { t_start: 0.0, t_end: 10.0, dt: 1.0 });
    let traj = OdeSim.run(&compiled, &params, SEED, &cfg).expect("ode run");

    let mut cumulative = 0.0_f64;
    let mut checked = 0;
    for snap in &traj.snapshots {
        let k = snap.t.round() as i64;
        if k == 0 {
            // initial snapshot: flow reset, zero.
            assert_eq!(snap.flows.as_real()[0], 0.0, "t=0 flow must be 0");
            continue;
        }
        // Per-interval flow over [k-1, k].
        let a = (k - 1) as f64;
        let b = k as f64;
        let got = snap.flows.as_real()[0];
        let analytic = beta * (b * b - a * a) / 2.0; // ∫_{a}^{b} beta·t dt
        let euler = beta * a; // left-rectangle: rate(a)·dt = beta·a·1
        assert!(
            (got - analytic).abs() < 1e-9,
            "augmented per-interval flow over [{a},{b}] must equal the analytic \
             integral {analytic}, got {got} (Δ {:.3e})",
            (got - analytic).abs()
        );
        // Non-vacuous: the augmented value is NOT the Euler value (they differ by
        // exactly beta/2 = {} here).
        assert!(
            (got - euler).abs() > beta / 4.0,
            "test is vacuous if augmented == Euler: got {got}, euler {euler}, beta {beta}"
        );
        cumulative += got;
        checked += 1;
    }
    assert_eq!(checked, 10, "expected 10 per-interval flows (t=1..10)");

    // Cumulative over [0,10] = beta·T²/2.
    let analytic_total = beta * 10.0 * 10.0 / 2.0;
    assert!(
        (cumulative - analytic_total).abs() < 1e-7,
        "cumulative flow over [0,10] must equal beta·T²/2 = {analytic_total}, got {cumulative}"
    );
}
