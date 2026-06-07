//! Wiring test for the #1-interim event-misfire guard
//! (`Schedule::reject_event_misfire`, exercised through the inference filters).
//!
//! The inference filters step EXACTLY to each observation time (StepPolicy::Exact).
//! When an observation is off the dt grid, the final substep of that window is
//! shortened, so its end lands off the `round(t/dt)` grid that always-active
//! events key their firing on — the event fires on the wrong step, a silent
//! likelihood error. The guard refuses such a configuration loudly.
//!
//! This test proves the guard is WIRED into `bootstrap_filter`: the same model
//! is accepted with on-grid observations and rejected with off-grid ones, and a
//! model WITHOUT always-active events is accepted either way (the predicate
//! itself is unit-tested exhaustively in `schedule.rs`). The accept-on-grid case
//! is the over-rejection control: always-active events with on-grid obs (the
//! common importation/seeding fit) must still run.

use std::collections::HashMap;
use std::sync::Arc;
use ir::{
    expr::{BinOpExpr, BinOpWrap, BinOp, Expr, ParamExpr, PopExpr},
    intervention::{Action, AddAction, Intervention, InterventionSchedule},
    model::{Compartment, CompartmentKind, InitialConditions, OutputConfig, OutputSchedule, SimulationConfig},
    parameter::Parameter,
    transition::{Transition, StoichiometryEntry, DrawMethod},
    Model,
};
use sim::{
    compiled_model::CompiledModel,
    inference::{
        obs_loglik::poisson_logpmf,
        particle_filter::bootstrap_filter,
        ChainBinomialProcess,
        traits::{ObservationModel, SMCConfig},
        ParticleState,
    },
};

struct PoissonPrevalenceObs {
    observations: Vec<f64>,
    obs_times: Vec<f64>,
}

impl ObservationModel<ParticleState> for PoissonPrevalenceObs {
    fn log_likelihood(&self, state: &ParticleState, obs_idx: usize, _params: &[f64]) -> f64 {
        poisson_logpmf(self.observations[obs_idx], (state.counts[0] as f64).max(0.1))
    }
    fn n_observations(&self) -> usize { self.observations.len() }
    fn obs_time(&self, obs_idx: usize) -> f64 { self.obs_times[obs_idx] }
}

/// Pure-death N with optional always-active importation event (`add N += 1`
/// every integer step). The event makes `has_always_active_events()` true.
fn death_model(with_event: bool) -> CompiledModel {
    let interventions = if with_event {
        vec![Intervention {
            name: "importation".into(),
            base_name: None,
            schedule: InterventionSchedule::AtTimes((1..=10).map(|k| k as f64).collect()),
            actions: vec![Action::Add(AddAction {
                compartment: "N".into(),
                count: Expr::const_(1.0),
            })],
            always_active: true,
        }]
    } else {
        vec![]
    };
    let model = Model {
        name: "death_event_guard".into(),
        version: "0.3".into(),
        time_unit: "days".into(),
        description: None,
        origin: None, origin_rata_die: None,
        compartments: vec![Compartment { name: "N".into(), kind: CompartmentKind::Integer }],
        transitions: vec![Transition {
            name: "death".into(),
            stoichiometry: vec![StoichiometryEntry("N".into(), -1)],
            rate: Expr::BinOp(BinOpWrap {
                bin_op: BinOpExpr {
                    op: BinOp::Mul,
                    left: Box::new(Expr::Param(ParamExpr { param: "mu".into() })),
                    right: Box::new(Expr::Pop(PopExpr { pop: "N".into() })),
                },
            }),
            metadata: None,
            draw_method: DrawMethod::Poisson, rate_grad: Default::default(), lineage: None,
        }],
        ode_equations: vec![],
        time_functions: vec![],
        tables: vec![],
        interventions,
        observations: vec![],
        bindings: vec![],
        parameters: vec![
            Parameter { name: "mu".into(), value: Some(0.01), bounds: None, prior: None, transform: None, initial_value: None, param_kind: None, param_dim: None, hierarchical: None },
        ],
        initial_conditions: InitialConditions::Explicit({
            let mut m = HashMap::new(); m.insert("N".into(), 100.0); m
        }),
        output: OutputConfig {
            times: OutputSchedule::AtTimes(vec![0.0, 10.0]),
            format: "tsv".into(),
            trajectory: true,
            observations: false,
        },
        simulation: SimulationConfig {
            t_start: 0.0, t_end: 10.0, time_semantics: "continuous".into(),
            dt: Some(1.0), rng_seed: Some(42),
        },
        presets: vec![],
        model_structure: None, balance: None, identity_tracked_compartments: vec![],
    };
    CompiledModel::new(model).unwrap()
}

fn run(with_event: bool, obs_times: Vec<f64>) -> Result<f64, sim::SimError> {
    let compiled = Arc::new(death_model(with_event));
    let params = compiled.default_params.clone();
    let process = ChainBinomialProcess::new(compiled, 1.0);
    let obs_model = PoissonPrevalenceObs {
        observations: obs_times.iter().map(|&t| 100.0 * (-0.01 * t).exp()).collect(),
        obs_times,
    };
    let config = SMCConfig {
        n_particles: 100, dt: 1.0, t_start: 0.0,
        skip_first_obs_from_loglik: false, record_ancestry: false,
        record_prequential: false, pf_wallclock_disabled: false,
    };
    bootstrap_filter(&process, &obs_model, &params, &config, 42).map(|r| r.log_likelihood)
}

/// RED: with an always-active event, off-grid observations must be rejected at
/// filter setup. Pre-guard the filter ran and returned a (silently-misfiring)
/// log-likelihood.
#[test]
fn pf_rejects_off_grid_obs_with_always_active_event() {
    let err = run(true, vec![3.5, 7.5])
        .expect_err("off-grid obs + always-active event must be rejected, not silently misfire");
    let msg = err.to_string();
    assert!(msg.contains("always-active"), "message should name the cause: {msg}");
    assert!(msg.contains("3.5"), "message should name the off-grid time: {msg}");
}

/// CONTROL (over-rejection guard): the SAME event model with on-grid obs must
/// still run — always-active events with on-grid observations are the common
/// importation/seeding fit and must not be refused.
#[test]
fn pf_accepts_on_grid_obs_with_always_active_event() {
    let ll = run(true, vec![4.0, 8.0]).expect("on-grid obs + event must run");
    assert!(ll.is_finite(), "log-likelihood should be finite, got {ll}");
}

/// CONTROL: a model WITHOUT always-active events is accepted with off-grid obs —
/// the guard keys on the event, not on off-grid obs alone.
#[test]
fn pf_accepts_off_grid_obs_without_event() {
    let ll = run(false, vec![3.5, 7.5]).expect("off-grid obs without event must run");
    assert!(ll.is_finite(), "log-likelihood should be finite, got {ll}");
}
