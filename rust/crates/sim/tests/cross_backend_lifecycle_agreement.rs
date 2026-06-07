//! Cross-backend within-substep LIFECYCLE AGREEMENT (M1 canonicalization).
//!
//! The four forward backends (chain_binomial, tau_leap, ode, gillespie) must
//! apply the within-substep effects in the SAME canonical order:
//!
//!     transitions → always_active events (from the start-of-step snapshot)
//!                 → interventions (on the post-event state) → balance
//!
//! Before M1, chain_binomial used this order but tau_leap/ode/gillespie ran the
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
    config::{ChainBinomialConfig, GillespieConfig, OdeConfig, SimConfig, TauLeapConfig},
    simulate::Simulate,
    ChainBinomialSim, GillespieSim, OdeSim, TauLeapSim,
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

/// Counts of A and B at the FINAL snapshot. The final snapshot is the robust
/// cross-backend probe: gillespie's absorbing-state output cadence back-fills
/// earlier output rows differently (it jumps to the t=5 boundary), but the
/// terminal state is the canonical post-lifecycle state on every backend.
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
            "tau_leap",
            &TauLeapSim,
            SimConfig::TauLeap(TauLeapConfig { t_start, t_end, dt: 0.5 }),
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

// ── Event READ-SOURCE FUSION (cycle 2a) ──────────────────────────────────────
//
// The strong fixture for the within-substep event delta read-source. A live
// transition `drain : A --> B @ k*A` DRAINS A every substep, while an
// always_active event `leak : transfer(fraction=f, from=A, to=C)` ALSO reads A.
// The event delta is floor(A * f) — value-dependent — so it DIFFERS depending on
// whether A is read pre-drain (snapshot, canonical) or post-drain (the pre-fix
// tau bug).
//
// HAND REASONING for the byte-equality assertion:
//   chain_binomial and tau_leap share the IDENTICAL Euler-multinomial kernel
//   (Binomial total-exit + proportional split, drawn in source-group order). At
//   the SAME dt and seed they consume the RNG in the SAME order and draw the
//   SAME transition flow. The events are RNG-free. Therefore the ONLY thing that
//   can make the two trajectories differ is the event READ-SOURCE:
//     - read from the start-of-step snapshot (pre-drain A): chain == tau, byte-
//       identical.
//     - read from post-drain A (the bug): tau diverges from chain at the first
//       coincident substep and the difference cascades.
//   So `chain_trajectory_hash == tau_trajectory_hash` at matched dt is a tight,
//   bit-exact PROOF that tau reads the snapshot — RED before the fusion fix,
//   GREEN after. (Confirmed empirically: pre-fix tau hash
//   0xd6ab8f44a5c6dc43 != chain 0x9f09ba4bd868112c; post-fix they are equal.)
//
// The event starts at t=1 (not t=0) so the backends agree on event CADENCE: a
// t=0 effect boundary makes tau land an extra zero-length substep that chain's
// step_one (which keys events to the END of each step) does not — a cadence
// difference orthogonal to the read-source this fixture isolates.
//
// ode (RK4) and gillespie (SSA) have DIFFERENT kernels, so they do NOT byte-
// match chain here and are deliberately excluded from the equality assertion;
// their baselines are pinned in gate_corner_case_baseline.rs.

fn drain_fusion_ir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../../tests/fixtures/corner_cases/ir/event_drain_fusion.ir.json")
}

fn load_drain_fusion() -> CompiledModel {
    let path = drain_fusion_ir();
    let contents = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("read {:?}: {}", path, e));
    let model: ir::Model = ir::from_str(&contents)
        .unwrap_or_else(|e| panic!("parse event_drain_fusion: {}", e));
    CompiledModel::new(model).unwrap_or_else(|e| panic!("compile: {:?}", e))
}

/// FNV-1a/64 over the full trajectory numeric content (counts + real values at
/// every snapshot time) — identical to the gate hashes so the two are
/// comparable.
fn trajectory_hash(traj: &sim::state::Trajectory) -> u64 {
    let mut h: u64 = 0xcbf29ce484222325;
    let mut mix = |bytes: &[u8]| {
        for &b in bytes {
            h ^= b as u64;
            h = h.wrapping_mul(0x100000001b3);
        }
    };
    for snap in &traj.snapshots {
        mix(&snap.t.to_bits().to_le_bytes());
        for &c in &snap.int_state.counts {
            mix(&c.to_le_bytes());
        }
        for &v in &snap.real_state.values {
            mix(&v.to_bits().to_le_bytes());
        }
    }
    h
}

#[test]
fn chain_and_tau_byte_identical_on_fused_event_read_source() {
    let compiled = load_drain_fusion();
    let params = compiled.default_params.clone();
    let t_start = compiled.model.simulation.t_start;
    let t_end = compiled.model.simulation.t_end;

    // MATCHED dt = 1.0 on both backends — the precondition for the identical
    // kernel to draw the identical flow in the identical RNG order.
    let chain = ChainBinomialSim
        .run(
            &compiled, &params, SEED,
            &SimConfig::ChainBinomial(ChainBinomialConfig { t_start, t_end, dt: 1.0 }),
        )
        .expect("chain_binomial forward sim must succeed");
    let tau = TauLeapSim
        .run(
            &compiled, &params, SEED,
            &SimConfig::TauLeap(TauLeapConfig { t_start, t_end, dt: 1.0 }),
        )
        .expect("tau_leap forward sim must succeed");

    let chain_hash = trajectory_hash(&chain);
    let tau_hash = trajectory_hash(&tau);

    // Negative control: the model must be NON-trivial — A actually drains and
    // the event actually transfers, so a vacuous (all-zero / no-flow) trajectory
    // can't pass the equality for the wrong reason.
    let chain_last = chain.snapshots.last().expect("at least one snapshot");
    let a_idx = local_idx(&compiled, "A");
    let b_idx = local_idx(&compiled, "B");
    let c_idx = local_idx(&compiled, "C");
    assert!(
        chain_last.int_state.counts[b_idx] > 0 && chain_last.int_state.counts[c_idx] > 0,
        "fixture is vacuous: B={}, C={} — both transition flow and event transfer \
         must be non-zero for the equality to mean anything",
        chain_last.int_state.counts[b_idx], chain_last.int_state.counts[c_idx],
    );
    assert!(
        chain_last.int_state.counts[a_idx] < 500,
        "fixture is vacuous: A did not drain (still {})",
        chain_last.int_state.counts[a_idx],
    );

    assert_eq!(
        chain_hash, tau_hash,
        "chain_binomial and tau_leap DIVERGED on the fused-event fixture at matched \
         dt=1 (chain 0x{chain_hash:016x}, tau 0x{tau_hash:016x}). They share the \
         identical kernel, so a divergence means tau read the event delta from the \
         POST-drain state instead of the start-of-step snapshot — the read-source \
         bug this cycle fixes."
    );
}
