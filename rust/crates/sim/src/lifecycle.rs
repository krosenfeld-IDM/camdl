//! The shared within-substep effect seam.
//!
//! Every fixed-step backend runs the same canonical within-substep lifecycle:
//!
//! ```text
//! PROPOSE event-deltas (from the START-OF-STEP SNAPSHOT)
//!     -> ADVANCE (kernel draws, fuse the event deltas)
//!     -> INTERVENE (on the post-advance state)
//!     -> BALANCE (last, chain-only)
//! ```
//!
//! The two functions here are the genuinely-shared parts of that lifecycle: the
//! PROPOSE stage (stage 1) and the post-ADVANCE INTERVENE+BALANCE tail (stages
//! 3–4). The ADVANCE kernel itself (transition draws) stays per-backend, because
//! the draw algorithms differ (Euler-multinomial vs independent Poisson vs RK4
//! vs SSA) — that is the seam: unify the effect bookkeeping, keep the kernels
//! distinct.
//!
//! These are deliberately thin, trait-shaped functions: each is documented as
//! the future `FixedStepLifecycle` trait method it will become once the
//! `{Int|Real}Delta` apply-seam lands and the snapshot/current state types are
//! generic over i64/f64.

use crate::{
    compiled_model::{CompiledModel, ResolvedBalance},
    error::SimError,
    intervention::{apply_interventions_at, inject_event_deltas},
    propensity::EvalCtx,
    resolved_expr::eval_resolved,
    state::{IntState, RealState},
};

/// → `FixedStepLifecycle::propose_event_deltas` once the {Int|Real}Delta apply-seam lands.
/// PROPOSE (stage 1): event deltas from the START-OF-STEP SNAPSHOT. Takes `&snapshot`,
/// never `&mut` — an event physically cannot read post-advance state. The kernel FUSES
/// the returned deltas into ADVANCE. RNG-free. i64-only this cycle (the f64 apply-seam is next).
///
/// Appends `(local_int_idx, delta)` entries onto `pending_deltas`; the caller
/// applies the fused (transition + event) deltas atomically. Wraps
/// [`inject_event_deltas`] — the single delta-producing definition.
#[allow(clippy::too_many_arguments)]
pub fn propose_event_deltas(
    model: &CompiledModel,
    fire_steps: &[std::collections::BTreeSet<i64>],
    snapshot: &IntState,
    real_snapshot: &RealState,
    params: &[f64],
    t: f64,
    dt: f64,
    pending_deltas: &mut Vec<(usize, i64)>,
) -> Result<(), SimError> {
    inject_event_deltas(
        model, fire_steps, snapshot, real_snapshot, params, t, dt, pending_deltas,
    )
}

/// → `FixedStepLifecycle::apply_post_advance`. Stages 3-4: INTERVENE then BALANCE on the
/// CURRENT post-advance state, in fixed order. One function so no backend can reorder them.
/// NOTE: for tau/ode/gillespie this is a one-call passthrough today (balance is chain-only).
///
/// INTERVENE fires every intervention whose `fire_steps` lands at `t + dt`
/// (within `tolerance`), reading the current post-advance state; BALANCE then
/// overwrites the target compartment so the population budget holds, reading the
/// post-intervention state. The balance target is exempt from the
/// negative-count check by construction (its negativity is a separate signal,
/// warned about here, not erred). RNG-free.
#[allow(clippy::too_many_arguments)]
pub fn apply_post_advance(
    model: &CompiledModel,
    fire_steps: &[std::collections::BTreeSet<i64>],
    current: &mut IntState,
    real: &mut RealState,
    params: &[f64],
    t: f64,
    dt: f64,
    tolerance: f64,
    balance: Option<&ResolvedBalance>,
) -> Result<(), SimError> {
    let t_end = t + dt;

    // Stage 3: INTERVENE on the current post-advance state.
    if !model.model.interventions.is_empty() {
        apply_interventions_at(
            t_end, model, fire_steps, dt, current, real, params, tolerance,
        )?;
    }

    // Stage 4: BALANCE — overwrite the target compartment so the population
    // budget holds. All other compartments are finalized at this point.
    if let Some(bal) = balance {
        let ctx = EvalCtx {
            model, int_s: current, real_s: real,
            params, t: t_end, dt, projected: None, int_float_override: None,
        };
        let val = eval_resolved(&bal.expr, &ctx);
        let bal_count = val.round() as i64;
        if bal_count < 0 {
            log::warn!("balance compartment went negative ({}) at t={:.1} — \
                        model may be inconsistent at these parameters", bal_count, t_end);
        }
        current.counts[bal.local_int_idx] = bal_count;
    }

    Ok(())
}
