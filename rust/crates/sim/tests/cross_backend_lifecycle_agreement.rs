//! Cross-backend within-substep LIFECYCLE AGREEMENT (M1 canonicalization).
//!
//! The three forward backends (chain_binomial, ode, gillespie) must apply the
//! within-substep effects in the SAME canonical order:
//!
//!     transitions → always_active events (from the start-of-step snapshot)
//!                 → interventions (on the post-event state) → balance
//!
//! Before M1, chain_binomial used this order but ode/gillespie ran the
//! INVERTED order (interventions first, then events reading the post-
//! intervention state). The divergence is only observable when an event and an
//! intervention are coincident AND the intervention reads a compartment the
//! event modified — every existing golden lacks such a model, so the hash gates
//! could not catch the divergence. This test is the missing agreement invariant.
//!
//! Fixture: `tests/fixtures/corner_cases/event_intervention_agree.camdl`
//! (IR baked with k=0, keep=0.5). The single transition `drain : A --> B @ k*A`
//! has rate ≡ 0, so NO stochastic flow occurs on any backend and the only state
//! change at t=5 is the coincident event + intervention. Counts are therefore
//! integer-exact and identical across the deterministic ODE and the three
//! stochastic backends.
//!
//! Hand-computed canonical lifecycle at the t=5 boundary:
//!     start of step:                       A = 50,  B = 0
//!     event  add(A, 100):                  A = 150, B = 0
//!     intervention transfer floor(A*0.5):  delta = floor(150 * 0.5) = 75
//!                                          A = 75,  B = 75   (reads post-event A)
//!     => A = 75, B = 75 for all t >= 5
//!
//! The pre-M1 inverted order would give transfer-first (floor(50*0.5)=25 →
//! A=25, B=25) then add → A=125, B=25. That value (A=125, B=25) is the negative
//! control: a backend stuck on the old order fails this test loudly.

use std::path::PathBuf;

use sim::{
    compiled_model::CompiledModel,
    config::{ChainBinomialConfig, GillespieConfig, OdeConfig, SimConfig},
    simulate::Simulate,
    ChainBinomialSim, GillespieSim, OdeSim,
};

const SEED: u64 = 42;

/// Canonical post-substep counts, hand-computed (see module header).
const EXPECTED_A: i64 = 75;
const EXPECTED_B: i64 = 75;
/// The pre-M1 inverted order would produce these — used only to make the
/// negative control explicit in the failure message.
const INVERTED_A: i64 = 125;
const INVERTED_B: i64 = 25;

fn fixture_ir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../../tests/fixtures/corner_cases/ir/event_intervention_agree.ir.json")
}

fn load() -> CompiledModel {
    let path = fixture_ir();
    let contents = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("read {:?}: {}", path, e));
    let model: ir::Model = ir::from_str(&contents)
        .unwrap_or_else(|e| panic!("parse event_intervention_agree: {}", e));
    CompiledModel::new(model).unwrap_or_else(|e| panic!("compile: {:?}", e))
}

fn local_idx(compiled: &CompiledModel, name: &str) -> usize {
    let g = *compiled.comp_index.get(name).expect("compartment");
    compiled.global_to_int[g].expect("integer compartment")
}

/// Counts of A and B at the FINAL snapshot — the canonical post-lifecycle state,
/// identical on every backend. Final-state agreement is a necessary but not
/// sufficient cross-backend invariant; the full-trajectory check below
/// (`full_trajectory_no_pre_event_leak_or_time_reversal`) is the load-bearing one.
fn final_a_b(compiled: &CompiledModel, sim: &dyn Simulate, cfg: &SimConfig) -> (i64, i64) {
    let params = compiled.default_params.clone();
    let traj = sim
        .run(compiled, &params, SEED, cfg)
        .expect("forward sim must succeed (zero-rate model)");
    let last = traj.snapshots.last().expect("at least one snapshot");
    let ia = local_idx(compiled, "A");
    let ib = local_idx(compiled, "B");
    (last.int_state.counts[ia], last.int_state.counts[ib])
}

#[test]
fn all_backends_agree_on_coincident_event_intervention() {
    let compiled = load();
    let t_start = compiled.model.simulation.t_start;
    let t_end = compiled.model.simulation.t_end;

    let backends: &[(&str, &dyn Simulate, SimConfig)] = &[
        (
            "chain_binomial",
            &ChainBinomialSim,
            SimConfig::ChainBinomial(ChainBinomialConfig { t_start, t_end, dt: 1.0 }),
        ),
        (
            "ode",
            &OdeSim,
            SimConfig::Ode(OdeConfig { t_start, t_end, dt: 1.0 }),
        ),
        (
            "gillespie",
            &GillespieSim,
            SimConfig::Gillespie(GillespieConfig { t_start, t_end, output_dt: None }),
        ),
    ];

    for (name, sim, cfg) in backends {
        let (a, b) = final_a_b(&compiled, *sim, cfg);
        assert!(
            a == EXPECTED_A && b == EXPECTED_B,
            "{name}: within-substep lifecycle order DIVERGED — got A={a}, B={b}, \
             expected the canonical A={EXPECTED_A}, B={EXPECTED_B} (event from \
             snapshot BEFORE intervention). A={INVERTED_A}, B={INVERTED_B} would \
             mean this backend still runs the pre-M1 inverted order \
             (intervention before event)."
        );
    }
}

/// The pre-event compartment state: before the t=5 boundary nothing has fired,
/// so A is unchanged at its init value and B is empty on every backend.
const PRE_EVENT_A: i64 = 50;
const PRE_EVENT_B: i64 = 0;
/// The coincident event + intervention both fire at t=5 (see fixture header).
const EVENT_T: f64 = 5.0;

/// gh#70 regression: full-trajectory cross-backend invariant, not just the final
/// snapshot. Two properties every backend must satisfy:
///
///   1. **Time never runs backward** — snapshot `t` is strictly non-decreasing.
///   2. **No pre-event state leak** — for every snapshot at `t < 5`, the state is
///      the untouched init (`A == 50`, `B == 0`); the `add(A, 100)` event and the
///      transfer intervention fire at `t = 5`, so nothing may appear earlier.
///
/// Before the fix, gillespie's absorbing-state branch flushes outputs only up to
/// `next_special` and then jumps `t` to the event time, stranding the output
/// cursor; the next boundary clip pulls `t` *backward* and records the post-event
/// state (`A = 150`) at an earlier output row. This test fails on gillespie row
/// `t = 2` today. chain_binomial and ode are correct. `final_a_b` (above) only
/// probed `snapshots.last()` — the one row neither defect corrupts — which is why
/// this divergence slipped through. (Process post-mortem:
/// docs/dev/incidents/2026-06-16-gillespie-silent-wrong-test-sidestep.md.)
#[test]
fn full_trajectory_no_pre_event_leak_or_time_reversal() {
    let compiled = load();
    let t_start = compiled.model.simulation.t_start;
    let t_end = compiled.model.simulation.t_end;

    let backends: &[(&str, &dyn Simulate, SimConfig)] = &[
        (
            "chain_binomial",
            &ChainBinomialSim,
            SimConfig::ChainBinomial(ChainBinomialConfig { t_start, t_end, dt: 1.0 }),
        ),
        (
            "ode",
            &OdeSim,
            SimConfig::Ode(OdeConfig { t_start, t_end, dt: 1.0 }),
        ),
        (
            "gillespie",
            &GillespieSim,
            SimConfig::Gillespie(GillespieConfig { t_start, t_end, output_dt: None }),
        ),
    ];

    let params = compiled.default_params.clone();
    let ia = local_idx(&compiled, "A");
    let ib = local_idx(&compiled, "B");

    for (name, sim, cfg) in backends {
        let traj = sim
            .run(&compiled, &params, SEED, cfg)
            .expect("forward sim must succeed (zero-rate model)");

        for w in traj.snapshots.windows(2) {
            assert!(
                w[1].t >= w[0].t,
                "{name}: trajectory time ran backward — snapshot {} followed by {} \
                 (gh#70: the absorbing-state boundary clip jumped t into the past).",
                w[0].t, w[1].t
            );
        }

        for snap in &traj.snapshots {
            if snap.t < EVENT_T - 1e-9 {
                let a = snap.int_state.counts[ia];
                let b = snap.int_state.counts[ib];
                assert!(
                    a == PRE_EVENT_A && b == PRE_EVENT_B,
                    "{name}: pre-event state leaked at t={} — got A={a}, B={b}, \
                     expected the untouched init A={PRE_EVENT_A}, B={PRE_EVENT_B}. \
                     The add(A,100) event + transfer fire at t={EVENT_T}; recording \
                     post-event state earlier is gh#70 (gillespie back-fills the \
                     event into pre-event output rows).",
                    snap.t
                );
            }
        }
    }
}

// The chain-vs-tau differential oracle that pinned the within-substep event
// read-source (start-of-step snapshot vs post-drain) lived here; with tau-leap
// dropped (scheduling-spine-v2 §D) the property is covered by pgas_event_density
// + the lifecycle audit, which exercise the same chain-side fusion.
