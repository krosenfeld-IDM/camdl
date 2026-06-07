use crate::{
    compiled_model::CompiledModel,
    error::SimError,
    propensity::EvalCtx,
    resolved_expr::{eval_resolved, ResolvedExpr},
    state::{IntState, RealState},
};
use ir::intervention::{Action, Intervention, InterventionSchedule};

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
fn finite_action_value(
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
/// `dt` is the **runtime** integrator step (not `model.simulation.dt`,
/// which the compiled model carries only as a default — the runtime
/// can override it via `SimConfig.dt`). `fire_steps` is the runtime-
/// resolved view of `model.fire_times` for that dt; callers obtain it
/// once per sim run via `model.resolve_fire_steps(dt)` and pass it
/// in. See gh#53 for why the compile/runtime split is load-bearing.
pub fn apply_interventions_at(
    t: f64,
    model: &CompiledModel,
    fire_steps: &[std::collections::BTreeSet<i64>],
    dt: f64,
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
    let current_step = crate::time::time_to_step(t, dt);
    let mut any_fired = false;
    for (iv_idx, iv) in model.model.interventions.iter().enumerate() {
        if iv.always_active { continue; }
        if fire_steps[iv_idx].contains(&current_step) {
            apply_intervention(iv, iv_idx, model, int_s, real_s, params, t, dt)?;
            any_fired = true;
        }
    }
    Ok(any_fired)
}

/// Inject always_active event actions as deltas into `pending_deltas`.
///
/// All action types are expressed as deltas from the snapshot state:
///   Add(n)        → (+n, target)
///   Transfer(f)   → (-delta, src), (+delta, dst) where delta = floor(src * f)
///   Set(v)        → (v - old, target) where old is from snapshot
///
/// Called from both `step_one` and `run_chain_binomial` to ensure events
/// are applied atomically with transitions, matching pomp's ordering.
pub fn inject_event_deltas(
    model: &CompiledModel,
    fire_steps: &[std::collections::BTreeSet<i64>],
    snapshot: &IntState,
    real_s: &RealState,
    params: &[f64],
    t: f64,
    dt: f64,
    pending_deltas: &mut Vec<(usize, i64)>,
) -> Result<(), SimError> {
    let t_end = t + dt;
    let ctx = EvalCtx {
        model, int_s: snapshot, real_s, params, t: t_end, dt, projected: None, int_float_override: None,
    };
    let current_step = crate::time::time_to_step(t_end, dt);
    for (iv_idx, iv) in model.model.interventions.iter().enumerate() {
        if !iv.always_active { continue; }
        if !fire_steps[iv_idx].contains(&current_step) { continue; }
        for (action_idx, action) in iv.actions.iter().enumerate() {
            let resolved_val = eval_resolved(&model.resolved.intervention_exprs[iv_idx][action_idx], &ctx);
            let resolved_val = finite_action_value(resolved_val, &iv.name, action, t_end)?;
            match action {
                Action::Add(aa) => {
                    let raw = resolved_val;
                    let n = raw.round() as i64;
                    if crate::chain_binomial::trace_enabled() {
                        eprintln!("EVENT '{}' at t={:.1}: add {} += {} (raw={:.2})",
                            iv.name, t_end, aa.compartment, n, raw);
                    }
                    if let Some(&global) = model.comp_index.get(aa.compartment.as_str()) {
                        if let Some(local) = model.global_to_int[global] {
                            pending_deltas.push((local, n));
                        }
                    }
                }
                Action::FractionTransfer(ft) => {
                    let frac = resolved_val.clamp(0.0, 1.0);
                    if let (Some(&sg), Some(&dg)) = (
                        model.comp_index.get(ft.src.as_str()),
                        model.comp_index.get(ft.dst.as_str()),
                    ) {
                        if let (Some(sl), Some(dl)) = (model.global_to_int[sg], model.global_to_int[dg]) {
                            let delta = (snapshot.counts[sl] as f64 * frac).floor() as i64;
                            if crate::chain_binomial::trace_enabled() {
                                eprintln!("EVENT '{}' at t={:.1}: transfer {} -> {} of {} (frac={:.2})",
                                    iv.name, t_end, ft.src, ft.dst, delta, frac);
                            }
                            pending_deltas.push((sl, -delta));
                            pending_deltas.push((dl, delta));
                        }
                    }
                }
                Action::AbsoluteTransfer(at) => {
                    let n = resolved_val.round() as i64;
                    if let (Some(&sg), Some(&dg)) = (
                        model.comp_index.get(at.src.as_str()),
                        model.comp_index.get(at.dst.as_str()),
                    ) {
                        if let (Some(sl), Some(dl)) = (model.global_to_int[sg], model.global_to_int[dg]) {
                            let transfer = n.min(snapshot.counts[sl]);
                            if crate::chain_binomial::trace_enabled() {
                                eprintln!("EVENT '{}' at t={:.1}: transfer {} -> {} of {} (raw={})",
                                    iv.name, t_end, at.src, at.dst, transfer, n);
                            }
                            pending_deltas.push((sl, -transfer));
                            pending_deltas.push((dl, transfer));
                        }
                    }
                }
                Action::Set(sa) => {
                    let new_val = resolved_val.round() as i64;
                    if let Some(&global) = model.comp_index.get(sa.compartment.as_str()) {
                        if let Some(local) = model.global_to_int[global] {
                            let old_val = snapshot.counts[local];
                            if crate::chain_binomial::trace_enabled() {
                                eprintln!("EVENT '{}' at t={:.1}: set {} = {} (was {})",
                                    iv.name, t_end, sa.compartment, new_val, old_val);
                            }
                            pending_deltas.push((local, new_val - old_val));
                        }
                    }
                }
            }
        }
    }
    Ok(())
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
    let mut pending: Vec<(usize, i64)> = Vec::new();
    // `inject_event_deltas` uses `t_end = t + dt` for both the EvalCtx and
    // the step-index lookup, so pass `t = t_event - dt` to land on
    // `t_end = t_event`.
    inject_event_deltas(
        model, fire_steps, int_s, real_s, params, t_event - dt, dt, &mut pending,
    )?;
    let fired = !pending.is_empty();
    for (local, delta) in pending {
        int_s.counts[local] += delta;
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

fn apply_intervention(
    iv: &Intervention,
    iv_idx: usize,
    model: &CompiledModel,
    int_s: &mut IntState,
    real_s: &mut RealState,
    params: &[f64],
    t: f64,
    dt: f64,
) -> Result<(), SimError> {
    for (action_idx, action) in iv.actions.iter().enumerate() {
        let resolved_val = eval_resolved(
            &model.resolved.intervention_exprs[iv_idx][action_idx],
            &EvalCtx { model, int_s, real_s, params, t, dt, projected: None, int_float_override: None },
        );
        let resolved_val = finite_action_value(resolved_val, &iv.name, action, t)?;
        let trace = crate::chain_binomial::trace_enabled();
        match action {
            Action::FractionTransfer(ft) => {
                let frac = resolved_val.clamp(0.0, 1.0);
                if trace {
                    eprintln!("INTERVENTION '{}' at t={:.1}: transfer {} -> {} (frac={:.2})",
                        iv.name, t, ft.src, ft.dst, frac);
                }
                let src_global = *model.comp_index.get(ft.src.as_str())
                    .ok_or_else(|| SimError::UnknownCompartment(ft.src.clone()))?;
                let dst_global = *model.comp_index.get(ft.dst.as_str())
                    .ok_or_else(|| SimError::UnknownCompartment(ft.dst.clone()))?;

                if let (Some(s_local), Some(d_local)) = (
                    model.global_to_int[src_global],
                    model.global_to_int[dst_global],
                ) {
                    let transfer = ((int_s.counts[s_local] as f64) * frac).floor() as i64;
                    int_s.counts[s_local] -= transfer;
                    int_s.counts[d_local] += transfer;
                } else if let (Some(s_local), Some(d_local)) = (
                    model.global_to_real[src_global],
                    model.global_to_real[dst_global],
                ) {
                    let transfer = real_s.values[s_local] * frac;
                    real_s.values[s_local] -= transfer;
                    real_s.values[d_local] += transfer;
                }
            }

            Action::AbsoluteTransfer(at) => {
                let n = resolved_val;
                if trace {
                    eprintln!("INTERVENTION '{}' at t={:.1}: transfer {} -> {} (raw={:.2})",
                        iv.name, t, at.src, at.dst, n);
                }
                let src_global = *model.comp_index.get(at.src.as_str())
                    .ok_or_else(|| SimError::UnknownCompartment(at.src.clone()))?;
                let dst_global = *model.comp_index.get(at.dst.as_str())
                    .ok_or_else(|| SimError::UnknownCompartment(at.dst.clone()))?;

                if let (Some(s_local), Some(d_local)) = (
                    model.global_to_int[src_global],
                    model.global_to_int[dst_global],
                ) {
                    let transfer = (n.round() as i64).min(int_s.counts[s_local]);
                    int_s.counts[s_local] -= transfer;
                    int_s.counts[d_local] += transfer;
                } else if let (Some(s_local), Some(d_local)) = (
                    model.global_to_real[src_global],
                    model.global_to_real[dst_global],
                ) {
                    let transfer = n.min(real_s.values[s_local]);
                    real_s.values[s_local] -= transfer;
                    real_s.values[d_local] += transfer;
                }
            }

            Action::Set(sa) => {
                let v = resolved_val;
                if trace {
                    eprintln!("INTERVENTION '{}' at t={:.1}: set {} = {:.2}",
                        iv.name, t, sa.compartment, v);
                }
                let global = *model.comp_index.get(sa.compartment.as_str())
                    .ok_or_else(|| SimError::UnknownCompartment(sa.compartment.clone()))?;
                if let Some(local) = model.global_to_int[global] {
                    int_s.counts[local] = v.round() as i64;
                } else if let Some(local) = model.global_to_real[global] {
                    real_s.values[local] = v;
                }
            }

            Action::Add(aa) => {
                let n = resolved_val;
                let count = n.round() as i64;
                if trace {
                    eprintln!("INTERVENTION '{}' at t={:.1}: add {} += {} (raw={:.2})",
                        iv.name, t, aa.compartment, count, n);
                }
                if count < 0 {
                    // gh#audit-C5 / S2. Action::Add resolving to a
                    // negative value is a config bug — the user wrote
                    // a fit.toml or DSL expression that produces a
                    // negative add. There's no inference scenario
                    // where you "discover" that an intervention should
                    // remove individuals via Add. Always hard error,
                    // regardless of caller (forward-sim or inference).
                    return Err(SimError::NegativeCount {
                        compartment: aa.compartment.clone(),
                        attempted_value: count,
                        t,
                        cause: crate::error::NegativeCountCause::InterventionAddNegative,
                    });
                }
                let global = *model.comp_index.get(aa.compartment.as_str())
                    .ok_or_else(|| SimError::UnknownCompartment(aa.compartment.clone()))?;
                if let Some(local) = model.global_to_int[global] {
                    int_s.counts[local] += count;
                } else if let Some(local) = model.global_to_real[global] {
                    real_s.values[local] += n;
                }
            }
        }
    }
    Ok(())
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
