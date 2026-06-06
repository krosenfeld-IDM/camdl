# Stage-0 oracle: validating inference before the unified-timeline refactor

Date: 2026-06-06
Project: camdl
Tags: inference, testing, recovery, regression-oracle, unified-timeline

## Context

The unified-timeline-effect refactor
([proposal](../proposals/2026-06-05-unified-timeline-effect-architecture.md))
rewrites the inference filter loop, the schedule, and the substep lifecycle. Its
"Stage 0" is to build the comparison oracle *before* the refactor flies: you
cannot prove a refactor byte-identical against a baseline that was never
captured, and the existing forward ratchet
(`gate_trajectory_baseline.rs`) scores nothing, so it cannot catch a refactor
that moves the likelihood. This note records what we built and what we learned
validating it.

## The recovery-vs-regression distinction (the load-bearing one)

Two artifacts, opposite rules — conflating them produces a false test:

- **Regression baseline** (the byte-identical refactor gate): one **fixed seed**,
  pinned, **never selected on whether it recovers**. The gate asks only "does the
  refactored code reproduce this loglik on this exact input"; recovery-to-truth is
  irrelevant to it. Choosing a dataset *because* it recovers well would bias the
  ratchet toward easy cases and test nothing.
- **Recovery validation** (is the inference correct): **all seeds, no selection** —
  a multi-dataset sweep, reported as a distribution.

## What "recovery" means

Not "θ̂ within bounds" (bounds are wide — tests nothing) and not "θ̂ = truth"
(impossible under Monte-Carlo + finite-sample bias). Recovery = **(1)** chains
converge consistently (the inference reached the MLE) **and (2)** truth falls
within the sampling spread — truth ∈ mean(θ̂) ± ~2·SE over the sweep, the
tolerance the sweep itself measures.

## What the SIR sweep taught us (the inference is sound)

The `sir` recovery case (`tests/recovery/cases/sir`, book getting-started SIR;
see its README) at truth β=0.4, γ=0.15:

- A **single** fit returned θ̂ = (0.476, 0.285) — γ ~2× truth — which looked
  alarming. It is not: a paired `pfilter` loglik check showed θ̂ is **+0.94 nats
  above truth**, stable across seeds, so the IF2 found the *genuine MLE*. The
  offset is Monte-Carlo on one realization (the γ ridge is broad), **not**
  under-convergence — so cooling / warmer starts do not help (they re-find the
  same MLE). The `ll(truth)` vs `ll(θ̂)` comparison is the cheap diagnostic that
  distinguishes the two.
- An **8-seed sweep** confirmed it is the data, not a systematic bias: the MLEs
  *straddle* truth (γ below on seeds 4 & 8, on it for seed 5, above otherwise;
  R0 ranges 1.43→3.46 around 2.67). Means: β̄ 0.43, γ̄ 0.20, R̄0 2.39 — truth
  brackets within ~2·SE, with a **mild right-skew lean** in γ (the documented
  NegBin-obs / ridge effect the book's fitting chapter is about — not a code bug;
  the external test is pomp on the same datasets).
- `seir_age` is the contrasting failure: **bimodal**, the 4-chain IF2 scout
  splits across basins (γ≈0.9 vs ≈0.12) and recovers nothing — it needs 8–16
  chains before it can be a reference. Convergence-consistency is the bar it
  fails; `sir` passes it.

Conclusion: the inference machinery is correct; the recovery gate is
consistent-convergence + MC-bracketing, and the per-dataset accuracy question is
a separate model-science study, not a refactor-regression gate.

## Artifacts built

- `tests/recovery/cases/sir/` — the validated recovery reference + README (sweep
  table, ll-check, baseline policy).
- `scripts/recovery_pairs.py` — uv-runnable (PEP-723) pair/corner plot for
  recovery diagnostics, truth overlaid.
- `rust/crates/sim/tests/gate_inference_baseline.rs` — the byte-identical
  PF-loglik ratchet. Runs the bootstrap filter at fixed (params, seed=42,
  particles=8000, dt=1) on in-memory references and asserts the loglik
  byte-for-byte. References: `sir_incidence_truth` (on-grid, −59.451 — matches the
  CLI `pfilter` at truth) and `sir_incidence_offgrid` (obs at 7.3, 14.6, … —
  −59.789, a *distinct* path the on-grid corpus cannot reach). Determinism +
  non-vacuity verified. Dev-machine ratchet (libm ULP caveat).
- `tests/fixtures/corner_cases/` — five timeline edge cases (off-grid
  intervention, coincident obs+intervention, fractional output end, off-grid obs,
  all-lifecycle) with observed behavior in the README. These make the eventual
  Stage-1 parity non-vacuous (the round-2 review's #1 finding: an all-on-grid
  corpus passes parity vacuously).

## Incidental findings

- **Interventions are off by default** in forward `simulate` — they need
  `--enable <name>` (or a scenario). Events (`always_active`) fire unconditionally.
- The off-grid divergence is **real but sub-step**: at coarse integer output it is
  small (off-grid intervention S[t3] = 475 vs 477 across backends), invisible to
  eyeballing but fatal to a byte-identical gate — which is why the gate, not a
  human reading a trajectory, is the right tool.
- `balance` requires a population-typed RHS — a bare integer literal is
  dimensionless and fails dimcheck (E302); use a `count` parameter or `pop(t)`.

## Forward oracle (done)

`gate_corner_case_baseline.rs` pins the forward trajectories of all five corner
cases under every capability-supported backend (17 baselines, FNV hash, seed=42),
from committed IR (`tests/fixtures/corner_cases/ir/*.ir.json`, params baked via
`--set`, regenerated by `make update-corner-golden`). The snap-vs-exact
divergence is a regression surface — `off_grid_intervention` hashes differently
under `chain_binomial` (snap) and `tau_leap` (exact); `gillespie` pins the
`iv_resolution_dt` phantom grid; `all_lifecycle` pins the fused
ADVANCE → intervention → balance → observe order. Non-vacuity verified.

## Next

- An off-grid-*intervention* inference reference (the forward divergence pinned
  above, now scored through a filter).
- The PGAS/IF2/PMMH + per-obs + RNG-draw-count harnesses — the inference-side
  Stage-0 remainder beyond the bootstrap-PF marginal already covered.
- `seir_age` config (8–16 chains) → a second converging inference reference.
- pomp cross-check on the `sir` datasets to settle whether the mild γ lean is the
  estimator or a camdl bug.
