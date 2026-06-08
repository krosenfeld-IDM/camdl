use crate::{
    compiled_model::CompiledModel,
    config::{SimConfig, TauLeapConfig},
    rng::StatefulRng,
    error::SimError,
    intervention::all_intervention_times,
    lineage::TransitionObserver,
    ode_integrator::rk4_step,
    output::output_times as get_output_times,
    propensity::{eval_propensities, EvalCtx},
    resolved_expr::eval_resolved,
    schedule::{Cursor, Schedule, StepPolicy},
    simulate::Simulate,
    state::{FlowVec, Snapshot, Trajectory},
};

pub struct TauLeapSim;

impl Simulate for TauLeapSim {
    fn run(
        &self,
        model: &CompiledModel,
        params: &[f64],
        seed: u64,
        config: &SimConfig,
    ) -> Result<Trajectory, SimError> {
        let cfg = match config {
            SimConfig::TauLeap(c) => c,
            _ => return Err(SimError::ConfigMismatch {
                expected: "TauLeap",
                got: config.variant_name(),
            }),
        };
        run_tau_leap(model, params, seed, cfg)
    }

    fn capabilities(&self) -> crate::Capabilities {
        crate::Capabilities::OVERDISPERSION
            | crate::Capabilities::REAL_COMPARTMENTS
            | crate::Capabilities::LINEAGES
    }

    fn name(&self) -> &'static str { "tau_leap" }
}

pub fn run_tau_leap(
    model: &CompiledModel,
    params: &[f64],
    seed: u64,
    cfg: &TauLeapConfig,
) -> Result<Trajectory, SimError> {
    run_tau_leap_with_observer(model, params, seed, cfg, None, None)
}

/// Tau-leap run with an optional [`TransitionObserver`] (individual-sampling
/// layer, Phase 3). `observer = None` reproduces [`run_tau_leap`] byte-for-byte
/// — the observer reads its own RNG stream and is invoked only after the
/// simulation RNG has drawn each transition's count, passing the
/// **start-of-step** state. This is the trajectory-invariance invariant
/// (Tier 2a). Because tau-leap fires `k` events of a transition against frozen
/// start-of-step rates, the observer is told `multiplicity = k` and samples
/// parents from a frozen pool snapshot (the `dt`-bias the diagnostic measures).
pub fn run_tau_leap_with_observer(
    model: &CompiledModel,
    params: &[f64],
    seed: u64,
    cfg: &TauLeapConfig,
    mut observer: Option<&mut dyn TransitionObserver>,
    // Per-timestep progress tick (RNG-free; `None` == byte-identical). See
    // chain_binomial.rs and tests/progress_tick_invariance.rs.
    mut tick: Option<&mut dyn FnMut(f64)>,
) -> Result<Trajectory, SimError> {
    let (mut int_s, mut real_s) = model.initial_state(params)?;
    let n_transitions = model.model.transitions.len();
    let n_real = real_s.values.len();

    // gh#53: resolve fire_steps using the runtime cfg.dt, not the
    // compile-time model.simulation.dt. See chain_binomial.rs for the
    // full explanation. gh#69: passes `params` for parametric `at [...]`
    // schedules.
    let fire_steps = model.resolve_fire_steps(cfg.dt, params);

    let mut rng = StatefulRng::new(seed);
    let mut propensities = Vec::with_capacity(n_transitions);

    // The merged timeline spine: tau-leap is the EXACT policy (clip dt to land on
    // each output/effect boundary). The schedule owns the sorted output/effect
    // times; `cursor` walks them. Firing stays inline (apply_interventions_at);
    // the schedule only answers "where is the next stop, what is due".
    let schedule = Schedule::new(
        cfg.dt,
        cfg.t_end,
        cfg.dt,
        StepPolicy::Exact,
        get_output_times(&model.model.output.times),
        all_intervention_times(model, params),
    );
    let mut cursor = Cursor::default();

    let mut traj = Trajectory::new();
    let mut current_flows = FlowVec::new(n_transitions);
    let mut t = cfg.t_start;

    // Initial snapshot
    if schedule.output_due_at(&cursor, t) {
        traj.push(Snapshot {
            t,
            int_state: int_s.clone(),
            real_state: real_s.clone(),
            flows: current_flows.clone(),
        });
        current_flows.reset();
        cursor.pass_output();
    }

    while t < cfg.t_end {
        // Progress tick: report current time before drawing this step. RNG-free.
        if let Some(cb) = tick.as_deref_mut() { cb(t); }

        // The schedule is the single source of truth for the step size:
        // dt.min(min(t_end, next_output, next_effect) - t) — the original formula,
        // bit-exact (not (t+dt)-t).
        let dt = schedule.substep(&cursor, t).expect("t < t_end inside loop");
        if dt <= 0.0 {
            // At a boundary — handle it.
            // Canonical within-substep lifecycle (matches chain_binomial): the
            // event PROPOSE stage reads the start-of-step snapshot, then ADVANCE
            // (no transition step here — dt<=0), then INTERVENE. With no
            // in-flight transition the snapshot IS the current state, so the
            // event delta and the (post-advance) intervention agree. `t` is
            // already the boundary; pass `t - cfg.dt` so the shared seam's
            // `t_end = t + dt` lands on the boundary with the cfg.dt step key.
            if schedule.effect_time(&cursor).is_some_and(|iv| (iv - t).abs() < 1e-10) {
                let mut ev = crate::effects::EffectDeltas::default();
                crate::effects::resolve_events(
                    model, &fire_steps, &int_s, &real_s, params, t - cfg.dt, cfg.dt, cfg.dt,
                    &mut ev,
                )?;
                for d in &ev.int {
                    int_s.counts[d.idx] += d.delta;
                }
                for d in &ev.real {
                    real_s.values[d.idx] += d.delta;
                }
                crate::lifecycle::apply_post_advance(
                    model, &fire_steps, &mut int_s, &mut real_s, params,
                    t - cfg.dt, cfg.dt, cfg.dt, 1e-10, None,
                )?;
                while schedule.effect_due_at(&cursor, t) { cursor.pass_effect(); }
            }
            // Record output if due
            while schedule.output_due_at(&cursor, t) {
                let ot = schedule.output_time(&cursor).expect("due implies present");
                traj.push(Snapshot {
                    t: ot,
                    int_state: int_s.clone(),
                    real_state: real_s.clone(),
                    flows: current_flows.clone(),
                });
                current_flows.reset();
                cursor.pass_output();
            }
            if t >= cfg.t_end { break; }
            continue;
        }

        // SNAPSHOT-CAPTURE HAZARD: freeze the start-of-step state BEFORE any
        // draw reads/mutates it. The transition draws below read `int_s.counts`
        // and accumulate into `pending_deltas` (they do not mutate `int_s`
        // in place until the drain), so this clone is the true pre-advance
        // snapshot. The event PROPOSE stage MUST read this snapshot — not the
        // post-drain `int_s` — so a value-dependent event on a draining
        // compartment (e.g. transfer(fraction=f, from=A) while A --> B fires)
        // reads pre-drain A, exactly like chain_binomial. Capturing after the
        // drain would silently use post-drain A.
        let snap_int = int_s.clone();
        let snap_real = real_s.clone();

        // Evaluate propensities at current state
        eval_propensities(model, &int_s, &real_s, params, t, cfg.dt, &mut propensities)?;

        // Pre-evaluate draw method for each transition (resolves overdispersion
        // σ² expressions from start-of-step state before any mutations).
        enum ResolvedDraw { Poisson, Deterministic, Overdispersed(f64) }
        let draws: Vec<ResolvedDraw> = {
            let ctx = EvalCtx { model, int_s: &int_s, real_s: &real_s, params, t, dt: cfg.dt, projected: None, int_float_override: None };
            model.model.transitions.iter().enumerate()
                .map(|(i, tr)| match &tr.draw_method {
                    ir::transition::DrawMethod::Poisson => ResolvedDraw::Poisson,
                    ir::transition::DrawMethod::Deterministic => ResolvedDraw::Deterministic,
                    ir::transition::DrawMethod::Overdispersed(_) => {
                        let sigma_sq = eval_resolved(model.resolved.overdispersion[i].as_ref().unwrap(), &ctx);
                        ResolvedDraw::Overdispersed(sigma_sq)
                    }
                })
                .collect()
        };
        // RM1 in 2026-04-19 engine review: for transitions that share
        // a source compartment (competing exits), independent Poisson
        // draws can produce more total exits than the source has
        // individuals, silently violating population conservation via
        // clamp_nonneg. Match chain-binomial's Euler-multinomial:
        //  1. Draw total exits from Binomial(n_src, 1-exp(-Σr_k·dt)).
        //  2. Split total multinomially with weights r_k/Σr_k.
        // For ungrouped transitions (inflows, non-competing exits)
        // keep the standard tau-leap independent Poisson draw.
        let mut handled = vec![false; n_transitions];
        let mut pending_deltas: Vec<(usize, i64)> = Vec::new();
        // Per-transition fired counts this step (for the lineage observer). Only
        // populated when an observer is attached; the observer is fed these
        // *after* all draws but *before* deltas are applied, so it sees the
        // start-of-step state. Zero-cost when no observer.
        let mut fired_counts: Vec<u64> = if observer.is_some() {
            vec![0; n_transitions]
        } else {
            Vec::new()
        };
        for &(src_local, ref group) in &model.source_groups {
            let n_src = int_s.counts[src_local].max(0);
            if n_src == 0 {
                for &tr_idx in group { handled[tr_idx] = true; }
                continue;
            }
            // Compute effective per-capita rates (with overdispersion if any).
            let mut effective: Vec<(usize, f64)> = Vec::with_capacity(group.len());
            let mut total_rate = 0.0_f64;
            for &tr_idx in group {
                let rate = propensities[tr_idx];
                if rate <= 0.0 { handled[tr_idx] = true; continue; }
                let per_capita = rate / n_src as f64;
                let eff = match draws[tr_idx] {
                    ResolvedDraw::Deterministic => {
                        // Handle deterministic separately below; don't compete.
                        handled[tr_idx] = true;
                        continue;
                    }
                    ResolvedDraw::Overdispersed(sigma_sq) => {
                        per_capita * rng.gamma_multiplier(sigma_sq, dt)
                    }
                    ResolvedDraw::Poisson => per_capita,
                };
                total_rate += eff;
                effective.push((tr_idx, eff));
            }
            if total_rate <= 0.0 || effective.is_empty() { continue; }
            // gh#audit-H3: stable (p, q) primitive (q discarded here).
            let (p_total, _q) = crate::inference::numerics::prob_q_from_rate_dt(total_rate, dt);
            let p_total = p_total.clamp(0.0, 1.0);
            let mut n_events = rng.binomial(n_src as u64, p_total);
            let n_competing = effective.len();
            let mut rate_remaining = total_rate;
            for (k, &(tr_idx, eff_rate)) in effective.iter().enumerate() {
                let count = if k == n_competing - 1 {
                    n_events
                } else if n_events > 0 && rate_remaining > 0.0 {
                    let p_split = (eff_rate / rate_remaining).clamp(0.0, 1.0);
                    let c = rng.binomial(n_events, p_split);
                    n_events -= c;
                    rate_remaining -= eff_rate;
                    c
                } else {
                    0
                };
                for &(local, delta) in &model.transition_stoich[tr_idx] {
                    pending_deltas.push((local, delta * count as i64));
                }
                current_flows.add(tr_idx, count);
                if !fired_counts.is_empty() { fired_counts[tr_idx] += count; }
                handled[tr_idx] = true;
            }
        }

        // Inflows and ungrouped transitions: independent draws per the
        // standard tau-leap approximation.
        for (i, &lambda) in propensities.iter().enumerate() {
            if handled[i] { continue; }
            let mean = lambda * dt;
            let count = match draws[i] {
                ResolvedDraw::Poisson => rng.poisson(mean),
                ResolvedDraw::Deterministic => mean.round() as u64,
                ResolvedDraw::Overdispersed(sigma_sq) => rng.neg_binomial(mean, sigma_sq, dt),
            };
            for &(local, delta) in &model.transition_stoich[i] {
                pending_deltas.push((local, delta * count as i64));
            }
            current_flows.add(i, count);
            if !fired_counts.is_empty() { fired_counts[i] += count; }
        }

        // Lineage observer: fed each transition's fired count for this step
        // BEFORE deltas are applied, so the observer sees the start-of-step
        // (frozen) state and pools. The observer owns a separate RNG stream, so
        // these calls cannot perturb the count trajectory (Tier 2a).
        if let Some(obs) = observer.as_deref_mut() {
            obs.begin_batch_step();
            for (tr_idx, &count) in fired_counts.iter().enumerate() {
                if count > 0 {
                    obs.on_fired(tr_idx, 0, count, t, &int_s, &real_s, params)?;
                }
            }
            obs.end_batch_step();
        }

        // PROPOSE (stage 1): always_active event deltas from the FROZEN
        // start-of-step snapshot, FUSED into the transition deltas so both are
        // applied atomically in the drain below — exactly like chain_binomial.
        // This substep lands on the boundary `t + dt` (Exact policy); fire only
        // when that boundary is a scheduled effect time. The boundary time is
        // `t + dt`; pass `(t + dt) - cfg.dt` so the seam's `t_end` lands on the
        // boundary with the cfg.dt step key (matching the prior
        // `apply_events_at(t + dt, …, cfg.dt)` evaluation point — only the
        // READ-SOURCE changes: snapshot, not post-drain state).
        let boundary = t + dt;
        if schedule.effect_time(&cursor).is_some_and(|iv| (iv - boundary).abs() < 1e-10) {
            let mut ev = crate::effects::EffectDeltas::default();
            crate::effects::resolve_events(
                model, &fire_steps, &snap_int, &snap_real, params,
                boundary - cfg.dt, cfg.dt, cfg.dt, &mut ev,
            )?;
            for d in &ev.int {
                pending_deltas.push((d.idx, d.delta));
            }
            for d in &ev.real {
                real_s.values[d.idx] += d.delta;
            }
        }

        for (local, delta) in pending_deltas.drain(..) {
            int_s.counts[local] += delta;
        }

        // gh#audit-C5 / S2. Negative count after stoichiometry → hard
        // error (BinomialOvershoot cause). The multinomial invariant
        // (RM10 / 2026-04-19 review) says this shouldn't happen on
        // tau-leap; if it does, the user wants to know. Inference
        // layers catch and recover per-particle.
        if let Some((local, val)) = int_s.first_negative() {
            return Err(crate::error::SimError::NegativeCount {
                compartment: model.comp_index.iter()
                    .find(|(_, &g)| model.global_to_int.get(g).copied().flatten() == Some(local))
                    .map(|(n, _)| n.clone())
                    .unwrap_or_else(|| format!("(local-int-{local})")),
                attempted_value: val,
                t,
                cause: crate::error::NegativeCountCause::BinomialOvershoot,
            });
        }

        // RK4 for real compartments (integer state now at end-of-step)
        if n_real > 0 {
            rk4_step(model, &int_s, &mut real_s, params, t, dt)?;
            real_s.clamp_nonneg();
        }

        t += dt;

        // INTERVENE (stage 3) on the post-advance state, if now at an effect
        // boundary. The event PROPOSE stage already fired this substep (fused
        // pre-drain, reading the snapshot), so only the intervention runs here —
        // on the post-advance, post-event state. `t` is the boundary; pass
        // `t - cfg.dt` so the seam's `t_end` lands on `t` with the cfg.dt step
        // key. Balance is chain-only (None here).
        if schedule.effect_time(&cursor).is_some_and(|iv| (iv - t).abs() < 1e-10) {
            crate::lifecycle::apply_post_advance(
                model, &fire_steps, &mut int_s, &mut real_s, params,
                t - cfg.dt, cfg.dt, cfg.dt, 1e-10, None,
            )?;
            while schedule.effect_due_at(&cursor, t) { cursor.pass_effect(); }
        }

        // Record outputs
        schedule.drain_outputs(&mut cursor, t, |ot| {
            traj.push(Snapshot {
                t: ot,
                int_state: int_s.clone(),
                real_state: real_s.clone(),
                flows: current_flows.clone(),
            });
            current_flows.reset();
        });
    }

    // Flush remaining outputs
    schedule.drain_outputs(&mut cursor, f64::INFINITY, |ot| {
        traj.push(Snapshot {
            t: ot,
            int_state: int_s.clone(),
            real_state: real_s.clone(),
            flows: current_flows.clone(),
        });
        current_flows.reset();
    });

    Ok(traj)
}
