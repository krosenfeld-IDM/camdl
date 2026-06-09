//! Capability gate for `Expr::Dt`-in-a-rate (gh#54): a model whose transition
//! rate references the runtime substep `dt` requires a backend that realizes a
//! substep length (`EvalCtx.dt = dt_actual`). ODE (RK4 flow accumulation, see
//! `ode_dt_rate_flow.rs`) and chain_binomial (StepClock, see
//! `gate_dt_rate_exact_clip.rs`) provide it; Gillespie does NOT — its SSA loop
//! has no substep, so it freezes the `Expr::Dt` node to the nominal
//! `simulation.dt`-or-`1.0` (gillespie.rs:269/366). Before this gate that
//! produced a DIFFERENT trajectory on each backend with NO warning — exactly the
//! BALANCE failure mode (`Capabilities::BALANCE`, gh#audit-C3).
//!
//! This mirrors the BALANCE precedent: the requirement is auto-derived by
//! `CompiledModel::required_capabilities()` (walking the rate ASTs for
//! `Expr::Dt`), the backend `capabilities()` sets declare support, and the
//! dispatch gate (`required - backend.capabilities()`) rejects the mismatch.
//!
//! Fixture: `tests/fixtures/corner_cases/ir/dt_rate.ir.json` — its `infection`
//! rate carries an explicit `(dt / tau)` factor.

use sim::{
    compiled_model::CompiledModel,
    config::{ChainBinomialConfig, GillespieConfig, OdeConfig, SimConfig},
    simulate::Simulate,
    Capabilities, ChainBinomialSim, GillespieSim, OdeSim,
};

fn load_dt_rate() -> CompiledModel {
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../../tests/fixtures/corner_cases/ir/dt_rate.ir.json"
    );
    let json = std::fs::read_to_string(path).unwrap_or_else(|e| panic!("read {path}: {e}"));
    let model = ir::from_str(&json).expect("parse dt_rate IR");
    CompiledModel::new(model).expect("compile dt_rate")
}

/// The model's required capabilities must include RUNTIME_DT, because its
/// `infection` rate references `Expr::Dt` via the `(dt / tau)` factor.
#[test]
fn dt_in_rate_requires_runtime_dt_capability() {
    let compiled = load_dt_rate();
    let required = compiled.required_capabilities();
    assert!(
        required.contains(Capabilities::RUNTIME_DT),
        "a model whose rate references `dt` (Expr::Dt) must require RUNTIME_DT; \
         got {required:?}"
    );
}

/// Gillespie does not realize a substep dt, so dispatching the dt_rate model on
/// it must be rejected by the capability gate (`required - capabilities()` is
/// non-empty) rather than silently running with a frozen nominal dt.
#[test]
fn gillespie_rejects_dt_in_rate() {
    let compiled = load_dt_rate();
    let required = compiled.required_capabilities();
    let missing = required - GillespieSim.capabilities();
    assert!(
        missing.contains(Capabilities::RUNTIME_DT),
        "gillespie has no substep — a dt-in-rate model must be a capability \
         mismatch on gillespie; missing = {missing:?}"
    );
}

/// ODE and chain_binomial realize a substep dt, so they DO declare RUNTIME_DT
/// and the dt_rate model dispatches and runs on them.
#[test]
fn ode_and_chain_binomial_accept_and_run_dt_in_rate() {
    let compiled = load_dt_rate();
    let required = compiled.required_capabilities();
    let params = compiled.default_params.clone();
    let t_start = compiled.model.simulation.t_start;
    let t_end = compiled.model.simulation.t_end;

    for (name, sim, config) in [
        (
            "ode",
            &OdeSim as &dyn Simulate,
            SimConfig::Ode(OdeConfig { t_start, t_end, dt: 1.0 }),
        ),
        (
            "chain_binomial",
            &ChainBinomialSim as &dyn Simulate,
            SimConfig::ChainBinomial(ChainBinomialConfig { t_start, t_end, dt: 1.0 }),
        ),
    ] {
        assert!(
            sim.capabilities().contains(Capabilities::RUNTIME_DT),
            "{name} must declare RUNTIME_DT (it realizes a substep dt)"
        );
        assert!(
            (required - sim.capabilities()).is_empty(),
            "{name} must satisfy the dt_rate model's capabilities"
        );
        sim.run(&compiled, &params, 42, &config)
            .unwrap_or_else(|e| panic!("{name} should run the dt_rate model: {e}"));
    }
}

/// Guard against false positives: a model WITHOUT `Expr::Dt` in any rate must
/// NOT require RUNTIME_DT (so gillespie still runs ordinary SIR). Built inline
/// by dropping the dt factor would require recompiling; instead assert via a
/// gillespie run of a known dt-free corner-case fixture.
#[test]
fn dt_free_model_does_not_require_runtime_dt() {
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../../tests/fixtures/corner_cases/ir/off_grid_obs.ir.json"
    );
    let json = std::fs::read_to_string(path).unwrap_or_else(|e| panic!("read {path}: {e}"));
    let model = ir::from_str(&json).expect("parse off_grid_obs IR");
    let compiled = CompiledModel::new(model).expect("compile off_grid_obs");
    let required = compiled.required_capabilities();
    assert!(
        !required.contains(Capabilities::RUNTIME_DT),
        "a model with no Expr::Dt in any rate must not require RUNTIME_DT; \
         got {required:?}"
    );
    // And it must still run on gillespie (no spurious gate).
    let t_start = compiled.model.simulation.t_start;
    let t_end = compiled.model.simulation.t_end;
    assert!(
        (required - GillespieSim.capabilities()).is_empty(),
        "dt-free model must satisfy gillespie capabilities"
    );
    GillespieSim
        .run(
            &compiled,
            &compiled.default_params,
            42,
            &SimConfig::Gillespie(GillespieConfig { t_start, t_end, output_dt: None }),
        )
        .expect("gillespie should run the dt-free model");
}
