# Substep-time convention: `t_start + s·dt` everywhere

Status: accepted (maintainer, 2026-06-05) — adopt the robust convention,
accept the result changes for time-inhomogeneous models.
Supersedes: nothing. Part of the unified-timeline-effect architecture
(`2026-06-05-unified-timeline-effect-architecture.md`).

## Decision

Every fixed-step integrator computes the substep start time — the `t` passed to
rate / forcing evaluation — as `window_start + s·dt` (s = within-window substep
index), **not** by accumulation (`t += dt`). This is the only value `t` that
reaches a time-dependent rate (`propensity.rs:87` `Expr::Time → ctx.t`,
`:186` `eval_time_func(…, ctx.t)`), so it is the only thing that changes.

## Why (the robustness argument)

`s·dt` is one multiply + one add → bounded **O(1)** rounding error. Accumulation
is `s` separate roundings → error grows **O(s)**. Measured: summing `0.1` out to
1100 days drifts the clock to `1100.1000000000095` (vs exact `1100`), a peak
`|accumulated − s·dt|` of **1.6e-10 days**, which for annual seasonal forcing is
a **~1e-12 relative** shift in the rate. Integer counts are insensitive to that
(forward trajectories byte-identical), but the **continuous** PGAS transition
density (`gamma`, `shape = dt/σ²`) is not — so today the forward simulator and the
PGAS likelihood sample seasonal forcing at *different* times for the same model.
That latent forward/inference disagreement is what this removes. Runtime cost:
nil (multiply vs add). It is purely an accuracy + consistency improvement.

## Blast radius

- **Changes:** time-inhomogeneous models (`forcing {}`, any `t`-dependent rate) at
  **fractional `dt`** over long horizons. Their forward trajectories and the
  EXACT-stepper inference results shift by the drift (ULP-scale per substep).
- **Unchanged:** time-homogeneous models (`t` never enters a rate — the two
  conventions are identical). Integer-`dt` runs (accumulation == `s·dt` exactly).
  PGAS / pgas_grad (already `s·dt`). Gillespie (continuous SSA, absolute event
  times — no grid). NUTS leapfrog (its `dt` is the HMC step size, not a sim grid).

## Exhaustive site inventory (verified by grep + read, 2026-06-05)

| site | role | current | anchor after fix |
| --- | --- | --- | --- |
| `chain_binomial.rs:211,304` | forward SNAP | `t += dt` (whole run) | **global** `t_start + s·dt` (match PGAS) |
| `tau_leap.rs:150,301` | forward EXACT | `t += dt` per clipped window | per-window `window_start + s·dt` |
| `ode.rs:254,261` | forward EXACT | `t += dt` per clipped window | per-window `window_start + s·dt` |
| `particle_filter.rs:243,251` | bootstrap PF | `t_local += step_dt` per obs window | per-obs-window |
| `correlated_pf.rs:342,347,354` | correlated PF | `t_local += step_dt` | per-obs-window |
| `if2.rs:409,415` | IF2 | `t_local += step_dt` | per-obs-window |
| `pmmh.rs` | PMMH | runs a PF for L̂(θ); no own stepping loop | inherits PF / correlated_pf fix |
| `pgas.rs:568,605,716` | PGAS | `t_start + s·dt` ✓ | unchanged (canonical) |
| `pgas_grad.rs:397` | PGAS gradient | `t_start + s·dt` ✓ | unchanged |
| `gillespie.rs` | forward SSA | absolute `t = iv_t/boundary/t_next` | unaffected |

The inference EXACT steppers (PF/correlated_pf/if2) all advance the kernel via
`ChainBinomialProcess::step` (`chain_binomial_process.rs:92`), which forwards the
caller's `t` to `step_one`. So fixing the `t_local` each one computes is the fix;
the kernel needs no change.

## Anchoring

- **SNAP** (chain_binomial forward, PGAS): global grid, `window_start = t_start`,
  step count `= interval_steps(t_start, t_end, dt)`. chain_binomial adopts PGAS's
  exact convention so the same model samples forcing identically in sim and fit.
- **EXACT** (tau_leap, ode, PF, correlated_pf, if2): the grid re-anchors at each
  clip — `window_start` = the boundary (output / intervention) or obs time the
  stepper just landed on; `s` resets to 0 there. This bounds the drift to a single
  inter-boundary window (≤ `window/dt` substeps) instead of the whole run.

## Implementation

A single robust helper on `Schedule` (one source of truth):

```rust
/// Substep start time: window_start + s*dt, bit-exact regardless of s.
pub fn substep_time(&self, window_start: f64, s: u64) -> f64 {
    window_start + s as f64 * self.dt
}
```

Each stepper tracks `(window_start, s)`, passes `schedule.substep_time(window_start, s)`
to the kernel, and re-anchors `(window_start = boundary, s = 0)` at each clip
(EXACT) or never (SNAP, global). Step size still comes from `Schedule::substep`
(the bit-exact `dt.min(boundary - t)` already landed in 16a61c8).

## Baselines that regenerate

- Forward seasonal goldens: `seir_vaccine_seasonal`, `seir_seasonal_patch`,
  `seir_spatial_5_inference` (any with `forcing {}` at fractional `dt`).
  `make update-golden && make update-expected`; review the diff is ONLY the
  seasonal/fractional-dt models.
- `gate_trajectory_baseline` / `gate_corner_case_baseline`: recapture; the new
  `seasonal_drift` corner case is added here as the permanent pin.
- `gate_inference_baseline`: the `sir` references are `dt=1` (integer) and
  homogeneous → **unchanged** (regression check: they must not move).

## Verification

1. Time-homogeneous + integer-`dt` corpus: byte-identical (regression — must not
   move). This is the proof the change is scoped to what we intend.
2. `seasonal_drift` (chain_binomial, `dt=0.1`): forward trajectory + a PGAS loglik
   baseline now AGREE on forcing-sample times (the consistency this buys).
3. The `schedule::tests::substep_is_bit_exact_*` style pin for `substep_time`.

## Open item

Confirm PMMH's stepping path (does it call `bootstrap_filter`, inheriting the
fix, or duplicate a stepping loop?). Resolve during implementation.
