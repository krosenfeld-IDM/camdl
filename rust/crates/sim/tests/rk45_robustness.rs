//! gh#166 Phase C: adaptive rk45 (Dopri5) robustness gates on the real-world
//! cells the smooth oracle models don't exercise (PR #231 review).
//!
//! - INTERVENTION PULSE: rk45 must land exactly on a mid-horizon intervention
//!   boundary (`h_max = next_boundary − t`), so the discrete transfer fires at
//!   the right instant and the post-pulse trajectory + incidence match fine-dt
//!   RK4. This was the proposal's named validation target.
//! - ERROR CONTROL: a stiff/fast-transient model at a loose tolerance must drive
//!   the controller to shrink (and reject) steps yet still match the reference —
//!   a bug in `shrink_factor`/the PI controller would diverge here. An
//!   unsatisfiable (zero) tolerance must fail LOUDLY (honest hard error), not
//!   return a silent coarse result. (See the finding in that test: a smooth
//!   model integrates fine even at 1e-300, and the `H_MIN` guard is shadowed by
//!   the max-rejections guard — so zero tolerance is the deterministic trigger.)
//! - REAL_COMPARTMENTS: the real-state branch of `dopri5_try_step` (separate
//!   from the integer branch) must agree with fine-dt RK4.
//! - atol/rtol LOAD-BEARING: a loose tolerance must land measurably further from
//!   the fine-dt reference than a tight one — proof the tolerances actually
//!   reach the integrator and are not silently dropped in plumbing.

use sim::{
    compiled_model::CompiledModel,
    config::{OdeConfig, SimConfig},
    simulate::Simulate,
    OdeSim,
};

fn repo_path(rel: &str) -> String {
    format!("{}/../../../{}", env!("CARGO_MANIFEST_DIR"), rel)
}

fn load_model(rel: &str) -> ir::Model {
    let path = repo_path(rel);
    let json = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {path}: {e}"));
    ir::from_str(&json).unwrap_or_else(|e| panic!("parse {rel}: {e}"))
}

fn run_with(
    model: ir::Model,
    params: &[f64],
    dt: f64,
    t_end: f64,
) -> sim::state::Trajectory {
    let compiled = CompiledModel::new(model).expect("compile");
    OdeSim
        .run(
            &compiled,
            params,
            0,
            &SimConfig::Ode(OdeConfig { t_start: 0.0, t_end, dt }),
        )
        .expect("ode run")
}

/// Run with the model's own baked/default parameters.
fn run_default(model: ir::Model, dt: f64, t_end: f64) -> sim::state::Trajectory {
    let compiled = CompiledModel::new(model).expect("compile");
    let params = compiled.default_params.clone();
    OdeSim
        .run(
            &compiled,
            &params,
            0,
            &SimConfig::Ode(OdeConfig { t_start: 0.0, t_end, dt }),
        )
        .expect("ode run")
}

fn rk4(model: &ir::Model) -> ir::Model {
    let mut m = model.clone();
    m.simulation.integrator = ir::model::Integrator::Rk4;
    m
}
fn rk45(model: &ir::Model, atol: f64, rtol: f64) -> ir::Model {
    let mut m = model.clone();
    m.simulation.integrator = ir::model::Integrator::Rk45 { atol: Some(atol), rtol: Some(rtol) };
    m
}

/// Bake concrete values onto a model's (otherwise estimated) parameters by name,
/// so `CompiledModel::new` can build `default_params`.
fn bake(model: &ir::Model, vals: &[(&str, f64)]) -> ir::Model {
    let mut m = model.clone();
    for p in &mut m.parameters {
        if let Some((_, v)) = vals.iter().find(|(n, _)| *n == p.name) {
            p.value = p.value.with_value(*v);
        }
    }
    m
}

// ───────────────────────────── intervention pulse ─────────────────────────────

#[test]
fn rk45_lands_on_intervention_pulse_and_matches_fine_rk4() {
    // SEIR+V with `transfer(fraction = vacc_frac, S -> V) at [180, 545, 910]`.
    // Concrete params (declaration order: beta, sigma, gamma, omega, vacc_frac,
    // N0, I0) so the SIA transfer has a visible (vacc_frac = 0.8) effect; both
    // integrators see identical params.
    let base = bake(
        &load_model("ocaml/golden/seir_vaccine.ir.json"),
        &[
            ("beta", 0.3), ("sigma", 0.2), ("gamma", 0.1), ("omega", 0.003),
            ("vacc_frac", 0.8), ("N0", 100_000.0), ("I0", 10.0),
        ],
    );
    let t_end = 1095.0; // 3 years
    let rk4_traj = run_default(rk4(&base), 0.05, t_end);
    let rk45_traj = run_default(rk45(&base, 1e-10, 1e-10), 1.0, t_end);

    assert_eq!(
        rk4_traj.snapshots.len(),
        rk45_traj.snapshots.len(),
        "rk4/rk45 must share the output grid"
    );

    // Non-vacuity: the SIA actually fired — S compartment (index 0) drops sharply
    // across the first pulse at t = 180. Check the fine-RK4 trajectory.
    let s_at = |traj: &sim::state::Trajectory, t: f64| -> i64 {
        traj.snapshots
            .iter()
            .min_by(|a, b| (a.t - t).abs().partial_cmp(&(b.t - t).abs()).unwrap())
            .map(|s| s.int_state.counts[0])
            .unwrap()
    };
    let s_before = s_at(&rk4_traj, 179.0);
    let s_after = s_at(&rk4_traj, 181.0);
    assert!(
        (s_after as f64) < 0.5 * (s_before as f64),
        "SIA pulse at t=180 should move ~80% of S→V: S(179)={s_before} S(181)={s_after} \
         — intervention did not fire (test would be vacuous)"
    );

    // Agreement: rk45 (adaptive, landing on the pulse) must match fine-dt RK4 on
    // prevalence AND incidence across the pulse boundary.
    let (mut worst_prev, mut worst_inc) = (0i64, 0.0f64);
    for (a, b) in rk4_traj.snapshots.iter().zip(&rk45_traj.snapshots) {
        assert!((a.t - b.t).abs() < 1e-9, "grid times must match at t={}", a.t);
        for (x, y) in a.int_state.counts.iter().zip(&b.int_state.counts) {
            let d = (x - y).abs();
            worst_prev = worst_prev.max(d);
            assert!(d <= 2, "prevalence rk4={x} vs rk45={y} at t={} (Δ {d})", a.t);
        }
        for (fx, fy) in a.flows.as_real().iter().zip(b.flows.as_real()) {
            let tol = 1e-1 + 1e-3 * fx.abs();
            let d = (fx - fy).abs();
            worst_inc = worst_inc.max(d / tol);
            assert!(
                d <= tol,
                "incidence rk4={fx} vs rk45={fy} at t={} (Δ {d:.3e} > tol {tol:.3e}) \
                 — rk45 mis-landed on the pulse boundary or the tableau is wrong",
                a.t
            );
        }
    }
    eprintln!(
        "intervention pulse: S(179)={s_before}→S(181)={s_after}; worst prevalence Δ {worst_prev}, \
         worst incidence {:.1}% of tol",
        100.0 * worst_inc
    );
}

// ───────────────────────────── error control ─────────────────────────────────

#[test]
fn rk45_handles_stiff_transient_at_loose_tol() {
    // Crank beta on the SIR oracle (index 0) to a sharp, fast epidemic. At the
    // DEFAULT (loose-ish) tolerance the controller must shrink/reject through the
    // peak; a broken shrink/PI controller diverges from the fine-dt reference.
    let base = load_model("tests/external/ode_oracle/models/sir.ir.json");
    let mut stiff = CompiledModel::new(base.clone()).expect("compile").default_params.clone();
    stiff[0] *= 8.0; // R0 ≈ 16 — a fast, sharp transient

    let t_end = 60.0;
    let reference = run_with(rk4(&base), &stiff, 0.002, t_end); // accurate fixed-step reference
    let adaptive = run_with(rk45(&base, 1e-8, 1e-6), &stiff, 1.0, t_end);
    let coarse = run_with(rk4(&base), &stiff, 2.0, t_end); // deliberately too-coarse fixed step

    let peak = |t: &sim::state::Trajectory| t.snapshots.iter().map(|s| s.int_state.counts[1]).max().unwrap();
    assert!(peak(&reference) > 50_000, "stiff SIR should peak high; got {}", peak(&reference));

    // Adaptive rk45 tracks the fine reference through the sharp peak ...
    let adaptive_err = reference
        .snapshots
        .iter()
        .zip(&adaptive.snapshots)
        .map(|(a, b)| (a.int_state.counts[1] - b.int_state.counts[1]).abs())
        .max()
        .unwrap();
    assert!(
        adaptive_err <= 50,
        "rk45 at default tol diverged from the fine reference on the stiff peak \
         (worst |ΔI| = {adaptive_err}) — shrink_factor / PI controller bug"
    );

    // ... while the too-coarse fixed step does NOT — proof the adaptive stepping
    // (the reject/shrink machinery) is doing real work, not riding the grid.
    let coarse_err = reference
        .snapshots
        .iter()
        .zip(&coarse.snapshots)
        .map(|(a, b)| (a.int_state.counts[1] - b.int_state.counts[1]).abs())
        .max()
        .unwrap();
    assert!(
        coarse_err > adaptive_err,
        "expected coarse fixed-step rk4 (worst |ΔI| {coarse_err}) to be WORSE than adaptive \
         rk45 ({adaptive_err}); if not, the model isn't actually stressing the controller"
    );
    eprintln!("stiff transient: adaptive |ΔI|={adaptive_err}, coarse-dt2 |ΔI|={coarse_err}");
}

#[test]
fn rk45_unsatisfiable_tolerance_errors_loudly() {
    // A tolerance no finite step can satisfy must produce an HONEST hard error
    // (naming the conflict + suggesting rk4 / a looser tol), never a silent
    // coarse trajectory.
    //
    // FINDING (PR #231): on a SMOOTH model even atol=rtol=1e-300 does NOT error —
    // the adaptive controller just takes accurate small steps and integrates to
    // roundoff-limited accuracy (the embedded error is genuinely satisfiable).
    // And the `H_MIN` underflow guard is SHADOWED by the max-rejections guard:
    // shrinking floors at DP_FACMIN=0.2, so after DP_MAX_REJECTIONS=10 rejections
    // h = h_max·0.2¹⁰ ≈ 1e-7·h_max, still ≫ h_min = 1e-10·span — so for any
    // reasonable span the rejection counter trips first. The deterministic
    // trigger for the error path is therefore a literally-zero tolerance: no
    // finite step has zero embedded error, so every step is rejected → the
    // honest hard error fires regardless of the model.
    let base = load_model("tests/external/ode_oracle/models/sir.ir.json");
    let compiled = CompiledModel::new(rk45(&base, 0.0, 0.0)).expect("compile");
    let params = compiled.default_params.clone();
    let res = OdeSim.run(
        &compiled,
        &params,
        0,
        &SimConfig::Ode(OdeConfig { t_start: 0.0, t_end: 60.0, dt: 1.0 }),
    );
    assert!(res.is_err(), "a zero tolerance must error, not return a coarse trajectory");
    let msg = format!("{:?}", res.unwrap_err()).to_lowercase();
    // honest: names the failure AND points at the fix (rk4 / looser tol).
    assert!(
        msg.contains("rejected") || msg.contains("underflow") || msg.contains("step-size"),
        "error must name the step-size failure, got: {msg}"
    );
    assert!(
        msg.contains("rk4") || msg.contains("loosen") || msg.contains("atol"),
        "error must suggest a fix (rk4 / loosen tolerance), got: {msg}"
    );
}

// ───────────────────────────── real compartments ─────────────────────────────

#[test]
fn rk45_real_compartments_match_fine_rk4() {
    // cholera_siwr has a REAL water reservoir `W` with genuine dynamics
    // (dW/dt = xi·I − omega_W·W), so W grows from infected shedding — it
    // exercises the real-state branch of `dopri5_try_step` (separate code from
    // the integer branch). All params are baked (fixed).
    let base = load_model("ir/golden/cholera_siwr.ir.json");
    assert!(
        base.compartments.iter().any(|c| matches!(c.kind, ir::model::CompartmentKind::Real)),
        "fixture must have a real compartment to exercise the real branch"
    );
    let t_end = base.simulation.t_end;
    let rk4_traj = run_default(rk4(&base), 0.002, t_end); // fine-dt reference
    let rk45_traj = run_default(rk45(&base, 1e-10, 1e-10), 1.0, t_end);

    assert_eq!(rk4_traj.snapshots.len(), rk45_traj.snapshots.len(), "shared grid");
    // Two accurate-but-different integrators agree on the real reservoir to ~0.2%
    // relative; a bug in the real branch of dopri5_try_step would diverge grossly,
    // not at the 4th-significant-figure level.
    let mut worst_rel = 0.0f64;
    let mut peak_w = 0.0f64;
    for (a, b) in rk4_traj.snapshots.iter().zip(&rk45_traj.snapshots) {
        for (x, y) in a.real_state.values.iter().zip(&b.real_state.values) {
            peak_w = peak_w.max(x.abs());
            let tol = 1e-1 + 2e-3 * x.abs();
            let d = (x - y).abs();
            worst_rel = worst_rel.max(d / x.abs().max(1.0));
            assert!(d <= tol, "real-state rk4={x} vs rk45={y} at t={} (Δ {d:.3e} > tol {tol:.3e})", a.t);
        }
    }
    assert!(peak_w > 10.0, "the real reservoir W never grew (peak {peak_w}) — test would be vacuous");
    eprintln!("real compartments: peak W {peak_w:.1}, worst relative Δ {:.2e}", worst_rel);
}

// ───────────────────────────── atol/rtol load-bearing ────────────────────────

#[test]
fn rk45_tolerances_are_load_bearing() {
    // A loose tolerance must land measurably FURTHER from the fine-dt reference
    // than a tight one. If the tolerances were dropped somewhere in the plumbing
    // (model → IR → OdeConfig → Dopri5), loose and tight would be identical and
    // this fails.
    let base = load_model("tests/external/ode_oracle/models/seir.ir.json");
    let t_end = 80.0;
    let reference = run_default(rk4(&base), 0.002, t_end);

    let dist = |traj: &sim::state::Trajectory| -> f64 {
        reference
            .snapshots
            .iter()
            .zip(&traj.snapshots)
            .flat_map(|(a, b)| {
                a.flows
                    .as_real()
                    .iter()
                    .zip(b.flows.as_real())
                    .map(|(x, y)| (x - y).abs())
                    .collect::<Vec<_>>()
            })
            .fold(0.0f64, f64::max)
    };
    let loose = dist(&run_default(rk45(&base, 1e-1, 1e-1), 1.0, t_end));
    let tight = dist(&run_default(rk45(&base, 1e-12, 1e-12), 1.0, t_end));

    assert!(
        loose > tight,
        "loose tol should be further from the reference than tight \
         (loose Δ {loose:.3e}, tight Δ {tight:.3e}) — tolerances not reaching the integrator?"
    );
    assert!(tight < 1e-1, "tight tol should track the reference closely; got Δ {tight:.3e}");
    eprintln!("load-bearing: loose Δ {loose:.3e} > tight Δ {tight:.3e}");
}
