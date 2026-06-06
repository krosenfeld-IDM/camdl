//! The merged timeline spine — one `Schedule` answering "where does the
//! integrator stop next, and what is due there" for every stepping idiom.
//!
//! Today that question is answered three incompatible ways (verified against the
//! backends, 2026-06-05):
//!
//!   - **chain_binomial** (`chain_binomial.rs:170-237`) — steps a FULL `cfg.dt`
//!     every substep (`dt = cfg.dt.min(t_end - t)`, never clipped to an
//!     output/effect time); outputs are emitted post-step at grid times; effects
//!     fire *inside* `step_one` keyed on `resolve_fire_steps(cfg.dt)` (a step
//!     index), NOT on the loop's boundary detection. This is the **snap** policy.
//!   - **tau_leap / ode** (`tau_leap.rs:110-139`, `ode.rs:205-239`) — compute
//!     `next_boundary = min(t_end, out_t, iv_t)` and clip
//!     `dt = cfg.dt.min(next_boundary - t)`, landing EXACTLY on each output/effect
//!     time, where they fire via `apply_interventions_at(t, …, 1e-10)`. This is
//!     the **exact** policy.
//!   - **gillespie** (`gillespie.rs:144-246`) — PROPOSES an exponential time and
//!     clips it back to `min(t_end, out_t, iv_t)`. The **clip** query.
//!
//! This module makes the time→step mapping ONE thing with an explicit policy and
//! an explicit grid, rather than three call-site conventions. The grid is a FIELD
//! (`cfg.dt` for chain/tau/ode, `model.simulation.dt.unwrap_or(1.0)` —
//! gillespie's `iv_resolution_dt`, `gillespie.rs:120` — for gillespie), the
//! single source of truth, so a model never snaps interventions on one grid and
//! observations on another.
//!
//! ## Scope of the byte-identical extraction (Stage 1)
//!
//! Stage 1 unifies the **boundary cursor**: the next-stop time, the substep
//! cadence, and the output-emission walk (the `while output_times[idx] <= t + 1e-12`
//! block duplicated in all four backends). It does NOT move where interventions
//! *fire*: chain_binomial keeps its `fire_steps`-in-`step_one` mechanism, the
//! exact backends keep `apply_interventions_at` at the clipped boundary
//! ("interventions as today"). Unifying the firing mechanism is a behaviour change
//! (it would move chain_binomial's snap-vs-exact divergence — see
//! `tests/fixtures/corner_cases/off_grid_intervention.camdl`) and belongs to the
//! `--obs-alignment` knob (Stage 2), not here. Accordingly, under `Snap` the
//! schedule reports only `Output` boundaries; effects are the backend's business.
//!
//! ## The CRN invariant (the one regression that breaks silently)
//!
//! `Schedule` is immutable and `Sync`; the per-particle [`Cursor`] is `Copy`.
//! [`Schedule::next_boundary`] is a PURE function of `(Schedule, cursor, t)` with
//! no interior mutability, so N particles in a parallel swarm walk an IDENTICALLY
//! ordered boundary sequence — paired-seed / CRN coupling depends on this, and a
//! shared-mutable cursor would corrupt it without failing any all-on-grid golden.
//! Pinned by [`tests::n_cursors_identical_sequence`].

/// How a fixed-step driver maps off-grid output/effect times onto its `dt` grid.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StepPolicy {
    /// chain_binomial: step a full `dt` every substep, never clipping to land on
    /// an output/effect time. Outputs are emitted at grid points; effects are
    /// snapped to a step elsewhere (`resolve_fire_steps`).
    Snap,
    /// tau_leap / ode: clip the step so the integrator lands exactly on the next
    /// output/effect boundary.
    Exact,
}

/// Per-particle walk position over a shared [`Schedule`]. `Copy` — each particle
/// holds its own; the schedule is never mutated.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Cursor {
    pub output_idx: usize,
    pub effect_idx: usize,
}

/// Tolerance for "an output time has been reached" — matches the
/// `<= t + 1e-12` test the backends use at output emission.
const OUTPUT_EPS: f64 = 1e-12;
/// Tolerance for "an effect time has been reached" — matches the `1e-10` the
/// exact backends pass to `apply_interventions_at`.
const EFFECT_EPS: f64 = 1e-10;

/// Immutable, `Sync`, shared by every particle. Construction sorts the boundary
/// times once; [`Cursor`] walks them. See the module header for the invariant.
#[derive(Clone, Debug)]
pub struct Schedule {
    dt: f64,
    t_end: f64,
    /// The snap grid (`cfg.dt` for chain/tau/ode; `iv_resolution_dt` for
    /// gillespie). Reserved for the Stage-2 `exact` sub-grid tiling; the `Snap`
    /// path emits effects off this grid via the backend's `resolve_fire_steps`.
    grid: f64,
    policy: StepPolicy,
    /// Sorted, ascending. Output snapshot times within `[t_start, t_end]`.
    output_times: Vec<f64>,
    /// Sorted, ascending. Scheduled effect (intervention / always-active event)
    /// boundary times.
    effect_times: Vec<f64>,
}

impl Schedule {
    /// Build from the same inputs the backends compute today: the integrator
    /// `dt`, the run window `[t_start, t_end]`, the snap `grid`, the policy, and
    /// the sorted output / effect time vectors (`get_output_times`,
    /// `all_intervention_times`). Times are assumed already sorted ascending (the
    /// producers guarantee it); debug-asserted.
    pub fn new(
        dt: f64,
        t_end: f64,
        grid: f64,
        policy: StepPolicy,
        output_times: Vec<f64>,
        effect_times: Vec<f64>,
    ) -> Self {
        debug_assert!(output_times.windows(2).all(|w| w[0] <= w[1]), "output_times not sorted");
        debug_assert!(effect_times.windows(2).all(|w| w[0] <= w[1]), "effect_times not sorted");
        Schedule { dt, t_end, grid, policy, output_times, effect_times }
    }

    fn next_output(&self, cursor: &Cursor) -> f64 {
        self.output_times.get(cursor.output_idx).copied().unwrap_or(f64::INFINITY)
    }

    fn next_effect(&self, cursor: &Cursor) -> f64 {
        self.effect_times.get(cursor.effect_idx).copied().unwrap_or(f64::INFINITY)
    }

    /// The substep size to advance from `t`. The SINGLE source of truth for the
    /// time→step mapping, computed the way the original backends did —
    /// `dt.min(boundary - t)`, NOT `(t + dt).min(boundary) - t`. The two are equal
    /// in exact arithmetic but NOT bit-identical for large fractional `t`
    /// (`(t + dt) - t != dt` once `t` is large relative to `dt`). The forward
    /// integer-count draws are insensitive to that ULP, but the chain-binomial
    /// transition density (`shape = dt/σ²`) PGAS evaluates is continuous and
    /// sensitive — so the inference loglik would move. Returning the step size
    /// directly keeps every consumer bit-exact for all `t`.
    ///
    /// PURE in `(self, cursor, t)` — does not mutate the cursor (the backend
    /// advances it via [`Cursor::pass_output`] / [`Cursor::pass_effect`] after
    /// handling what is due, using [`Schedule::output_due_at`] /
    /// [`Schedule::effect_due_at`]). Returns `None` once `t >= t_end`.
    ///
    /// - `Exact`: clip to the next boundary —
    ///   `dt.min(min(t_end, next_output, next_effect) - t)`. A zero/negative
    ///   result means `t` is already AT a boundary (the backend handles it
    ///   without stepping).
    /// - `Snap`: never clip to an output/effect — `dt.min(t_end - t)`.
    pub fn substep(&self, cursor: &Cursor, t: f64) -> Option<f64> {
        if t >= self.t_end {
            return None;
        }
        let boundary = match self.policy {
            StepPolicy::Exact => {
                self.t_end.min(self.next_output(cursor)).min(self.next_effect(cursor))
            }
            StepPolicy::Snap => self.t_end,
        };
        Some(self.dt.min(boundary - t))
    }

    /// Gillespie's query: the process proposes `t_proposed` (drawn exponential)
    /// from the current time `t`; the schedule clips it back to the nearest
    /// boundary if one falls before it, else passes it through. The boundary is
    /// `min(t_end, next_output, next_effect-strictly-after-t)`.
    ///
    /// The `> t` filter on the effect (but NOT the output) is deliberate and
    /// matches the SSA's boundary semantics: an effect exactly at `t` has already
    /// been applied this iteration and must not re-fire, whereas an output exactly
    /// at `t` is still recorded. (Reproduces gillespie.rs's `next_iv` `> t` guard
    /// vs `next_out_t` raw — the asymmetry is observable only when a reaction
    /// lands exactly on a boundary time.) Returns the clipped time and whether it
    /// hit a boundary (vs a reaction firing at `t_proposed`).
    pub fn clip(&self, cursor: &Cursor, t: f64, t_proposed: f64) -> ClipResult {
        let eff = self.effect_time(cursor).filter(|&e| e > t).unwrap_or(f64::INFINITY);
        let boundary = self.t_end.min(self.next_output(cursor)).min(eff);
        if boundary < t_proposed {
            ClipResult { t: boundary, hit_boundary: true }
        } else {
            ClipResult { t: t_proposed, hit_boundary: false }
        }
    }

    /// Whether an output is due at `t` for `cursor` (the `<= t + OUTPUT_EPS` test).
    pub fn output_due_at(&self, cursor: &Cursor, t: f64) -> bool {
        self.next_output(cursor) <= t + OUTPUT_EPS
    }

    /// Whether an effect is due at `t` for `cursor` (the `<= t + EFFECT_EPS` test).
    pub fn effect_due_at(&self, cursor: &Cursor, t: f64) -> bool {
        self.next_effect(cursor) <= t + EFFECT_EPS
    }

    /// The current (next un-emitted) output time for `cursor`, or `None` past the
    /// end. The backend records its snapshot AT this time.
    pub fn output_time(&self, cursor: &Cursor) -> Option<f64> {
        self.output_times.get(cursor.output_idx).copied()
    }

    /// The current (next un-applied) effect time for `cursor`, or `None` past the
    /// end. The backend keeps its own firing-tolerance check against this time
    /// (e.g. tau-leap's `(iv - t).abs() < 1e-10`).
    pub fn effect_time(&self, cursor: &Cursor) -> Option<f64> {
        self.effect_times.get(cursor.effect_idx).copied()
    }

    pub fn t_end(&self) -> f64 {
        self.t_end
    }

    pub fn grid(&self) -> f64 {
        self.grid
    }

    /// The start time of the `s`-th substep within a window beginning at
    /// `window_start`: `window_start + s*dt`. The SINGLE source of truth for the
    /// time passed to rate / forcing evaluation. Computed by multiplication (one
    /// rounding, O(1) error) rather than accumulation (`t += dt`, O(s) drift), so
    /// the same model samples time-inhomogeneous forcing at identical times in the
    /// forward simulator and in PGAS — which already uses `t_start + s*dt`. SNAP
    /// steppers anchor `window_start = t_start` (global grid); EXACT steppers
    /// re-anchor `window_start` to each boundary/obs they clip to.
    /// See docs/dev/proposals/2026-06-05-substep-time-sdt-convention.md.
    pub fn substep_time(&self, window_start: f64, s: u64) -> f64 {
        window_start + s as f64 * self.dt
    }
}

impl Cursor {
    /// Advance past the current output (after the backend has recorded it).
    pub fn pass_output(&mut self) {
        self.output_idx += 1;
    }

    /// Advance past the current effect (after the backend has applied it).
    pub fn pass_effect(&mut self) {
        self.effect_idx += 1;
    }
}

/// Result of [`Schedule::clip`].
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ClipResult {
    pub t: f64,
    pub hit_boundary: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn exact(dt: f64, t_end: f64, out: Vec<f64>, eff: Vec<f64>) -> Schedule {
        Schedule::new(dt, t_end, dt, StepPolicy::Exact, out, eff)
    }
    fn snap(dt: f64, t_end: f64, out: Vec<f64>, eff: Vec<f64>) -> Schedule {
        Schedule::new(dt, t_end, dt, StepPolicy::Snap, out, eff)
    }

    /// Walk the whole schedule into a sequence of substep sizes (bit-exact),
    /// draining cursors exactly as a backend would.
    fn walk(s: &Schedule) -> Vec<u64> {
        let mut cur = Cursor::default();
        let mut t = 0.0_f64;
        let mut seq = Vec::new();
        let mut guard = 0;
        while let Some(step_dt) = s.substep(&cur, t) {
            seq.push(step_dt.to_bits());
            t += step_dt;
            // Drain what is due at the new t, as a backend does.
            while s.output_due_at(&cur, t) {
                cur.pass_output();
            }
            while s.effect_due_at(&cur, t) {
                cur.pass_effect();
            }
            guard += 1;
            assert!(guard < 100_000, "walk did not terminate");
        }
        seq
    }

    #[test]
    fn exact_clips_to_off_grid_effect() {
        // off_grid_intervention: cull at t=2.5, dt=1, t_end=5. Exact lands on 2.5.
        let s = exact(1.0, 5.0, vec![], vec![2.5]);
        let cur = Cursor::default();
        // First step from 0 → full dt (no boundary inside (0,1]).
        assert_eq!(s.substep(&cur, 0.0).unwrap(), 1.0);
        // From t=2, the 2.5 boundary clips the step to 0.5 (lands on 2.5).
        assert_eq!(s.substep(&cur, 2.0).unwrap(), 0.5, "exact must clip to the off-grid effect");
        assert!(s.effect_due_at(&cur, 2.5), "effect is due at the clipped landing");
    }

    #[test]
    fn snap_steps_full_dt_over_off_grid_effect() {
        // Same model under Snap: the step is NOT clipped to 2.5; effects never
        // surface as schedule boundaries (the backend's step_one fires them).
        let s = snap(1.0, 5.0, vec![], vec![2.5]);
        let cur = Cursor::default();
        assert_eq!(s.substep(&cur, 2.0).unwrap(), 1.0, "snap steps a full dt past the off-grid effect");
    }

    #[test]
    fn exact_and_snap_diverge_on_off_grid() {
        // The corner-case divergence, at the schedule layer: the two policies
        // produce DIFFERENT substep sequences for an off-grid effect.
        let e = walk(&exact(1.0, 5.0, vec![0.0, 1.0, 2.0, 3.0, 4.0, 5.0], vec![2.5]));
        let n = walk(&snap(1.0, 5.0, vec![0.0, 1.0, 2.0, 3.0, 4.0, 5.0], vec![2.5]));
        assert_ne!(e, n, "off-grid snap vs exact must differ (the pinned divergence)");
    }

    #[test]
    fn coincident_output_and_effect() {
        // coincident_obs_intervention: output and effect both at t=10.
        let s = exact(1.0, 12.0, vec![10.0], vec![10.0]);
        let cur = Cursor::default();
        assert_eq!(s.substep(&cur, 9.0).unwrap(), 1.0, "steps to land on 10");
        assert!(
            s.output_due_at(&cur, 10.0) && s.effect_due_at(&cur, 10.0),
            "coincident kinds both due at the landing"
        );
    }

    #[test]
    fn fractional_end_clips_last_step() {
        // fractional_output_end: t_end = 80.5, dt = 1. The final step clips to 0.5
        // (lands on 80.5); stepping terminates there.
        let s = exact(1.0, 80.5, vec![], vec![]);
        let cur = Cursor::default();
        assert_eq!(s.substep(&cur, 80.0).unwrap(), 0.5);
        assert!(s.substep(&cur, 80.5).is_none(), "terminates at t_end");
    }

    #[test]
    fn substep_is_bit_exact_dt_min_not_t_to_minus_t() {
        // The robustness property: at large fractional t the substep is
        // dt.min(boundary - t) EXACTLY, NOT the FP-fragile (t+dt).min(boundary) - t.
        // Forward integer draws are insensitive to the ULP, but the PGAS continuous
        // transition density (shape = dt/σ²) is not — so the inference loglik would
        // move. This test pins the fix.
        let dt = 0.1;
        let s = exact(dt, 5000.0, vec![], vec![]);
        let cur = Cursor::default();
        let t = 1095.7275;
        let got = s.substep(&cur, t).unwrap();
        assert_eq!(
            got.to_bits(),
            dt.min(5000.0 - t).to_bits(),
            "substep must be the robust dt.min(boundary - t)"
        );
        let fragile = (t + dt).min(5000.0) - t;
        assert_ne!(got.to_bits(), fragile.to_bits(), "the fragile (t+dt)-t formula differs here");
    }

    #[test]
    fn substep_time_is_sdt_drift_free() {
        // window_start + s*dt is bit-exact (one multiply), while accumulating
        // dt over s steps drifts. At dt=0.1, s=10000 the two diverge.
        let dt = 0.1;
        let s = snap(dt, 5000.0, vec![], vec![]);
        let n: u64 = 10_000;
        // s*dt form:
        let sdt = s.substep_time(0.0, n);
        assert_eq!(sdt.to_bits(), (n as f64 * dt).to_bits());
        // accumulation drifts away from it:
        let mut acc = 0.0_f64;
        for _ in 0..n {
            acc += dt;
        }
        assert_ne!(acc.to_bits(), sdt.to_bits(), "accumulation must drift from s*dt by s=10000");
        // and s*dt equals the true grid point exactly (1000.0):
        assert_eq!(sdt, 1000.0);
        // window re-anchoring (EXACT steppers): offset by the window start.
        assert_eq!(s.substep_time(2.5, 3), 2.5 + 3.0 * dt);
    }

    #[test]
    fn gillespie_clip_passes_through_and_clips() {
        let s = exact(1.0, 100.0, vec![5.0], vec![3.0]);
        let cur = Cursor::default();
        // Proposed reaction before the next boundary (3.0): pass through.
        let r = s.clip(&cur, 0.0, 2.4);
        assert_eq!(r.t, 2.4);
        assert!(!r.hit_boundary);
        // Proposed reaction past the boundary: clip to 3.0.
        let r = s.clip(&cur, 0.0, 3.7);
        assert_eq!(r.t, 3.0);
        assert!(r.hit_boundary);
    }

    #[test]
    fn clip_excludes_effect_exactly_at_t_but_not_output() {
        // The SSA asymmetry: a reaction landing exactly on an effect time (t=3.0,
        // effect at 3.0) must NOT clip back to it (already applied) — the > t
        // filter excludes it, so the proposed 4.0 passes through (next is out=5.0).
        let s = exact(1.0, 100.0, vec![5.0], vec![3.0]);
        let cur = Cursor::default();
        let r = s.clip(&cur, 3.0, 4.0);
        assert_eq!(r.t, 4.0);
        assert!(!r.hit_boundary);
        // An OUTPUT exactly at t is NOT excluded (no > t filter): output at 3.0,
        // t=3.0, proposed 4.0 → clips to 3.0.
        let s2 = exact(1.0, 100.0, vec![3.0], vec![]);
        let r2 = s2.clip(&cur, 3.0, 4.0);
        assert_eq!(r2.t, 3.0);
        assert!(r2.hit_boundary);
    }

    #[test]
    fn n_cursors_identical_sequence() {
        // THE CRN invariant: N independent cursors over one immutable Schedule
        // walk a byte-identical boundary sequence (next_boundary is pure; the
        // cursor is the only per-particle state).
        let s = exact(0.7, 13.3, vec![0.0, 2.0, 4.0, 6.0, 8.0, 10.0, 12.0], vec![3.5, 7.1, 11.0]);
        let reference = walk(&s);
        for _ in 0..64 {
            assert_eq!(walk(&s), reference, "per-cursor walk must be byte-identical");
        }
        assert!(!reference.is_empty());
    }
}
