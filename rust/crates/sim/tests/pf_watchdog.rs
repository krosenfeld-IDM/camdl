//! gh#110 — particle filter degeneracy watchdog integration test.
//!
//! Construct a contrived SIR chain-binomial model where the
//! dynamics blow up (R0 ~ 50) and feed it observations that the
//! likelihood cannot reconcile. ESS collapses within a handful of
//! observation windows. The watchdog must return
//! `Err(SimError::PFDegenerate { kind: EssCollapsed, .. })` and not
//! hang past a generous wall-clock budget.
//!
//! The acceptance criterion on gh#110 is explicit: this kind of
//! pathology must surface within ~5 seconds, not 30+ minutes.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Instant;

use ir::{
    expr::{BinOpExpr, BinOpWrap, BinOp, Expr, ParamExpr, PopExpr, ConstExpr},
    model::{Compartment, CompartmentKind, InitialConditions, OutputConfig, OutputSchedule, SimulationConfig},
    parameter::Parameter,
    transition::{Transition, StoichiometryEntry, DrawMethod},
    Model,
};
use sim::{
    compiled_model::CompiledModel,
    error::{PFDegenerateKind, SimError},
    inference::{
        obs_loglik::poisson_logpmf,
        particle_filter::bootstrap_filter,
        ChainBinomialProcess,
        traits::{ObservationModel, SMCConfig},
        ParticleState,
    },
    rng::StatefulRng,
};

/// Observe compartment index 2 (I) with Poisson likelihood. Used to
/// build a deliberately mis-specified problem where the model predicts
/// a huge epidemic and the data is flat zero.
struct PoissonOnIObs {
    observations: Vec<f64>,
    obs_times: Vec<f64>,
}

impl ObservationModel<ParticleState> for PoissonOnIObs {
    fn log_likelihood(&self, state: &ParticleState, obs_idx: usize, _params: &[f64]) -> f64 {
        // Observe I (index 2). Clamp tiny mean to avoid -inf so the
        // failure mode is ESS collapse, not All-Particles-Dead via
        // a -inf swarm. The watchdog must catch ESS collapse first.
        let predicted = (state.counts[2] as f64).max(0.1);
        poisson_logpmf(self.observations[obs_idx], predicted)
    }
    fn n_observations(&self) -> usize { self.observations.len() }
    fn obs_time(&self, obs_idx: usize) -> f64 { self.obs_times[obs_idx] }
    fn n_streams(&self) -> usize { 1 }
    fn sample(&self, _: &ParticleState, _: usize, _: &[f64], _: &mut StatefulRng) -> Vec<f64> { vec![] }
    fn mean(&self, _: &ParticleState, _: usize, _: &[f64]) -> Vec<f64> { vec![] }
}

/// Pathological SIR: R0 = beta/gamma ≈ 50 with N=1000 and only S0
/// initial pop. Every particle's I count explodes within a few days;
/// observations of "I = 0" are then astronomically unlikely under
/// the simulator. The PF re-weights kill all but one particle per
/// window, ESS goes to ~1, and stays there.
fn pathological_sir_model() -> (CompiledModel, Vec<f64>) {
    let beta = 5.0;   // contacts/day
    let gamma = 0.1;  // 1/recovery_days → R0 = 50
    let n_pop = 1000.0;

    let mut ic = HashMap::new();
    ic.insert("S".into(), n_pop - 1.0);
    ic.insert("I".into(), 1.0);
    ic.insert("R".into(), 0.0);

    // beta * S * I / N
    let infection_rate = Expr::BinOp(BinOpWrap { bin_op: BinOpExpr {
        op: BinOp::Div,
        left: Box::new(Expr::BinOp(BinOpWrap { bin_op: BinOpExpr {
            op: BinOp::Mul,
            left: Box::new(Expr::BinOp(BinOpWrap { bin_op: BinOpExpr {
                op: BinOp::Mul,
                left: Box::new(Expr::Param(ParamExpr { param: "beta".into() })),
                right: Box::new(Expr::Pop(PopExpr { pop: "S".into() })),
            }})),
            right: Box::new(Expr::Pop(PopExpr { pop: "I".into() })),
        }})),
        right: Box::new(Expr::Const(ConstExpr { value: n_pop })),
    }});
    // gamma * I
    let recovery_rate = Expr::BinOp(BinOpWrap { bin_op: BinOpExpr {
        op: BinOp::Mul,
        left: Box::new(Expr::Param(ParamExpr { param: "gamma".into() })),
        right: Box::new(Expr::Pop(PopExpr { pop: "I".into() })),
    }});

    let model = Model {
        name: "pathological_sir_pf".into(),
        version: "0.3".into(),
        time_unit: "days".into(),
        description: None,
        origin: None, origin_rata_die: None,
        compartments: vec![
            Compartment { name: "S".into(), kind: CompartmentKind::Integer },
            Compartment { name: "R".into(), kind: CompartmentKind::Integer },
            Compartment { name: "I".into(), kind: CompartmentKind::Integer },
        ],
        transitions: vec![
            Transition {
                name: "infection".into(),
                stoichiometry: vec![
                    StoichiometryEntry("S".into(), -1),
                    StoichiometryEntry("I".into(), 1),
                ],
                rate: infection_rate,
                metadata: None,
                draw_method: DrawMethod::Poisson,
                rate_grad: Default::default(),
                lineage: None,
            },
            Transition {
                name: "recovery".into(),
                stoichiometry: vec![
                    StoichiometryEntry("I".into(), -1),
                    StoichiometryEntry("R".into(), 1),
                ],
                rate: recovery_rate,
                metadata: None,
                draw_method: DrawMethod::Poisson,
                rate_grad: Default::default(),
                lineage: None,
            },
        ],
        ode_equations: vec![],
        time_functions: vec![],
        tables: vec![],
        interventions: vec![],
        observations: vec![],
        parameters: vec![
            Parameter { name: "beta".into(), value: Some(beta), bounds: None, prior: None, transform: None, initial_value: None, param_kind: None, param_dim: None, hierarchical: None },
            Parameter { name: "gamma".into(), value: Some(gamma), bounds: None, prior: None, transform: None, initial_value: None, param_kind: None, param_dim: None, hierarchical: None },
        ],
        initial_conditions: InitialConditions::Explicit(ic),
        output: OutputConfig {
            times: OutputSchedule::AtTimes(vec![0.0, 50.0]),
            format: "tsv".into(), trajectory: true, observations: false,
        },
        simulation: SimulationConfig {
            t_start: 0.0, t_end: 50.0, time_semantics: "continuous".into(),
            dt: Some(1.0), rng_seed: Some(1),
        },
        presets: vec![],
        model_structure: None, balance: None, identity_tracked_compartments: vec![],
    };
    let compiled = CompiledModel::new(model).unwrap();
    let params = compiled.default_params.clone();
    (compiled, params)
}

/// gh#110 acceptance: a contrived chain-binomial test model that
/// triggers ESS collapse fast must return `Err(SimError::PFDegenerate
/// { kind: EssCollapsed })` from `bootstrap_filter` within 5 seconds
/// of test wall-clock, NOT a hang.
#[test]
fn bootstrap_filter_bails_on_ess_collapse() {
    let (compiled, params) = pathological_sir_model();
    let compiled = Arc::new(compiled);
    let process = ChainBinomialProcess::new(compiled.clone(), 1.0);

    // 50 daily observations of "I = 0" against a model whose I count
    // hits hundreds by day 5. Every particle is astronomically
    // unlikely after the first few obs; ESS collapses immediately.
    let obs_times: Vec<f64> = (1..=50).map(|k| k as f64).collect();
    let observations: Vec<f64> = vec![0.0; 50];
    let obs_model = PoissonOnIObs { observations, obs_times };

    let config = SMCConfig {
        n_particles: 200, dt: 1.0, t_start: 0.0,
        skip_first_obs_from_loglik: false,
        record_ancestry: false, record_prequential: false,
    };

    let t0 = Instant::now();
    let res = bootstrap_filter(&process, &obs_model, &params, &config, 42);
    let elapsed = t0.elapsed();

    // Acceptance criterion: <5s, not a hang.
    assert!(elapsed.as_secs() < 5,
        "watchdog must bail within 5s; took {:?}", elapsed);

    match res {
        Err(SimError::PFDegenerate { kind, obs_window, elapsed_s: _ }) => {
            // The specific kind we expect on this pathology is
            // EssCollapsed. AllParticlesDead is also acceptable
            // (limit case of the same collapse), but WallClockExceeded
            // would mean the watchdog didn't see ESS collapse and we
            // saved the user only by the wall-clock fallback —
            // surface that as a failure of the K-window detector.
            match kind {
                PFDegenerateKind::EssCollapsed { last_ess } => {
                    assert!(last_ess.iter().all(|&e| e <= sim::inference::degeneracy::ESS_FLOOR),
                        "ESS history at bail must all be at or below the floor: {:?}", last_ess);
                }
                PFDegenerateKind::AllParticlesDead => {
                    // Limit case of ESS collapse — acceptable.
                }
                PFDegenerateKind::WallClockExceeded => {
                    panic!("expected EssCollapsed (or AllParticlesDead); \
                            watchdog fell through to wall-clock, which means \
                            the ESS detector didn't fire. obs_window={}", obs_window);
                }
            }
            assert!(obs_window < obs_model.n_observations(),
                "obs_window must be a valid index into the obs series");
        }
        Err(other) => panic!("expected SimError::PFDegenerate, got {:?}", other),
        Ok(r) => panic!(
            "expected PFDegenerate error; PF returned loglik={} with ESS trace {:?}",
            r.log_likelihood, r.ess_trace,
        ),
    }
}
