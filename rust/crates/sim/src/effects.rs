//! Pure effect resolution + trivial application.
//!
//! The within-substep effect system has two orthogonal axes: **representation**
//! (integer `i64` vs continuous `f64` compartments) and **purity** (resolving an
//! effect — reading a snapshot, computing a delta, no side effect — vs applying
//! it). This module separates both:
//!
//!   - [`resolve_intervention`] is PURE: it reads an immutable [`StateRef`] and
//!     emits typed [`IntDelta`]/[`RealDelta`] entries into an [`EffectDeltas`].
//!     All the bug-prone arithmetic — rounding mode, clamps, snapshot
//!     subtraction, arena dispatch — lives here, once, testable as plain data.
//!   - [`apply_effects`] is TRIVIAL: it writes the deltas into a [`StateMut`].
//!     No arithmetic, no branch on representation, so it cannot carry a bug.
//!
//! Representation collapses into the delta *type* (no runtime `match` at apply
//! time); purity collapses into the `Ref`/`Mut` *types* (a resolver cannot
//! mutate). The per-action rules reproduce the historical behaviour exactly:
//! `round` for add/set/absolute-transfer, `floor` for fraction-transfer, the
//! `frac ∈ [0,1]` and `.min(src)` clamps; the real arena applies exact `f64`.
//!
//! Two historical asymmetries are unified here (both byte-identical on every
//! current model, since no fixture exercises either): events targeting a real
//! compartment now apply instead of being dropped, and a negative `add`
//! resolves to a hard error on every path, not just the intervention path.

use crate::{
    compiled_model::CompiledModel,
    error::{NegativeCountCause, SimError},
    propensity::EvalCtx,
    resolved_expr::eval_resolved,
    state::{IntState, RealState},
};
use ir::intervention::{Action, Intervention};

/// Immutable read view over the two compartment arenas. A resolver takes this
/// and *cannot* mutate state — the purity half of the seam, type-enforced.
#[derive(Clone, Copy)]
pub struct StateRef<'a> {
    pub int: &'a IntState,
    pub real: &'a RealState,
}

/// Mutable apply target — the other half of the purity seam.
pub struct StateMut<'a> {
    pub int: &'a mut IntState,
    pub real: &'a mut RealState,
}

/// A change to one integer compartment (local int index).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct IntDelta {
    pub idx: usize,
    pub delta: i64,
}

/// A change to one real compartment (local real index).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RealDelta {
    pub idx: usize,
    pub delta: f64,
}

/// The typed output of resolution: deltas for each arena. Representation is
/// carried by the entry type, so application needs no runtime branch.
#[derive(Default, Debug, Clone, PartialEq)]
pub struct EffectDeltas {
    pub int: Vec<IntDelta>,
    pub real: Vec<RealDelta>,
}

impl EffectDeltas {
    pub fn is_empty(&self) -> bool {
        self.int.is_empty() && self.real.is_empty()
    }
    pub fn clear(&mut self) {
        self.int.clear();
        self.real.clear();
    }
}

/// Apply resolved deltas in order. Trivial — no arithmetic, no representation
/// branch; a delta either lands in `int` or `real` by its own type.
pub fn apply_effects(d: &EffectDeltas, s: StateMut<'_>) {
    for IntDelta { idx, delta } in &d.int {
        s.int.counts[*idx] += *delta;
    }
    for RealDelta { idx, delta } in &d.real {
        s.real.values[*idx] += *delta;
    }
}

/// Whether the actions came from a scheduled intervention (post-advance,
/// applied in place) or an always-active event (pre-advance snapshot, fused
/// with the draw). Used only for the `CAMDL_TRACE_STEPS` label today.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum EffectKind {
    Intervention,
    Event,
}

impl EffectKind {
    fn label(self) -> &'static str {
        match self {
            EffectKind::Intervention => "INTERVENTION",
            EffectKind::Event => "EVENT",
        }
    }
}

/// `CAMDL_TRACE_STEPS` observability for one action. Stderr-only, env-gated, no
/// effect on results — kept out of the pure resolver and emitted at the wiring.
fn trace_action(kind: EffectKind, iv_name: &str, action: &Action, v: f64, t: f64) {
    if !crate::chain_binomial::trace_enabled() {
        return;
    }
    let k = kind.label();
    match action {
        Action::Add(a) => eprintln!(
            "{k} '{iv_name}' at t={t:.1}: add {} += {} (raw={v:.2})",
            a.compartment, v.round() as i64
        ),
        Action::Set(a) => {
            eprintln!("{k} '{iv_name}' at t={t:.1}: set {} = {v:.2}", a.compartment)
        }
        Action::FractionTransfer(ft) => eprintln!(
            "{k} '{iv_name}' at t={t:.1}: transfer {} -> {} (frac={:.2})",
            ft.src, ft.dst, v.clamp(0.0, 1.0)
        ),
        Action::AbsoluteTransfer(at) => eprintln!(
            "{k} '{iv_name}' at t={t:.1}: transfer {} -> {} (raw={v:.2})",
            at.src, at.dst
        ),
    }
}

/// The scheduled-intervention path: resolve + apply each action **sequentially**
/// against the live post-advance state, so action `i+1` sees action `i`'s effect
/// (the historical `apply_intervention` semantics — distinct from the event
/// path, which resolves every action against one frozen pre-advance snapshot).
/// Byte-identical to the prior in-place apply.
#[allow(clippy::too_many_arguments)]
pub fn apply_intervention_effects(
    model: &CompiledModel,
    iv_idx: usize,
    iv: &Intervention,
    int_s: &mut IntState,
    real_s: &mut RealState,
    params: &[f64],
    t: f64,
    dt: f64,
) -> Result<(), SimError> {
    let mut out = EffectDeltas::default();
    for (action_idx, action) in iv.actions.iter().enumerate() {
        out.clear();
        resolve_one(
            model, iv_idx, action_idx, &iv.name, action,
            StateRef { int: int_s, real: real_s }, params, t, dt,
            EffectKind::Intervention, &mut out,
        )?;
        apply_effects(&out, StateMut { int: int_s, real: real_s });
    }
    Ok(())
}

/// Which arena a compartment lives in, plus its local index.
enum Arena {
    Int(usize),
    Real(usize),
}

/// Resolve a compartment name to its arena + local index (the same dispatch the
/// rate evaluator uses: `comp_index → global → global_to_int else global_to_real`).
fn resolve_target(model: &CompiledModel, name: &str) -> Result<Arena, SimError> {
    let g = *model
        .comp_index
        .get(name)
        .ok_or_else(|| SimError::UnknownCompartment(name.to_string()))?;
    if let Some(i) = model.global_to_int[g] {
        Ok(Arena::Int(i))
    } else if let Some(i) = model.global_to_real[g] {
        Ok(Arena::Real(i))
    } else {
        Err(SimError::UnknownCompartment(name.to_string()))
    }
}

/// Resolve one action against `snap`: evaluate its amount expression, finite-
/// check it, trace it, and append the typed delta(s) to `out`. The single
/// per-action path shared by the intervention (sequential) and event (parallel)
/// resolvers. PURE w.r.t. state — no mutation, no RNG.
#[allow(clippy::too_many_arguments)]
fn resolve_one(
    model: &CompiledModel,
    iv_idx: usize,
    action_idx: usize,
    iv_name: &str,
    action: &Action,
    snap: StateRef<'_>,
    params: &[f64],
    t: f64,
    dt: f64,
    kind: EffectKind,
    out: &mut EffectDeltas,
) -> Result<(), SimError> {
    let ctx = EvalCtx {
        model, int_s: snap.int, real_s: snap.real, params, t, dt,
        projected: None, int_float_override: None,
    };
    let v = eval_resolved(&model.resolved.intervention_exprs[iv_idx][action_idx], &ctx);
    let v = crate::intervention::finite_action_value(v, iv_name, action, t)?;
    trace_action(kind, iv_name, action, v, t);
    resolve_action(model, action, v, snap, t, out)
}

/// Resolve every action of one intervention/event against the SAME `snap`,
/// appending the typed deltas to `out` (the parallel idiom — every action sees
/// the same frozen snapshot, used by the event path). PURE.
#[allow(clippy::too_many_arguments)]
pub fn resolve_intervention(
    model: &CompiledModel,
    iv_idx: usize,
    iv: &Intervention,
    snap: StateRef<'_>,
    params: &[f64],
    t: f64,
    dt: f64,
    kind: EffectKind,
    out: &mut EffectDeltas,
) -> Result<(), SimError> {
    for (action_idx, action) in iv.actions.iter().enumerate() {
        resolve_one(model, iv_idx, action_idx, &iv.name, action, snap, params, t, dt, kind, out)?;
    }
    Ok(())
}

/// Resolve all always-active events firing at this step into typed deltas. The
/// EVENT path: every action of every firing event resolves against the frozen
/// pre-advance snapshot at `t_end = t + dt` (so events fuse with the kernel
/// draw). PURE — the caller fuses `out.int` into the draw and applies `out.real`
/// to the real reservoir. Replaces the historical int-only `inject_event_deltas`
/// (which silently dropped real-targeted events).
pub fn resolve_events(
    model: &CompiledModel,
    fire_steps: &[std::collections::BTreeSet<i64>],
    snapshot: &IntState,
    real_snapshot: &RealState,
    params: &[f64],
    t: f64,
    dt: f64,
    out: &mut EffectDeltas,
) -> Result<(), SimError> {
    let t_end = t + dt;
    let current_step = crate::time::time_to_step(t_end, dt);
    let snap = StateRef { int: snapshot, real: real_snapshot };
    for (iv_idx, iv) in model.model.interventions.iter().enumerate() {
        if !iv.always_active {
            continue;
        }
        if !fire_steps[iv_idx].contains(&current_step) {
            continue;
        }
        resolve_intervention(model, iv_idx, iv, snap, params, t_end, dt, EffectKind::Event, out)?;
    }
    Ok(())
}

// ── Continuous (ODE) effect application ─────────────────────────────────────
//
// ODE holds its INTEGER compartments as f64 (`int_vals`) and integrates them
// with RK4. The discrete resolver above rounds the integer arena to i64; routing
// ODE through it would discard the fractional state the integrator accumulated
// at every effect boundary (the historical `to_states` round-trip). The
// continuous path below applies effects to the f64 vectors EXACTLY — same action
// structure, but no rounding and no `.floor()` — reading integer compartments as
// f64 via `int_float_override` (the same mechanism `eval_propensities` uses for
// ODE rates). The discrete backends keep the i64 resolver untouched.

/// Evaluate one action's amount expression against f64 compartment values (the
/// ODE read path: integer compartments via `int_float_override`).
fn eval_amount_f64(
    model: &CompiledModel,
    iv_idx: usize,
    action_idx: usize,
    int_f64: &[f64],
    real_f64: &[f64],
    params: &[f64],
    t: f64,
    dt: f64,
) -> f64 {
    let placeholder = IntState::new(model.int_local_to_global.len());
    let rs = RealState::from_vec(real_f64.to_vec());
    let ctx = EvalCtx {
        model, int_s: &placeholder, real_s: &rs, params, t, dt,
        projected: None, int_float_override: Some(int_f64),
    };
    eval_resolved(&model.resolved.intervention_exprs[iv_idx][action_idx], &ctx)
}

/// Apply one action exactly in f64. `read_int`/`read_real` supply the values the
/// snapshot-relative arithmetic reads (the frozen snapshot for events; the live
/// state for sequential interventions); the result is written to
/// `int_vals`/`real_vals`.
#[allow(clippy::too_many_arguments)]
fn apply_action_f64(
    model: &CompiledModel,
    action: &Action,
    v: f64,
    read_int: &[f64],
    read_real: &[f64],
    int_vals: &mut [f64],
    real_vals: &mut [f64],
) -> Result<(), SimError> {
    match action {
        Action::Add(aa) => {
            if v.round() < 0.0 {
                return Err(SimError::NegativeCount {
                    compartment: aa.compartment.clone(),
                    attempted_value: v.round() as i64,
                    t: 0.0,
                    cause: NegativeCountCause::InterventionAddNegative,
                });
            }
            match resolve_target(model, &aa.compartment)? {
                Arena::Int(i) => int_vals[i] += v,
                Arena::Real(i) => real_vals[i] += v,
            }
        }
        Action::Set(sa) => match resolve_target(model, &sa.compartment)? {
            Arena::Int(i) => int_vals[i] = v,
            Arena::Real(i) => real_vals[i] = v,
        },
        Action::FractionTransfer(ft) => {
            let frac = v.clamp(0.0, 1.0);
            match (resolve_target(model, &ft.src)?, resolve_target(model, &ft.dst)?) {
                (Arena::Int(s), Arena::Int(d)) => {
                    let x = read_int[s] * frac;
                    int_vals[s] -= x;
                    int_vals[d] += x;
                }
                (Arena::Real(s), Arena::Real(d)) => {
                    let x = read_real[s] * frac;
                    real_vals[s] -= x;
                    real_vals[d] += x;
                }
                _ => return Err(mixed_arena_err(&ft.src, &ft.dst)),
            }
        }
        Action::AbsoluteTransfer(at) => {
            match (resolve_target(model, &at.src)?, resolve_target(model, &at.dst)?) {
                (Arena::Int(s), Arena::Int(d)) => {
                    let x = v.min(read_int[s]);
                    int_vals[s] -= x;
                    int_vals[d] += x;
                }
                (Arena::Real(s), Arena::Real(d)) => {
                    let x = v.min(read_real[s]);
                    real_vals[s] -= x;
                    real_vals[d] += x;
                }
                _ => return Err(mixed_arena_err(&at.src, &at.dst)),
            }
        }
    }
    Ok(())
}

/// Apply all boundary effects to ODE's continuous compartment vectors, exactly
/// (no rounding). Order matches the discrete lifecycle: always-active EVENTS
/// fire first, every action reading the frozen pre-intervention snapshot; then
/// scheduled INTERVENTIONS apply sequentially on the post-event state. ODE
/// carries no balance. `t_boundary` is the effect time; `dt` is the step that
/// built `fire_steps`.
pub fn apply_boundary_effects_continuous(
    model: &CompiledModel,
    fire_steps: &[std::collections::BTreeSet<i64>],
    int_vals: &mut [f64],
    real_vals: &mut [f64],
    params: &[f64],
    t_boundary: f64,
    dt: f64,
) -> Result<(), SimError> {
    let current_step = crate::time::time_to_step(t_boundary, dt);
    let t = t_boundary;

    // EVENTS — frozen snapshot, every firing event's actions resolve against it.
    if model.model.interventions.iter().any(|iv| iv.always_active) {
        let snap_int = int_vals.to_vec();
        let snap_real = real_vals.to_vec();
        for (iv_idx, iv) in model.model.interventions.iter().enumerate() {
            if !iv.always_active || !fire_steps[iv_idx].contains(&current_step) {
                continue;
            }
            for (action_idx, action) in iv.actions.iter().enumerate() {
                let v = eval_amount_f64(model, iv_idx, action_idx, &snap_int, &snap_real, params, t, dt);
                let v = crate::intervention::finite_action_value(v, &iv.name, action, t)?;
                trace_action(EffectKind::Event, &iv.name, action, v, t);
                apply_action_f64(model, action, v, &snap_int, &snap_real, int_vals, real_vals)?;
            }
        }
    }

    // INTERVENTIONS — sequential, each action reads the live state.
    for (iv_idx, iv) in model.model.interventions.iter().enumerate() {
        if iv.always_active || !fire_steps[iv_idx].contains(&current_step) {
            continue;
        }
        for (action_idx, action) in iv.actions.iter().enumerate() {
            let live_int = int_vals.to_vec();
            let live_real = real_vals.to_vec();
            let v = eval_amount_f64(model, iv_idx, action_idx, &live_int, &live_real, params, t, dt);
            let v = crate::intervention::finite_action_value(v, &iv.name, action, t)?;
            trace_action(EffectKind::Intervention, &iv.name, action, v, t);
            apply_action_f64(model, action, v, &live_int, &live_real, int_vals, real_vals)?;
        }
    }
    Ok(())
}

/// The pure arithmetic core: one action + its resolved `f64` value + the
/// snapshot → typed deltas. No model state is read except the snapshot and the
/// arena map. Mirrors the historical `apply_intervention` / `inject_event_deltas`
/// rounding exactly for the integer arena.
fn resolve_action(
    model: &CompiledModel,
    action: &Action,
    v: f64,
    snap: StateRef<'_>,
    t: f64,
    out: &mut EffectDeltas,
) -> Result<(), SimError> {
    match action {
        Action::Add(aa) => {
            let count = v.round() as i64;
            // A negative add is always a config bug (you cannot add a negative
            // number of individuals) — hard error on every path.
            if count < 0 {
                return Err(SimError::NegativeCount {
                    compartment: aa.compartment.clone(),
                    attempted_value: count,
                    t,
                    cause: NegativeCountCause::InterventionAddNegative,
                });
            }
            match resolve_target(model, &aa.compartment)? {
                Arena::Int(i) => out.int.push(IntDelta { idx: i, delta: count }),
                Arena::Real(i) => out.real.push(RealDelta { idx: i, delta: v }),
            }
        }
        Action::Set(sa) => match resolve_target(model, &sa.compartment)? {
            Arena::Int(i) => out.int.push(IntDelta {
                idx: i,
                delta: (v.round() as i64) - snap.int.counts[i],
            }),
            Arena::Real(i) => out.real.push(RealDelta {
                idx: i,
                delta: v - snap.real.values[i],
            }),
        },
        Action::FractionTransfer(ft) => {
            let frac = v.clamp(0.0, 1.0);
            match (resolve_target(model, &ft.src)?, resolve_target(model, &ft.dst)?) {
                (Arena::Int(s), Arena::Int(d)) => {
                    let x = ((snap.int.counts[s] as f64) * frac).floor() as i64;
                    out.int.push(IntDelta { idx: s, delta: -x });
                    out.int.push(IntDelta { idx: d, delta: x });
                }
                (Arena::Real(s), Arena::Real(d)) => {
                    let x = snap.real.values[s] * frac;
                    out.real.push(RealDelta { idx: s, delta: -x });
                    out.real.push(RealDelta { idx: d, delta: x });
                }
                _ => return Err(mixed_arena_err(&ft.src, &ft.dst)),
            }
        }
        Action::AbsoluteTransfer(at) => {
            match (resolve_target(model, &at.src)?, resolve_target(model, &at.dst)?) {
                (Arena::Int(s), Arena::Int(d)) => {
                    let x = (v.round() as i64).min(snap.int.counts[s]);
                    out.int.push(IntDelta { idx: s, delta: -x });
                    out.int.push(IntDelta { idx: d, delta: x });
                }
                (Arena::Real(s), Arena::Real(d)) => {
                    let x = v.min(snap.real.values[s]);
                    out.real.push(RealDelta { idx: s, delta: -x });
                    out.real.push(RealDelta { idx: d, delta: x });
                }
                _ => return Err(mixed_arena_err(&at.src, &at.dst)),
            }
        }
    }
    Ok(())
}

/// A transfer whose endpoints land in different arenas (one integer, one real)
/// is not representable — error instead of the historical silent no-op.
fn mixed_arena_err(src: &str, dst: &str) -> SimError {
    SimError::Validation(format!(
        "transfer '{src}' -> '{dst}': source and destination must be the same \
         representation (both integer or both real compartments)"
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::compiled_model::CompiledModel;
    use ir::{
        expr::Expr,
        intervention::{
            AbsoluteTransfer, AddAction, FractionTransfer, Intervention, InterventionSchedule,
            SetAction,
        },
        model::{
            Compartment, CompartmentKind, InitialConditions, OutputConfig, OutputSchedule,
            SimulationConfig,
        },
        parameter::Parameter,
        transition::{DrawMethod, StoichiometryEntry, Transition},
        Model,
    };
    use std::collections::HashMap;

    // S, I integer; W real. One trivial transition so the model compiles.
    fn model_with(actions: Vec<Action>) -> CompiledModel {
        let m = Model {
            name: "effects_test".into(),
            version: "0.1".into(),
            time_unit: "days".into(),
            description: None,
            origin: None,
            origin_rata_die: None,
            compartments: vec![
                Compartment { name: "S".into(), kind: CompartmentKind::Integer },
                Compartment { name: "I".into(), kind: CompartmentKind::Integer },
                Compartment { name: "W".into(), kind: CompartmentKind::Real },
            ],
            transitions: vec![Transition {
                name: "decay".into(),
                stoichiometry: vec![StoichiometryEntry("S".into(), -1), StoichiometryEntry("I".into(), 1)],
                rate: Expr::const_(0.0),
                metadata: None,
                draw_method: DrawMethod::Poisson,
                rate_grad: Default::default(),
                lineage: None,
            }],
            ode_equations: vec![],
            time_functions: vec![],
            tables: vec![],
            interventions: vec![Intervention {
                name: "iv".into(),
                base_name: None,
                schedule: InterventionSchedule::AtTimes(vec![1.0]),
                actions,
                always_active: false,
            }],
            observations: vec![],
            bindings: vec![],
            parameters: vec![Parameter {
                name: "p".into(), value: Some(1.0), bounds: None, prior: None,
                transform: None, initial_value: None, param_kind: None,
                param_dim: None, hierarchical: None,
            }],
            initial_conditions: InitialConditions::Explicit({
                let mut h = HashMap::new();
                h.insert("S".into(), 100.0);
                h.insert("I".into(), 0.0);
                h.insert("W".into(), 50.0);
                h
            }),
            output: OutputConfig {
                times: OutputSchedule::AtTimes(vec![0.0, 1.0]),
                format: "tsv".into(),
                trajectory: true,
                observations: false,
            },
            simulation: SimulationConfig {
                t_start: 0.0, t_end: 1.0, time_semantics: "continuous".into(),
                dt: Some(1.0), rng_seed: Some(1),
            },
            presets: vec![],
            model_structure: None,
            balance: None,
            identity_tracked_compartments: vec![],
        };
        CompiledModel::new(m).unwrap()
    }

    // S=100 (local int 0), I=0 (local int 1), W=50.0 (local real 0).
    fn snap<'a>(int_s: &'a IntState, real_s: &'a RealState) -> StateRef<'a> {
        StateRef { int: int_s, real: real_s }
    }

    fn states() -> (IntState, RealState) {
        let int_s = IntState::from_vec(vec![100, 0]);
        let real_s = RealState::from_vec(vec![50.0]);
        (int_s, real_s)
    }

    fn resolve(model: &CompiledModel) -> EffectDeltas {
        let (int_s, real_s) = states();
        let mut out = EffectDeltas::default();
        resolve_intervention(model, 0, &model.model.interventions[0], snap(&int_s, &real_s),
                             &model.default_params, 1.0, 1.0, EffectKind::Intervention, &mut out).unwrap();
        out
    }

    #[test]
    fn add_int_rounds_and_emits_positive_delta() {
        let m = model_with(vec![Action::Add(AddAction { compartment: "I".into(), count: Expr::const_(3.6) })]);
        let d = resolve(&m);
        assert_eq!(d.int, vec![IntDelta { idx: 1, delta: 4 }]); // round(3.6)=4 to I(local 1)
        assert!(d.real.is_empty());
    }

    #[test]
    fn add_real_is_exact_f64() {
        let m = model_with(vec![Action::Add(AddAction { compartment: "W".into(), count: Expr::const_(2.5) })]);
        let d = resolve(&m);
        assert_eq!(d.real, vec![RealDelta { idx: 0, delta: 2.5 }]); // exact, no round
        assert!(d.int.is_empty());
    }

    #[test]
    fn add_negative_is_hard_error_on_any_path() {
        let m = model_with(vec![Action::Add(AddAction { compartment: "I".into(), count: Expr::const_(-1.0) })]);
        let (int_s, real_s) = states();
        let mut out = EffectDeltas::default();
        let err = resolve_intervention(&m, 0, &m.model.interventions[0], snap(&int_s, &real_s),
                                       &m.default_params, 1.0, 1.0, EffectKind::Intervention, &mut out).unwrap_err();
        assert!(matches!(err, SimError::NegativeCount { cause: NegativeCountCause::InterventionAddNegative, .. }));
    }

    #[test]
    fn set_int_emits_snapshot_relative_delta() {
        let m = model_with(vec![Action::Set(SetAction { compartment: "S".into(), value: Expr::const_(70.4) })]);
        let d = resolve(&m);
        // round(70.4)=70, snapshot S=100 → delta -30 → S ends at 70.
        assert_eq!(d.int, vec![IntDelta { idx: 0, delta: 70 - 100 }]);
    }

    #[test]
    fn set_real_is_exact() {
        let m = model_with(vec![Action::Set(SetAction { compartment: "W".into(), value: Expr::const_(12.5) })]);
        let d = resolve(&m);
        assert_eq!(d.real, vec![RealDelta { idx: 0, delta: 12.5 - 50.0 }]);
    }

    #[test]
    fn fraction_transfer_int_floors() {
        // 0.337 * 100 = 33.7 → floor 33.
        let m = model_with(vec![Action::FractionTransfer(FractionTransfer {
            src: "S".into(), dst: "I".into(), fraction: Expr::const_(0.337),
        })]);
        let d = resolve(&m);
        assert_eq!(d.int, vec![IntDelta { idx: 0, delta: -33 }, IntDelta { idx: 1, delta: 33 }]);
    }

    #[test]
    fn absolute_transfer_int_rounds_then_clamps_to_src() {
        // round(250.6)=251, clamped to src S=100.
        let m = model_with(vec![Action::AbsoluteTransfer(AbsoluteTransfer {
            src: "S".into(), dst: "I".into(), count: Expr::const_(250.6),
        })]);
        let d = resolve(&m);
        assert_eq!(d.int, vec![IntDelta { idx: 0, delta: -100 }, IntDelta { idx: 1, delta: 100 }]);
    }

    #[test]
    fn mixed_arena_transfer_errors() {
        let m = model_with(vec![Action::FractionTransfer(FractionTransfer {
            src: "S".into(), dst: "W".into(), fraction: Expr::const_(0.5),
        })]);
        let (int_s, real_s) = states();
        let mut out = EffectDeltas::default();
        let err = resolve_intervention(&m, 0, &m.model.interventions[0], snap(&int_s, &real_s),
                                       &m.default_params, 1.0, 1.0, EffectKind::Intervention, &mut out).unwrap_err();
        assert!(matches!(err, SimError::Validation(_)));
    }

    #[test]
    fn apply_effects_sums_in_order() {
        let mut int_s = IntState::from_vec(vec![100, 0]);
        let mut real_s = RealState::from_vec(vec![50.0]);
        let d = EffectDeltas {
            int: vec![IntDelta { idx: 0, delta: -30 }, IntDelta { idx: 1, delta: 30 }],
            real: vec![RealDelta { idx: 0, delta: 2.5 }],
        };
        apply_effects(&d, StateMut { int: &mut int_s, real: &mut real_s });
        assert_eq!(int_s.counts, vec![70, 30]);
        assert_eq!(real_s.values, vec![52.5]);
    }

    // ── Continuous (ODE) path — exact f64, no rounding ──────────────────────
    // These are the de-quantization oracle at the f64 level (before the output
    // contract rounds integer compartments for display): the continuous apply
    // must carry the fractional integrator state exactly.

    /// A fraction-transfer on a FRACTIONAL integer compartment moves the exact
    /// fraction — no `.floor()`, unlike the discrete path. S = 704.69, transfer
    /// 0.5 → both ends = 352.345.
    #[test]
    fn continuous_fraction_transfer_is_exact() {
        let m = model_with(vec![Action::FractionTransfer(FractionTransfer {
            src: "S".into(), dst: "I".into(), fraction: Expr::const_(0.5),
        })]);
        let fire = m.resolve_fire_steps(1.0, &m.default_params);
        let mut int_vals = vec![704.69_f64, 0.0]; // S (local int 0), I (local int 1)
        let mut real_vals = vec![50.0_f64];        // W
        apply_boundary_effects_continuous(&m, &fire, &mut int_vals, &mut real_vals, &m.default_params, 1.0, 1.0).unwrap();
        assert_eq!(int_vals[0], 704.69 - 352.345);
        assert_eq!(int_vals[1], 352.345);
        // Discrete would floor(704*0.5)=352 after rounding S to 704 — different.
    }

    /// A `set` on an integer compartment lands the exact f64 (no round).
    #[test]
    fn continuous_set_int_is_exact() {
        let m = model_with(vec![Action::Set(SetAction {
            compartment: "S".into(), value: Expr::const_(70.4),
        })]);
        let fire = m.resolve_fire_steps(1.0, &m.default_params);
        let mut int_vals = vec![100.0_f64, 0.0];
        let mut real_vals = vec![50.0_f64];
        apply_boundary_effects_continuous(&m, &fire, &mut int_vals, &mut real_vals, &m.default_params, 1.0, 1.0).unwrap();
        assert_eq!(int_vals[0], 70.4); // exact, not round(70.4)=70
    }
}
