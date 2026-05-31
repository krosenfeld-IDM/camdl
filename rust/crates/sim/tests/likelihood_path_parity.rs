//! gh#139: `MultiStreamObsModel` has two independent likelihood
//! summation loops — the trait `log_likelihood` (PF/IF2/PMMH) and the
//! inherent `log_likelihood_from_flows_and_counts` (PGAS). They must
//! agree on the same `(flows, counts, data)`, since `ParticleState` is
//! exactly `{ counts, flow_accumulators }`.
//!
//! This is a CHARACTERIZATION test: it is green on the current code
//! (the two loops are byte-identical today) and LOCKS that invariant so
//! a future edit to one loop but not the other — the GH#6 /
//! incident-2026-04-22 class of bug, which has produced a ~100×
//! log-likelihood divergence twice — fails loudly here.
//!
//! After the gh#139 unification (trait method delegates to the flat
//! method) the invariant is structural, but the test stays as a guard.

use ir::expr::{Expr, BinOpExpr, BinOp, BinOpWrap, ParamExpr, PopExpr, ConstExpr};
use ir::observation::{
    Likelihood, NegBinomialLikelihood, ObservationModel, ObservationSchedule,
    Projection,
};
use ir::{Model, SimulationConfig};
use sim::compiled_model::CompiledModel;
use sim::inference::multi_stream_obs::{MultiStreamObsModel, StreamSpec, StreamProjection};
use sim::inference::types::ParticleState;
use sim::inference::traits::ObservationModel as ObservationModelTrait;
use std::sync::Arc;

/// S,I,R with a *state-dependent* observation likelihood:
/// `neg_binomial(mean = rho * I, r = k)`. The mean references a
/// compartment (`I`) via a `Pop` node, so the likelihood eval reads
/// `counts` — exactly the GH#6 case where a zero scratch silently broke
/// one path.
fn state_dependent_model() -> Model {
    let mut m = Model::default();
    m.name = "parity".into();
    m.time_unit = "days".into();
    m.compartments = vec!["S".into(), "I".into(), "R".into()];
    m.parameters = vec![
        ir::Parameter { name: "rho".into(), value: Some(0.3), ..Default::default() },
        ir::Parameter { name: "k".into(),   value: Some(5.0), ..Default::default() },
    ];
    m.transitions = vec![];
    m.observations = vec![ObservationModel {
        name: "cases".into(),
        schedule: ObservationSchedule::FromData,
        projection: Projection::CurrentPop("I".into()),
        likelihood: Likelihood::NegBinomial(NegBinomialLikelihood {
            // mean = rho * I  (Pop ref → reads counts)
            mean: Expr::BinOp(BinOpWrap { bin_op: BinOpExpr {
                op: BinOp::Mul,
                left: Box::new(Expr::Param(ParamExpr { param: "rho".into() })),
                right: Box::new(Expr::Pop(PopExpr { pop: "I".into() })),
            }}),
            dispersion: Expr::Const(ConstExpr { constant: 5.0 }),
        }),
    }];
    m.simulation = SimulationConfig {
        t_start: 0.0, t_end: 10.0, time_semantics: "continuous".into(),
        dt: Some(1.0), rng_seed: Some(1),
    };
    m
}

#[test]
fn pf_and_pgas_likelihood_paths_agree() {
    let model = state_dependent_model();
    let compiled = Arc::new(CompiledModel::new(model).unwrap());

    let spec = StreamSpec {
        ir_model: compiled.model.observations[0].clone(),
        projection: StreamProjection::CurrentPop(vec![compiled.comp_index["I"]]),
        observations: vec![12.0, 30.0],
        obs_times: vec![1.0, 5.0],
    };
    let obs_model = MultiStreamObsModel::new(vec![spec], compiled.clone()).unwrap();

    // Non-zero in BOTH fields so the identity exercises the full
    // ParticleState, not a degenerate all-zero case.
    let counts = vec![950i64, 40, 10];  // S, I, R
    let flows  = vec![7u64];            // a non-empty flow vector
    let params = [0.3f64, 5.0];         // rho, k

    let state = ParticleState { counts: counts.clone(), flow_accumulators: flows.clone() };

    for obs_idx in 0..2 {
        // PF/IF2/PMMH path (trait):
        let via_state = obs_model.log_likelihood(&state, obs_idx, &params);
        // PGAS path (flat arrays):
        let via_flat  =
            obs_model.log_likelihood_from_flows_and_counts(&flows, &counts, obs_idx, &params);

        assert!(via_state.is_finite(),
            "obs {obs_idx}: trait path must be finite, got {via_state}");
        assert!(via_flat.is_finite(),
            "obs {obs_idx}: flat path must be finite, got {via_flat}");
        // The load-bearing invariant: the two seams agree exactly.
        assert_eq!(via_state, via_flat,
            "obs {obs_idx}: PF/IF2 (trait) and PGAS (flat) likelihood paths \
             diverged — gh#139 / the GH#6 dual-loop class. state={via_state} flat={via_flat}");
    }

    // Negative control: the two observation indices have different data
    // (12 vs 30), so the likelihood must actually differ — proves the
    // test isn't passing on a trivial constant.
    let ll0 = obs_model.log_likelihood(&state, 0, &params);
    let ll1 = obs_model.log_likelihood(&state, 1, &params);
    assert!((ll0 - ll1).abs() > 1e-9,
        "different observed data must score differently (non-vacuous guard): {ll0} vs {ll1}");
}
