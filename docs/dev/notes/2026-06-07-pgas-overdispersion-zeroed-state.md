# PGAS gamma-density value evaluates σ² at a zeroed state

Date: 2026-06-07
Project: camdl
Tags: inference, pgas, overdispersion, nuts, value-gradient-mismatch
Status: finding (reviewer trace, NOT yet independently reproduced) — needs a
failing test before it becomes an incident.

## Context

Surfaced by an adversarial review of `feature/unified-timeline` (the inference-
math reviewer). **Pre-existing on `main`** — not introduced by that branch
(`git show main:rust/crates/sim/src/inference/pgas.rs` shows the same zeroed
`int_s_local` at the σ² eval). Filed here so it is not lost; flagged to the
maintainer as a candidate next investigation.

## The claim (reviewer trace — verify before acting)

In `complete_data_loglik`, the gamma-multiplier (overdispersion) density loop at
`rust/crates/sim/src/inference/pgas.rs:759-784` builds
`int_s_local = IntState::new(n_int_local)` (all zeros, ~line 759) and evaluates
`sigma_sq = eval_resolved(resolved_od, &ctx)` against that **zeroed** state
(~line 784) — even though the rate check just above (lines 772-776) correctly
uses `rec.counts_before`.

The three sibling sites all evaluate σ² at the **real** start-of-step state:

- `step_one` — `chain_binomial.rs:353-359` (σ² at the live state).
- `log_transition_density_substep` — `pgas.rs:576-587` (σ² at `counts_before`).
- `log_gamma_density_grad_substep` — `pgas_grad.rs:290-302` (σ² at `counts_before`).

## Why it would matter

For a model with **state-dependent** overdispersion, e.g.
`overdispersed(rate, sigma_base * S / N0)` (σ² an expression over a live
compartment), the gamma term in the *value* uses σ²(state = 0) while the
*gradient* and the *simulator* use σ²(state = counts_before). NUTS then
differentiates a value function it does not match → biased acceptance / wrong
posterior on the σ²-controlling parameter. For the common case (σ² = a bare
parameter, state-independent) there is no error — the zeroed state never reaches
a `Pop`/`PopSum` node, so the evaluated σ² is identical.

This is exactly the class camdl treats as priority-zero: a silent wrong
posterior, not a crash.

## Open questions (resolve before filing as an incident)

1. **Is state-dependent σ² expressible at all?** Confirm the DSL/IR permits
   `overdispersed(rate, <expr-over-Pop>)` rather than only a constant/parameter.
   If σ² is constrained to be state-independent, this is dead-but-tidy (the
   zeroed eval is still wrong-in-principle but unreachable) — downgrade to a
   one-line `int_s_local → counts_before` cleanup.
2. **TDD repro.** Compile a model with `overdispersed(rate, sigma_base * S / N0)`,
   build a 1-substep trajectory, and assert `complete_data_loglik(...).transition`
   equals an independent recompute that evaluates σ² at `counts_before` — it
   should differ by the gamma term (RED). Cross-function FD (value vs
   `complete_data_loglik_grad`) on the σ²-coupled parameter should also fail.

## Related coverage gap (same review)

No test exercises overdispersion (the gamma `shape = dt_substep/σ²` term) under
`StepPolicy::Exact` clipping — the exact-PGAS oracles (`pgas_exact_tiling.rs`,
`gate_dt_rate_exact_clip.rs`) run on non-overdispersed models. The `dt_substep`
threading was traced correct, but the gamma-density × clipping interaction is
unoracled in both value and gradient. A combined fixture (overdispersed SEIR +
off-grid obs) would close both this gap and pin the fix for the bug above.

## Next

1. Answer Q1 (grep the DSL/dimcheck for whether σ² accepts a `Pop` expr).
2. If yes: write the RED test (Q2), fix `int_s_local → counts_before`, confirm
   GREEN, add the overdispersion-under-Exact oracle.
3. If no: downgrade to the unreachable-cleanup + add a guard/assert.
