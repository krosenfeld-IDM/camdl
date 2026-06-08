use crate::{
    compiled_model::CompiledModel,
    error::SimError,
    propensity::EvalCtx,
    resolved_expr::{eval_resolved, ResolvedExpr},
    state::{IntState, RealState},
};
use ir::intervention::{Action, InterventionSchedule};

/// Short human label for an action, for diagnostics (`"set V"`,
/// `"transfer S -> I (fraction)"`). Error-path only.
fn action_label(action: &Action) -> String {
    match action {
        Action::Add(a) => format!("add {}", a.compartment),
        Action::Set(a) => format!("set {}", a.compartment),
        Action::FractionTransfer(t) => format!("transfer {} -> {} (fraction)", t.src, t.dst),
        Action::AbsoluteTransfer(t) => format!("transfer {} -> {} (absolute)", t.src, t.dst),
    }
}

/// Validate that an intervention/event action's resolved value is finite
/// before it is cast to a count. A non-finite value otherwise casts
/// silently and wrongly — `NaN as i64 == 0`, `inf as i64 == i64::MAX`,
/// `-inf as i64 == i64::MIN` — corrupting the trajectory with no error.
/// The finite guard on the intervention *time* has no analogue for the
/// resolved *value*; this is it. Hard error (not per-particle-recoverable):
/// a non-finite effect amount is a structural/config defect in the action
/// expression, not a stochastic exploration artifact, so it surfaces
/// regardless of caller (forward-sim or inference).
pub(crate) fn finite_action_value(
    value: f64,
    iv_name: &str,
    action: &Action,
    t: f64,
) -> Result<f64, SimError> {
    if !value.is_finite() {
        return Err(SimError::Validation(format!(
            "intervention '{iv_name}' action ({}) resolved to a non-finite \
             value ({value}) at t={t:.3}; a non-finite effect amount would \
             cast silently to a wrong count (NaN→0, +inf→i64::MAX, \
             -inf→i64::MIN) — check the action expression",
            action_label(action)
        )));
    }
    Ok(value)
}

/// Convert an `InterventionSchedule` to a sorted list of fire times.
///
/// For parametric `at [...]` lists (gh#69, `AtTimesExpr`) the caller
/// supplies pre-resolved `ResolvedExpr`s for the entries — evaluated
/// here against the current `params` vector with the rest of `EvalCtx`
/// (state, time, dt) filled by scratch. Schedule-time expressions are
/// constrained at compile time to reference only parameters and
/// constants (see `CompiledModel::new` validation, gh#69), so the
/// scratch values are never consulted.
pub fn intervention_fire_times(
    sched: &InterventionSchedule,
    resolved_at_times: Option<&[ResolvedExpr]>,
    model: &CompiledModel,
    params: &[f64],
) -> Vec<f64> {
    match sched {
        InterventionSchedule::AtTimes(times) => times.clone(),
        InterventionSchedule::AtTimesExpr(_) => {
            let resolved = resolved_at_times
                .expect("AtTimesExpr schedule must be accompanied by resolved exprs");
            let n_int = model.int_local_to_global.len();
            let n_real = model.real_local_to_global.len();
            let scratch_int = IntState::new(n_int);
            let scratch_real = RealState::new(n_real);
            let ctx = EvalCtx {
                model,
                int_s: &scratch_int,
                real_s: &scratch_real,
                params,
                t: 0.0,
                dt: 0.0,
                projected: None,
                int_float_override: None,
            };
            resolved.iter().map(|e| eval_resolved(e, &ctx)).collect()
        }
        InterventionSchedule::Recurring(rs) => {
            let mut times = Vec::new();
            if let Some(at_day) = rs.at_day {
                // Fire at at_day + k*period, for smallest k where target >= start
                let k0 = ((rs.start - at_day) / rs.period).ceil().max(0.0) as u64;
                let mut t = at_day + k0 as f64 * rs.period;
                while t <= rs.end + rs.period * 1e-9 {
                    times.push(t);
                    t += rs.period;
                }
            } else {
                let mut t = rs.start;
                while t <= rs.end + rs.period * 1e-9 {
                    times.push(t);
                    t += rs.period;
                }
            }
            times
        }
    }
}

/// Apply all interventions scheduled at time `t` (in document order).
///
/// `dt` is `dt_actual` — the realized integrator substep (not
/// `model.simulation.dt`, which the compiled model carries only as a default —
/// the runtime can override it via `SimConfig.dt`); it drives the effect-amount
/// evaluation. `grid_dt` is the nominal model dt the `fire_steps` step-index
/// table was built on (`resolve_fire_steps(grid_dt, …)`), so the intervention
/// FIRING KEY is computed on `grid_dt`, not `dt`. They are equal under Snap and
/// for on-grid Exact substeps, diverging only when an inference filter clips a
/// substep to land on an off-grid observation — keying on `grid_dt` lands the
/// clipped substep on the correct nominal step. See gh#53 for the compile/runtime
/// split and docs/dev/proposals/2026-06-07-scheduling-spine-v2.md §A for the
/// two step lengths.
#[allow(clippy::too_many_arguments)]
pub fn apply_interventions_at(
    t: f64,
    model: &CompiledModel,
    fire_steps: &[std::collections::BTreeSet<i64>],
    dt: f64,
    grid_dt: f64,
    int_s: &mut IntState,
    real_s: &mut RealState,
    params: &[f64],
    _tolerance: f64,
) -> Result<bool, SimError> {
    // Rm4 in 2026-04-19 engine review: guard against NaN t silently
    // rounding to step 0. NaN `as i64` is 0 on current rustc, which
    // would make every intervention match step 0 if an upstream bug
    // ever produced NaN.
    if !t.is_finite() {
        return Err(SimError::Validation(format!(
            "apply_interventions_at: non-finite t = {}", t
        )));
    }
    let current_step = crate::time::time_to_step(t, grid_dt);
    let mut any_fired = false;
    for (iv_idx, iv) in model.model.interventions.iter().enumerate() {
        if iv.always_active { continue; }
        if fire_steps[iv_idx].contains(&current_step) {
            crate::effects::apply_intervention_effects(
                model, iv_idx, iv, int_s, real_s, params, t, dt,
            )?;
            any_fired = true;
        }
    }
    Ok(any_fired)
}

/// Apply always_active event actions directly to `int_s` / `real_s`.
///
/// gh#67: ode/tau_leap/gillespie do not have a `pending_deltas` pipeline
/// (only chain_binomial does, for atomic interleaving with multinomial
/// draws). They call this helper at each intervention boundary instead of
/// `inject_event_deltas`. `t_event` is the time the boundary was scheduled
/// for; `dt` is the same dt used to build `fire_steps` so the step lookup
/// matches.
pub fn apply_events_at(
    t_event: f64,
    model: &CompiledModel,
    fire_steps: &[std::collections::BTreeSet<i64>],
    dt: f64,
    int_s: &mut IntState,
    real_s: &mut RealState,
    params: &[f64],
) -> Result<bool, SimError> {
    if !t_event.is_finite() {
        return Err(SimError::Validation(format!(
            "apply_events_at: non-finite t = {}", t_event
        )));
    }
    // `resolve_events` uses `t_end = t + dt` for the EvalCtx and `t_end / grid_dt`
    // for the step-index lookup, so pass `t = t_event - dt` to land on
    // `t_end = t_event`. `dt` here is the nominal grid the `fire_steps` were built
    // on (gillespie's `iv_resolution_dt`); the realized substep coincides with it
    // on this at-boundary event path, so it is both `dt_actual` and `grid_dt`.
    let mut ev = crate::effects::EffectDeltas::default();
    crate::effects::resolve_events(
        model, fire_steps, int_s, real_s, params, t_event - dt, dt, dt, &mut ev,
    )?;
    let fired = !ev.is_empty();
    for d in &ev.int {
        int_s.counts[d.idx] += d.delta;
    }
    for d in &ev.real {
        real_s.values[d.idx] += d.delta;
    }
    Ok(fired)
}

/// Collect sorted, deduplicated intervention times.
///
/// gh#69: takes `params` so any `AtTimesExpr` schedules can be resolved
/// against the current parameter vector. Parametric schedules' resolved
/// expressions live on `CompiledModel.resolved.intervention_at_time_exprs`.
pub fn all_intervention_times(model: &CompiledModel, params: &[f64]) -> Vec<f64> {
    let mut times: Vec<f64> = model.model.interventions.iter()
        .enumerate()
        .flat_map(|(iv_idx, iv)| {
            let resolved = model.resolved.intervention_at_time_exprs[iv_idx].as_deref();
            intervention_fire_times(&iv.schedule, resolved, model, params)
        })
        .collect();
    times.sort_by(|a, b| a.total_cmp(b));
    times.dedup();
    times
}

#[cfg(test)]
mod tests {
    use super::*;
    use ir::intervention::SetAction;
    use ir::expr::Expr;

    fn set_v() -> Action {
        Action::Set(SetAction { compartment: "V".into(), value: Expr::const_(0.0) })
    }

    /// Every non-finite kind must error before the cast. `NaN as i64 == 0`,
    /// `+inf as i64 == i64::MAX`, `-inf as i64 == i64::MIN` — all silent
    /// corruption if they reach the cast. The guard rejects all three.
    #[test]
    fn finite_action_value_rejects_non_finite() {
        let action = set_v();
        for bad in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
            let err = finite_action_value(bad, "campaign", &action, 5.0)
                .expect_err(&format!("{bad} must be rejected"));
            let msg = err.to_string();
            assert!(msg.contains("non-finite"), "message should name the cause: {msg}");
            assert!(msg.contains("campaign"), "message should name the intervention: {msg}");
            assert!(msg.contains("set V"), "message should label the action: {msg}");
        }
    }

    /// A finite value passes through unchanged (negative is finite — the
    /// negative-count check is a *separate* guard, applied post-state, not
    /// here).
    #[test]
    fn finite_action_value_passes_finite() {
        let action = set_v();
        for ok in [0.0, 1.0, -3.0, 1e9, f64::MIN_POSITIVE] {
            assert_eq!(
                finite_action_value(ok, "campaign", &action, 0.0).unwrap(),
                ok
            );
        }
    }
}
