---
date: 2026-06-04
status: proposal
related: ../notes/hierarchical-priors-gate2-plan.md, ../notes/2026-06-04-alpha-issue-triage.md
issue: gh#175
---

# Gate 3b: hierarchical-prior gradient for PGAS + NUTS

## Problem

PGAS + NUTS silently fails on every model with a hierarchical prior. The
idiomatic stratified prior

```camdl
R0[patch] ~ normal(mu = R0_mu, sigma = R0_sd)
```

compiles each leaf `R0_patch` to a `Prior::Hierarchical`. In the NUTS
gradient evaluator the hierarchical arm is stubbed:

```rust
// pgas.rs::prior_log_density_and_grad_z
Prior::Hierarchical(_) => return (f64::NEG_INFINITY, 0.0),
```

and the non-gradient MH fallback uses the non-env `log_density`, which is
also `-inf` for hierarchical (`prior.rs`). So the log-posterior is `-inf`
everywhere, the leapfrog Hamiltonian is infinite, every transition is
"divergent," acceptance is 0%, and the θ-chain freezes at its warm start.
Reproduced (gh#175): a 5-patch model with literal-constant priors mixes at
83% acceptance; flip the leaf prior to hierarchical and it is 0% / 100%
divergent. A frozen posterior warm-started at truth looks tight and
well-mixed — the dangerous silent-wrong mode for a tool that informs
public-health decisions.

An immediate guard (committed) makes PGAS hard-error on hierarchical priors,
pointing the user at `algorithm = pmmh` (which supports them). This proposal
is the real fix: implement the gradient and remove the guard.

## Where this sits

Per [`hierarchical-priors-gate2-plan.md`](../notes/hierarchical-priors-gate2-plan.md):

- **Gate 1** — language surface + IR classification (`parameter.hierarchical`).
- **Gate 2** — env-aware `log p(leaf | hyper)`, scipy-validated; wires
  hierarchical priors into **PMMH** (which needs only the density). Shipped.
- **Gate 3** — wire hierarchical priors into the gradient-based algorithms.
  Gate 2 explicitly deferred "gradient of the hierarchical log-density,
  needed for NUTS, to Gate 3 when we touch `pgas_grad.rs`." This is that gate.

The density already exists and is validated (`hierarchical::hierarchical_log_density`),
the `ParamEnv` / `NamedParams` plumbing exists, and **PMMH is a working
template** (`pmmh.rs` builds the env and calls `log_density_env`). The only
missing piece is the gradient.

## The math, and the one structural change

For a single hierarchical leaf `θ_leaf ~ Normal(μ(φ), σ(φ))`, where the
arguments `μ, σ` are expressions over hyperparameters `φ` (themselves
estimated parameters with their own NUTS z-coordinates), the leaf's
log-density contributes to the gradient slots of **three** coordinates, not
one:

- `∂/∂z_leaf  = [-(θ_leaf - μ)/σ²] · dθ_leaf/dz_leaf`
- `∂/∂z_{μ}   = [ (θ_leaf - μ)/σ²] · dμ/dz_{μ}`  (μ an estimated hyper)
- `∂/∂z_{σ}   = [ (θ_leaf - μ)²/σ³ - 1/σ] · dσ/dz_{σ}`

The current accumulation loop is **per-parameter-independent**:

```rust
// pgas.rs (the NUTS target)
let (prior_val, prior_grad_z) = prior_log_density_and_grad_z(&priors[i], &if2_params[i], theta, z[i]);
grad_z[i] += prior_grad_z;          // ← only ever touches slot i
```

A plain prior touches only its own slot; a hierarchical leaf's prior must
**also** add to its hyperparameters' slots. This cross-parameter coupling is
the one structural change: `prior_log_density_and_grad_z` must take the env
and return contributions to multiple `z` indices (e.g. a small
`Vec<(usize, f64)>` of `(z_index, grad)` pairs, resolved via a parameter
name → z-index map), and the loop accumulates each into `grad_z`.

This is the gradient analogue of Gate 2's Class-B (hyperparameter lookup) and
Class-D/A3 (transform-Jacobian, the IC3 no-double-count contract): the
`dθ/dz` and `dμ/dz` chain-rule factors must use the same per-parameter
`transform_deriv` the plain-prior arms already use, on the z-scale, so the
hierarchical gradient composes with `Transform::Log` without double-counting.

## Scope of change

| File | Change |
|---|---|
| `rust/crates/sim/src/inference/pgas.rs` | Thread the env (`NamedParams` over current θ + a name→z-index map) into `prior_log_density_and_grad_z`; implement the hierarchical arm returning multi-slot contributions; accumulate cross-terms in the NUTS loop; fix the MH-fallback density (use `log_density_env`). Remove the gh#175 guard. |
| `rust/crates/sim/src/inference/hierarchical.rs` | Add `hierarchical_log_density_grad` (analytic ∂/∂args) alongside the existing density, per supported `HierarchicalKind` (Normal first; LogNormal/HalfNormal/Gamma/Beta as the density already supports). |
| `rust/crates/sim/tests/gradient_check.rs` (or a new `gradient_check_hierarchical.rs`) | Finite-difference validation of the hierarchical gradient against the density, on the z-scale, per family — the existing FD-test pattern. |
| `rust/crates/sim/tests/pgas_gate_hierarchical.rs` | Once Gate 3b lands, this guard test inverts: the 2-patch hierarchical model must now **run** and assert acceptance ∈ [15%, 90%] and post-burn-in divergence < 10% (warm-started at truth). |

## Test obligations (no Gate without these)

1. **FD-gradient battery** — for each supported family, the analytic
   hierarchical gradient matches central finite differences of
   `hierarchical_log_density` to ≤ 1e-6 relative error, at z-scale, at bulk /
   tail / near-boundary points. Mirrors Gate 2's A1–A4 for the gradient.
2. **Cross-term correctness** — a 2-level Normal-Normal model: perturb a
   hyperparameter `z_μ` by Δ and confirm the analytic `∂/∂z_μ` predicts the
   density change (catches a dropped or mis-indexed hyper slot — Gate 2's
   B1/B2 for the gradient).
3. **Transform composition** — a log-transformed leaf: the gradient on the
   z-scale composes with the Jacobian without double-counting (the IC3 / A3
   contract, in gradient form).
4. **End-to-end recovery** — `run_pgas` on a 2-patch hierarchical model,
   warm-started at truth, recovers `μ_hyper, σ_hyper, leaf[patch]` within 2σ
   and reports acceptance ∈ [15%, 90%]. This is the test whose absence let
   gh#175 ship.

## Non-centered reparameterisation (companion, optional)

Centered hierarchical models have a funnel geometry that NUTS handles poorly
even with a correct gradient (Neal's funnel). The Gate-2 plan flagged
non-centered reparameterisation as "ships with Gate 3 inference wiring." It is
a sampling-efficiency concern, not a correctness one, so it is **optional for
Gate 3b**: land the correct gradient first (test 4 may need a wide-σ regime to
pass at the centered parameterisation), then add non-centered as a follow-up
if the funnel bites on realistic σ. Flag in the proposal so it is a conscious
deferral, not an omission.

## Effort

~2–4 focused days, consistent with the Gate-2 estimate (~3 days). The density,
env plumbing, and FD-test harness all exist; the new work is the analytic
gradient per family (mechanical, FD-validated), the cross-parameter
accumulation (the one conceptual change), and the recovery integration test.
Inference-math risk: every gradient arm must be FD-validated before the
recovery test is trusted — "plausible" is not "verified."
