# Incident: chain-binomial reads stale (zero) real-compartment state in transition rates

**Date:** 2026-06-07 **Status:** Forward-sim path fixed (uncommitted, pending
review). Inference path doubly-broken — diagnosed, scoped, NOT fixed here (see
§Inference scope). **Severity:** Critical for any model where an integer
transition's rate couples to a real-valued (ODE) compartment — the coupling is
silently evaluated as zero. Zero for all integer-only / real-decoupled models
(byte-identical). **Class:** code-vs-code (the chain-binomial backend disagreed
with ODE / Gillespie / tau-leap, which all read the real state correctly).

## Fundamental vs. implementation

**Fundamental:** nothing. The chain-binomial (Euler-multinomial) algorithm has
no opinion that would justify ignoring a real compartment in a rate. The
partially-deterministic Markov process (PDMP) coupling — integer compartments
jumping at rates that depend on a continuous reservoir — is exactly what the
cholera SIWR model is *for*.

**Implementation:** `step_one` (the core per-substep stepper in
`rust/crates/sim/src/chain_binomial.rs`) synced its scratch *integer* state from
the run's counts but never synced its scratch *real* state from the run's real
values. Propensities are evaluated against the scratch real state, which stayed
at its zero initialization for the entire run.

## Reproduction (concrete input → wrong output)

Fixture `rust/crates/sim/tests/fixtures/real_coupled_rate.ir.json`: integer
`S --> I` with rate `beta * (W/(W+kappa)) * S`, where `W` is a **real**
compartment held constant by `dW/dt = 0`. With `W` large the saturation term
`W/(W+kappa) ~ 1` (rate `~ beta*S`); with `W = 0` the rate is exactly 0.

Test `rust/crates/sim/tests/chain_binomial_real_state.rs`, run on the buggy code:

```
$ cargo test -p sim --test chain_binomial_real_state
test chain_binomial_rate_couples_to_real_compartment ... FAILED
  chain-binomial ignored the real compartment W in the infection rate:
  W=1e6 should drive ~beta*S infections, but fired 0 (identical to the
  W=0 run, inf_w0=0). This is the stale-real-state bug: step_one never
  synced scratch.real_s.
test chain_binomial_agrees_with_ode_and_gillespie_on_real_coupling ... FAILED
  chain-binomial did NOT deplete S (S_end=1000) while ODE (S_end=0) and
  Gillespie (S_end=0) did. chain-binomial ignored the real compartment W
  coupling in the rate.
```

With `W = 1e6` the correct rate is `~ beta*S = 0.5*S` and S should deplete
to ~0 over 20 days. The buggy chain-binomial fired **0 infections** and left
S at the full 1000 — identical to the `W = 0` run, proving it read `W ≡ 0`.
ODE and Gillespie (which advance and read the real state) both depleted S to 0.

The in-tree `cholera_siwr` golden (`ir/golden/cholera_siwr.ir.json`) is the
production reproduction: its `infection` rate has a water-borne term
`beta_W * W / (W + kappa)`. Forcing `beta_W = 0` mimics the bug exactly (it
zeroes the same term the bug zeroed by making `W ≡ 0`):

```
chain_binomial, dt=1, seed=42:
  t       fixed (W-coupled)      beta_W=0 (bug-equivalent)
  7        S=46  I=346            S=947 I=25      ← epidemic vs near-extinction
  364      S=51  I=81             S=460 I=22      ← endemic vs collapsed
```

The water-borne route drives the entire epidemic takeoff in this
parameterization; the bug silently removed it, producing a near-extinct outbreak
where the correct dynamics show a full epidemic.

## Root cause

`rust/crates/sim/src/chain_binomial.rs`, `step_one`:

- Integer sync (present): `scratch.int_s.counts.copy_from_slice(counts)`.
- Real sync (MISSING): there was no `scratch.real_s.values.copy_from_slice(...)`.

`scratch.real_s` is a `RealState::new(n_real)` allocated in `StepScratch::new`
— all zeros. Propensity evaluation reads it:

- `eval_propensities(model, &scratch.int_s, &scratch.real_s, ...)`
- `propensity.rs` (`Expr::Pop` / `Expr::PopSum` real branch):
  `ctx.real_s.values[local]` — returns 0 for every real compartment.

The run's own `real_s` *was* advanced correctly by `rk4_step(...)` before the
`step_one` call, but that advanced value never reached the scratch the rate
evaluator reads. Every other backend (Gillespie, tau-leap, ODE) reads the live
real state directly, so the bug was unique to chain-binomial — and chain-binomial
is the only inference kernel.

## Secondary bug found in the same spot: real-compartment interventions dropped

`step_one` calls `apply_post_advance(... &mut scratch.real_s ...)`, which can
mutate the real state via interventions (`apply_intervention` writes `real_s` at
`intervention.rs` for `set()` / `transfer()` / `add()` actions targeting a real
compartment). The mutated `scratch.real_s` was never copied back into the run's
`real_s` — only `scratch.int_s.counts` was. So real-compartment interventions on
the chain-binomial backend were silently no-ops. The fix copies the real state
back (see below); a dedicated regression for real-compartment interventions is
not yet written.

## Fix

Thread the run's real state through `step_one` and sync it in both directions:

```rust
// signature: add `real: &mut RealState`
scratch.real_s.values.copy_from_slice(&real.values);  // read: before propensity eval
...
real.values.copy_from_slice(&scratch.real_s.values);  // write: after apply_post_advance
```

Every `step_one` caller updated to pass the current real state:

- Forward run (`run_chain_binomial_with_observer`): passes its live `real_s`
  (advanced by `rk4_step` just above the call). **This is the path the fix
  corrects.**
- Inference callers (`ChainBinomialProcess::step`, PGAS `simulate_reference_on_grid`
  and `csmc_as`, `correlated_pf`): pass an explicit zeroed `RealState` (see
  §Inference scope — these never tracked real state and still don't; the change
  makes the zero *visible* rather than hidden inside `step_one`).
- Tests/benches (`if2`, `spatial_density`, `snapshot_projections`,
  `benches/scaling`): pass a `RealState` sized for the model.

For real-free models (`n_real == 0`) the synced `RealState` is empty and every
step is byte-identical to before.

## What it changes (goldens)

One trajectory baseline moves, and only one:

- `sir_reservoir_mixed / chain_binomial`:
  `0x0bacf4e75cfcb7fc → 0x0597d93ff326fb1b`
  (`rust/crates/sim/tests/gate_trajectory_baseline.rs`).

This model's infection rate is `beta * S * I / Total`, where the binding
`Total = pop_sum(S, I, R, W1, W2, W3, W4, W5)` includes the five real reservoirs
`W1..W5`. Under the bug, chain-binomial computed `Total = S + I + R` (reals
read as 0) — a too-small denominator and a too-high infection rate. The fix
includes the real reservoirs in `Total`, matching what ODE / Gillespie /
tau-leap already computed.

**Verification the new value is correct** (cross-backend, t=30, baseline params):

```
ode             S=705  Wsum=1030.2
gillespie       S=808  Wsum= 693.1
chain_binomial  S=782  Wsum= 750.1   ← now between ODE and Gillespie
```

All three agree that S depletes (~705–808 from 990) and the reservoirs grow
(~700–1030); the corrected chain-binomial sits sensibly between the deterministic
ODE and the exact-stochastic Gillespie, where a correct Euler-multinomial kernel
should land. Before the fix, chain-binomial was the outlier.

**Models that correctly did NOT move** (the control):

- `sir_reservoir` (non-mixed, real W) on chain-binomial stays byte-identical —
  its rate uses `N = pop_sum(S, I, R)`, integers only, so no integer rate
  couples to W. The bug never touched it; the fix correctly leaves it alone.
- All integer-only / real-decoupled models on all backends: byte-identical
  (141 of 142 baseline entries unchanged).
- `sir_reservoir_mixed` on ode / gillespie / tau_leap: byte-identical (those
  backends always read W correctly).

`cholera_siwr` has no committed chain-binomial trajectory baseline (it lives in
`ir/golden/`, which the trajectory-gate corpus — `ocaml/golden/` — does not
include; its only in-tree test, `golden_simulate::test_cholera_siwr_gillespie`,
exercises Gillespie and asserts invariants only). So no committed cholera golden
moved, but its chain-binomial output was nonetheless catastrophically wrong (see
reproduction above).

## Inference scope (NOT fixed here — separate, larger cycle)

The fix corrects forward simulation. The inference path is **doubly broken** for
real-coupled models and this fix does not repair it:

1. The particle state (`inference/types.rs::ParticleState`) carries `counts` and
   `flow_accumulators` only — there is **no real-compartment state anywhere in
   the particle filter**, and no code advances a real reservoir (no `rk4_step`)
   inside any inference loop. So even with `step_one` fixed, every inference
   caller passes a zeroed `RealState`; a real-coupled model is fit with its real
   compartments pinned at 0 (≡ their init for cholera SIWR).

2. The fit-side capability gate does **not** reject this. In
   `cli/src/fit/methods.rs::check_model_capabilities`, the `chain_binomial`
   backend is granted `Capabilities::REAL_COMPARTMENTS`, so a real-coupled model
   passes the gate and proceeds to inference, where it is silently mis-fit. The
   "use backend = ode" hint text for `REAL_COMPARTMENTS` is therefore dead for
   chain-binomial.

Consequence: fits of any real-coupled model (cholera SIWR, `sir_reservoir_mixed`,
any PDMP) on the chain-binomial kernel produce parameter estimates against
dynamics with the real reservoir frozen at its initial value. The likelihood,
the MLE (IF2), and the posterior (PGAS / PMMH) are all computed against the wrong
process. **This needs the maintainer's decision**: either (a) make the particle
state carry and RK4-advance the real reservoir (the real fix, a larger lift), or
(b) hard-gate real-coupled models out of chain-binomial inference (drop
`REAL_COMPARTMENTS` from the inference-side `chain_binomial` capability set) until
(a) lands.

## Tests

- `rust/crates/sim/tests/chain_binomial_real_state.rs` — RED-first regression
  (the reproduction above), now GREEN.
- `rust/crates/sim/tests/fixtures/real_coupled_rate.ir.json` — minimal fixture.
- `rust/crates/sim/tests/gate_trajectory_baseline.rs` — one baseline updated
  (`sir_reservoir_mixed / chain_binomial`), verified cross-backend.
