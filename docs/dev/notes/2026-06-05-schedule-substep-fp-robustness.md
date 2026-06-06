# Schedule substep must be `dt.min(boundary - t)`, not `(t+dt) - t`

Date: 2026-06-05
Project: camdl
Tags: schedule, unified-timeline, floating-point, inference, pgas

## Context

While wiring the forward backends through the merged `Schedule`
(unified-timeline Stage 1), the first version of `Schedule::next_boundary`
returned the *landing time* `t_to = (t+dt).min(boundary)` and each backend
computed its step as `step_dt = t_to - t`. The original backends computed
`step_dt = dt.min(boundary - t)` directly. These are equal in exact arithmetic
but **not bit-identical** in f64.

## The bug

`(t + dt) - t` is not `dt` once `t` is large relative to `dt`. Verified:

```
t=1095.7275, dt=0.1, boundary far:
  (t+dt).min(b) - t = 0.0999999999999090   # the t_to - t formulation
  dt.min(b - t)     = 0.1                   # the original formulation
  → differ by ~9e-14 (many ULPs)
```

4 of 8 realistic `(t, dt, boundary)` cases differed at the bit level (large or
fractional `t`).

## Why it was invisible to the Stage-1 gates

The forward trajectory gate (`gate_trajectory_baseline`,
`gate_corner_case_baseline`) and the bootstrap-PF marginal gate
(`gate_inference_baseline`) are **integer-valued**: chain-binomial draws integer
counts, and a ~1e-13 perturbation in `step_dt` essentially never flips a binomial
integer draw (the RNG threshold would have to fall in a 1e-13-wide window).
Confirmed empirically: an endemic SIS at `dt=0.1` out to `t=1100`, run under the
`t_to - t` code and the pre-Schedule original, produced a **byte-identical**
trajectory hash. So the forward path is genuinely insensitive — the gates were
not wrong, they cannot see this.

The **PGAS** path is different. The chain-binomial *transition density*
(`gamma`, `shape = dt/σ²`, `Var = σ²/dt`, `pgas.rs`) is a **continuous** function
of the realized `step_dt`; a ULP shift moves the scored log-density. So the
fragility is invisible to every integer-valued gate that exists today but **would
move the PGAS loglik at large fractional `t`** once PGAS is routed through the
schedule. The existing Stage-0 oracle (forward trajectory + PF marginal) could
never have caught it.

## The fix

`Schedule::substep(cursor, t) -> Option<f64>` returns the step size directly, as
the original backends did:

- `Exact`: `dt.min(min(t_end, next_output, next_effect) - t)`
- `Snap`:  `dt.min(t_end - t)`

This is byte-identical to the original per-backend formula by construction (same
operations, same operands), so it is correct for **all** `t`, not just the
integer `t` the gates happen to exercise. The three fixed-step backends
(tau_leap, ode, chain_binomial) now read `substep` instead of `t_to - t`;
gillespie was never affected (it uses absolute boundary times via `clip`, never
`t + dt`).

Pinned by `schedule::tests::substep_is_bit_exact_dt_min_not_t_to_minus_t`
(asserts `substep == dt.min(boundary - t)` AND `!= (t+dt).min(boundary) - t` at
`t = 1095.7275, dt = 0.1`). Forward + PF gates still byte-identical (the integer
draws are insensitive, as expected).

## Consequence for the inference routing

When PGAS is routed through the schedule, it MUST take `step_dt` from `substep`
(robust), never reconstruct it as `t_to - t`. The Stage-0 oracle should grow a
**large-fractional-`t` PGAS loglik baseline** — that is the gate that would
actually catch a regression here; the integer-valued gates cannot.

## Process note

This was surfaced by sequencing the inference probe *before* Stage 2 (the
maintainer's "expose the largest problem first" call). Reading the PF stepping to
check whether the schedule abstraction fits the inference path is what exposed
the `t_to - t` interface as FP-fragile — a latent issue in already-committed
Stage-1 forward code that no existing test could fail on.
