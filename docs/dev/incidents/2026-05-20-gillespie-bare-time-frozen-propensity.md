# Gillespie freezes propensities for rates that depend on bare `t`

Date: 2026-05-20 Severity: high (silent wrong dynamics; no error, no warning)
Backends affected: Gillespie only (chain-binomial / tau-leap / ODE correct)
Status: fixed

## What happened

A transition rate that depends on simulation time through a **bare time
reference** `t` — e.g. a smooth importation pulse
`lambda / (1 + exp(-(t - tau)/w))` — was simulated incorrectly on the Gillespie
backend. The rate's propensity was **frozen at its `t = 0` value** for the whole
run. No error or warning was emitted; the simulation completed and produced a
plausible-looking trajectory that was simply wrong.

Concretely, on a SIR model seeded by such a pulse (`seed : --> I @ ...`), with
`lambda = 2`, `w = 3`:

| backend                | τ=10 seed inflow | τ=40 seed inflow |
| ---------------------- | ---------------- | ---------------- |
| gillespie (before fix) | 10               | **0**            |
| gillespie (after fix)  | 189              | 141              |
| chain_binomial         | 213              | 166              |
| tau_leap               | 213              | 166              |

At τ=40 the pulse is essentially off at `t=0` (`rate(0) ≈ 3·10⁻⁶`), so the
frozen propensity stayed ≈0 for the entire run and **no seeding ever occurred —
no epidemic at all**, despite the rate rising to ≈2/day after t≈40. The failure
mode is worst exactly where it matters most for seed-timing inference: a late
introduction silently vanishes.

## How it was detected

While hand-verifying mechanism B of the seed-timing proposal
(`docs/dev/proposals/2026-05-20-seed-timing-inference.md`), comparing seed
inflow across backends. Gillespie disagreed sharply with the fixed-step backends
and produced zero inflow for a late seed. The per-substep tracer
(`CAMDL_TRACE_STEPS=1`) confirmed `rate_seed` _was_ computed correctly when
re-evaluated, isolating the fault to _which_ transitions get re-evaluated.

## Root cause

`CompiledModel::new` builds `time_dep_transitions` — the set of transitions
Gillespie re-evaluates at each output/intervention boundary, because the SSA
otherwise holds a transition's propensity constant between events. That set was
built by `expr_has_time_func`, which matched only `Expr::TimeFunc` (named
forcings such as `seasonal(t)`) and **not** `Expr::Time` (a bare `t`). So a rate
using bare `t` was omitted from `time_dep_transitions` and never re-evaluated as
time advanced; its propensity stayed at the `t=0` value.

The fixed-step backends (chain-binomial, tau-leap) re-evaluate _all_
propensities every substep regardless of this set, so they were unaffected —
which is why the bug hid: the blessed seasonal-forcing path uses `TimeFunc`, and
no golden model used bare `t` in a rate, so no test exercised the gap.

`rust/crates/sim/src/compiled_model.rs` — `expr_has_time_func` (the classifier),
consumed at the `time_dep_transitions` build site and in
`rust/crates/sim/src/gillespie.rs` (the boundary re-evaluation loop).

## Remediation

- Extended the classifier to treat `Expr::Time` as time-dependent and renamed it
  `expr_is_time_dependent` (the old name described only half of what the set
  must contain). `Expr::Dt` (the step-size constant) is deliberately _not_
  time-dependent.
- Added unit tests (`compiled_model::tests`) asserting bare `Time` — including
  nested under `BinOp`/`UnOp` as in a logistic pulse — classifies as
  time-dependent, and that a time-free rate does not.
- No golden files changed: no committed model used bare `t` in a rate, so the
  fix only newly-classifies a construct that had no prior test coverage.

## Residual behavior (not a bug, document it)

Gillespie re-evaluates time-dependent propensities only at **output /
intervention boundaries**, not continuously. For a time-varying rate this is a
piecewise-constant approximation on the output grid (the τ=10 gillespie inflow
189 vs the fixed-step 213 reflects this, not the freeze). For trustworthy
Gillespie dynamics under a sharply time-varying rate, use a fine output grid; or
prefer a fixed-step backend, which is in any case required when the seed feature
composes with lineage trees on overdispersed processes. Exact treatment of
time-inhomogeneous propensities (modified next-reaction / thinning) is future
work, already noted as a TODO in `gillespie.rs`.

## What it suggests

- The classifier was named for the case its author had in mind (`TimeFunc`),
  which masked a category it silently failed to cover (`Time`). Naming a
  predicate after the closure it computes ("is time-dependent") rather than one
  member of it would have made the gap visible at the call site.
- A backend-agreement test over a model with a bare-`t` rate would have caught
  this. The seed-timing recovery test (mechanism B) now exercises a bare-`t`
  rate on Gillespie end-to-end, closing the coverage gap.
