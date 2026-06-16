//! gh#166 Phase C: adaptive rk45 (Dopri5) correctness gates.
//!
//! - C5 (internal agreement): fixed-RK4 at a fine `dt` and rk45 at a tight
//!   `(atol, rtol)` must agree on BOTH prevalence and incidence at the output
//!   grid. This is the backstop on the DOPRI5 tableau: a wrong coefficient makes
//!   rk45 diverge from the (independently-correct) fine-dt RK4 and fails here.
//! - C7 (determinism): same `(model, atol, rtol)` → byte-identical trajectory.
//! - C3 (capability gate): rk45 rejects an `Expr::Dt` / RUNTIME_DT model (no
//!   single fixed step) with an honest error, never a silent rk4 fallback.

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
    let json = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {path}: {e}"));
    ir::from_str(&json).unwrap_or_else(|e| panic!("parse {name}: {e}"))
}

fn run(model: ir::Model, dt: f64, t_end: f64) -> sim::state::Trajectory {
    let compiled = CompiledModel::new(model).expect("compile");
    let params = compiled.default_params.clone();
    OdeSim
        .run(&compiled, &params, 0, &SimConfig::Ode(OdeConfig { t_start: 0.0, t_end, dt }))
        .expect("ode run")
}

fn traj_hash(t: &sim::state::Trajectory) -> u64 {
    let mut h = 0xcbf29ce484222325u64;
    let mut mix = |b: u64| { h ^= b; h = h.wrapping_mul(0x100000001b3); };
    for s in &t.snapshots {
        mix(s.t.to_bits());
        for &c in &s.int_state.counts { mix(c as u64); }
        for &v in &s.real_state.values { mix(v.to_bits()); }
        for &f in s.flows.as_real() { mix(f.to_bits()); }
    }
    h
}

#[test]
fn rk45_agrees_with_fine_rk4_on_prevalence_and_incidence() {
    for (name, t_end) in [("sir", 60.0), ("seir", 80.0)] {
        // Fixed RK4 at a fine dt — the accurate, independent reference.
        let mut m_rk4 = oracle_model(name);
        m_rk4.simulation.integrator = ir::model::Integrator::Rk4;
        let rk4 = run(m_rk4, 0.01, t_end);

        // Adaptive rk45 at a tight tolerance.
        let mut m_rk45 = oracle_model(name);
        m_rk45.simulation.integrator = ir::model::Integrator::Rk45 { atol: Some(1e-10), rtol: Some(1e-10) };
        let rk45 = run(m_rk45, 1.0, t_end);

        assert_eq!(
            rk4.snapshots.len(),
            rk45.snapshots.len(),
            "{name}: rk4 and rk45 must emit at the same output grid"
        );
        let (mut worst_prev, mut worst_inc) = (0.0f64, 0.0f64);
        for (a, b) in rk4.snapshots.iter().zip(&rk45.snapshots) {
            assert!((a.t - b.t).abs() < 1e-9, "{name}: grid times must match");
            // Prevalence (rounded integer counts): agree to within snapshot rounding.
            for (x, y) in a.int_state.counts.iter().zip(&b.int_state.counts) {
                let d = (*x - *y).abs() as f64;
                worst_prev = worst_prev.max(d);
                assert!(d <= 1.0, "{name}: prevalence rk4={x} vs rk45={y} at t={} (Δ {d})", a.t);
            }
            // Incidence (unrounded flows): TIGHT — this is the tableau backstop.
            for (fx, fy) in a.flows.as_real().iter().zip(b.flows.as_real()) {
                let tol = 1e-2 + 1e-4 * fx.abs();
                let d = (fx - fy).abs();
                worst_inc = worst_inc.max(d / tol);
                assert!(
                    d <= tol,
                    "{name}: incidence rk4={fx} vs rk45={fy} at t={} (Δ {d:.3e} > tol {tol:.3e}) \
                     — a DOPRI5 tableau error would surface here",
                    a.t
                );
            }
        }
        eprintln!(
            "{name}: rk4(dt=0.01) vs rk45(tol=1e-10) — worst prevalence Δ {worst_prev}, \
             worst incidence {:.2}% of tol",
            100.0 * worst_inc
        );
    }
}

#[test]
fn rk45_is_deterministic() {
    let mut m = oracle_model("sir");
    m.simulation.integrator = ir::model::Integrator::Rk45 { atol: Some(1e-8), rtol: Some(1e-6) };
    let h1 = traj_hash(&run(m.clone(), 1.0, 60.0));
    let h2 = traj_hash(&run(m, 1.0, 60.0));
    assert_eq!(h1, h2, "rk45 must be byte-identical for the same (model, atol, rtol)");
}

#[test]
fn rk45_takes_adaptive_steps_not_just_the_grid() {
    // Sanity: rk45 must actually integrate (produce a non-trivial epidemic),
    // not degenerate. Peak infections should be a real fraction of N0=100000.
    let mut m = oracle_model("sir");
    m.simulation.integrator = ir::model::Integrator::Rk45 { atol: Some(1e-8), rtol: Some(1e-6) };
    let traj = run(m, 1.0, 60.0);
    let peak_i = traj.snapshots.iter().map(|s| s.int_state.counts[1]).max().unwrap();
    assert!(peak_i > 1000, "rk45 SIR epidemic should peak well above 1000 infections, got {peak_i}");
}

#[test]
fn rk45_rejects_runtime_dt_model() {
    let path = format!(
        "{}/../../../tests/fixtures/corner_cases/ir/dt_rate.ir.json",
        env!("CARGO_MANIFEST_DIR")
    );
    let mut m = ir::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    m.simulation.integrator = ir::model::Integrator::Rk45 { atol: None, rtol: None };
    let compiled = CompiledModel::new(m).expect("compile dt_rate");
    let params = compiled.default_params.clone();
    let res = OdeSim.run(
        &compiled,
        &params,
        0,
        &SimConfig::Ode(OdeConfig { t_start: 0.0, t_end: 2.0, dt: 1.0 }),
    );
    assert!(res.is_err(), "rk45 must reject a dt-in-rate (RUNTIME_DT) model, not silently run rk4");
    let msg = format!("{:?}", res.unwrap_err());
    assert!(
        msg.contains("rk45") && msg.to_lowercase().contains("dt"),
        "the error must explain the rk45 / dt-in-rate conflict, got: {msg}"
    );
}
