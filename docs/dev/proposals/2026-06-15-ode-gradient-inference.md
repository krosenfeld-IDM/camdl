---
status: proposal
date: 2026-06-15
target: five phases; ODE-MH + comparison lands in Phase 1 (the priority)
supersedes: docs/dev/proposals/archive/pre-alpha/2026-05-04-ode-inference-three-phase.md (Phase 1 shipped; rest superseded)
---

# State-of-the-art ODE inference: real-valued likelihood, MH, model comparison, and a shared gradient stack

## TL;DR

The ODE backend today supports only gradient-free maximum-likelihood inference
(`nl-sbplx`, `nl-bobyqa`). This proposal brings it to the state of the art and,
in the process, fixes a silent correctness bug that makes the current
deterministic likelihood wrong for low-rate models (e.g. TB latency).

The work is staged so the **near-term goal — trustworthy Bayesian posteriors on
ODE models plus the ability to compare them — lands in Phase 1**, before any of
the gradient machinery:

| Phase | Deliverable                                                              | Needs gradient? |
| ----- | ------------------------------------------------------------------------ | --------------- |
| 0     | Real-valued ODE flow (correctness fix; unblocks every ODE likelihood)    | —               |
| 1     | `mh` on `ode` + model comparison (WAIC / PSIS-LOO, typed like-with-like) | no              |
| 2     | The gradient spine: symbolic forward sensitivities                       | builds it       |
| 3     | Gradient-based MLE (`nl-lbfgs`, `nl-slsqp`)                              | yes             |
| 4     | `nuts` on `ode`                                                          | yes             |

Three principles:

1. **Symbolic gradients only.** Gradients are derived by source-to-source
   differentiation in the OCaml compiler and evaluated by the existing Rust
   `eval_resolved`. Finite differences appear **only** as the gradient-check
   test oracle (Phase 2) — never as a production path. No runtime autodiff, no
   numerical-difference fallback.
2. **Consolidate before the matrix can drift, but not ahead of the goal.** The
   two pieces of NUTS machinery currently inlined in PGAS (warmup/mass-matrix
   adaptation; the prior+transform+Jacobian posterior wrapper) get extracted —
   but they are NUTS-only and are sequenced in Phase 4, _not_ ahead of MH. MH
   reuses the existing `run_pmmh` driver and needs neither.
3. **Two statistical objects, named to the user and enforced by types.** A
   deterministic-likelihood method computes `p(y | θ, ODE skeleton)`; the
   particle-filter methods compute the stochastic `p(y | θ)`. They coincide in
   the low-noise / large-population limit but are different objects. Model
   comparison must never silently cross that line — the comparison artifact is
   typed so an ODE fit and a PF fit cannot be compared except through a shared
   out-of-sample predictive score.

## Phase 0 — Real-valued ODE flow (correctness prerequisite)

### The bug

The deterministic ODE backend accumulates transition flow in `f64` (rate × dt)
but rounds it to `u64` at **every output snapshot** and resets
(`rust/crates/sim/src/ode.rs:204,208-210,221,257`). `compute_ode_loglik` then
sums these already-rounded snapshot flows
(`rust/crates/cli/src/fit/runner.rs:830-834`), and the function's own comment
(`runner.rs:801-806`) documents that the output grid is fine (daily). The
result: for a model whose per-snapshot flow is sub-unit, every snapshot rounds
to 0, the accumulated incidence is 0, and the likelihood is `-∞`.

This is invisible at epidemic scale (typhoid daily flow ≈ 1370; rounding 1370.3
→ 1370 is nothing) and **fatal for TB latency**, where reactivation flow is a
tiny fraction per day: a 0.01/yr rate over a 10⁴ latent reservoir is ~0.27/day →
rounds to 0 → likelihood `-∞` across the entire slow-rate regime, the exact
region of interest. It bites the MH path (Phase 1) directly, silently, with no
error.

Root cause: `FlowVec { counts: Vec<u64> }` (`rust/crates/sim/src/state.rs:76`)
is a stochastic-backend type (integer event counts), and the deterministic ODE
path is forced to quantize its genuinely-continuous flow through it.

### The fix: a real-flow variant on the trajectory snapshot

Make the snapshot's flow representation a sum type so integer (stochastic) and
real (deterministic) flows cannot be confused:

```rust
// rust/crates/sim/src/state.rs
pub enum Flows {
    Int(Vec<u64>),    // Gillespie / chain-binomial: genuine event counts
    Real(Vec<f64>),   // ODE: continuous rate·dt accumulation, no rounding
}
// Snapshot.flows: FlowVec  →  Snapshot.flows: Flows
```

This is chosen over a parallel `flows_real` field deliberately: the bug _is_
"code read the rounded `u64` flow on an ODE trajectory," and a parallel field
leaves that footgun in place. The enum makes it **unrepresentable** — an ODE
snapshot has no integer flow field to misread, and any consumer wanting integers
must explicitly handle (or reject) the `Real` arm. Same memory as today (one
`Vec` either way + an 8-byte tag); a parallel field would be strictly heavier.

The PGAS latent flow (`SubstepRecord.flows: Vec<u64>`,
`rust/crates/sim/src/inference/pgas.rs:136`) is a **separate struct** feeding
the chain-binomial transition density, which is genuinely integer (binomial
draws). It is untouched — this fix changes only the trajectory snapshot.

`f64` represents every integer exactly to 2⁵³ ≈ 9.0×10¹⁵, and counts top out
around 10¹⁰, so all `u64`↔`f64` conversions are lossless. The distinction is
semantic, not precision: integer flows feed an integer density; real flows do
not. That is why we do not globally widen `FlowVec` to `f64`.

### Scoring path

The shared scoring seam takes `u64`
(`MultiStreamObsModel::fold_into_acc(&[u64], &mut [u64])`,
`log_likelihood_from_flows_and_counts(&[u64], …)`,
`rust/crates/sim/src/inference/multi_stream_obs.rs:896,947`; the projection
already converts `acc[k] as f64`). Add an `f64` accumulation path used by the
ODE loglik only — `fold_into_acc_real(&[f64], &mut [f64])` plus a real-`acc`
scoring entry. The stochastic `u64` path (PF/IF2/PGAS) is unchanged → zero risk
to the existing samplers.

### Blast radius and gates

`Snapshot.flows` has ~3 production consumers (`compute_ode_loglik`, the TSV
trajectory writers in `cli/src/main.rs` and `cli/src/util.rs`) and ~10 tests;
all gain an explicit match on `Flows`. **Golden impact:** ODE trajectory TSV
flow columns will print real values (`0.27`) instead of rounded (`0`), so
`ir/expected/*.tsv` for ODE models changes — a deliberate, reviewed golden
update (the rounded output was the bug), surfaced explicitly in the commit per
the golden-review rule.

TDD gate: a low-rate model (TB-scale reactivation) whose `compute_ode_loglik`
returns `-∞` today → write the failing test, confirm red, apply the fix, confirm
green; then `make test` + `make update-expected` with the golden diff reviewed.

## Phase 1 — `mh` on `ode` + model comparison (the priority)

### MH via the existing `run_pmmh` driver

`run_pmmh` is already closure-based over the likelihood —
`eval_loglik: &dyn Fn(&[f64], u64) -> f64`
(`rust/crates/sim/src/inference/pmmh.rs:263`); its docstring states it "doesn't
need to know how the PF is constructed." Deterministic MH-on-ODE is then:

```rust
run_pmmh(if2_params, priors, …, observations,
         &|p, _seed| eval_ode_loglik(&compiled, &obs_model, &obs_times, dt, p),
         /* eval_loglik_correlated */ None, seed, …)
```

A deterministic (zero-variance) likelihood is a valid special case of the
unbiased estimator pseudo-marginal MH expects, so this reduces to ordinary
Metropolis-Hastings — theoretically clean. It inherits, already correct and
tested, z-space proposals, the change-of-variables Jacobian, prior handling
(including hierarchical), MAP tracking, resume, and R̂/ESS. The deterministic
eval closure already exists (`rust/crates/cli/src/fit/nlopt_stage.rs:418`).

**One correctness fix:** `AdaptiveProposal` (`pmmh.rs:117-229`) re-estimates its
Cholesky every 100 steps indefinitely, with no termination. On a stochastic PF
target the bias is dominated by PF noise; on an exact deterministic target,
standing adaptation breaks π-invariance (adaptive MCMC is ergodic only under
diminishing adaptation or freeze-after-warmup; Roberts & Rosenthal 2007). Add a
`freeze()` (mirroring the one specified for `NutsWarmup` in Phase 4) and stop
adapting after burn-in.

Files: `cli/src/fit/mh_stage.rs` (new runner), `("mh","ode")` in
`methods.rs::METHODS` (`rust/crates/cli/src/fit/methods.rs:67`) + MH knobs on
`Stage`, dispatch arm, remove the generic rejection. The from-scratch
`mh_det.rs` / `adaptive_metropolis.rs` extraction the earlier plan called for is
**not** on the critical path — reuse `run_pmmh` directly; extract later only if
a second consumer justifies it.

### Error boundary

`eval_ode_loglik` returns `Result<f64, SimError>` and **classifies** errors at
the parameter-eval boundary rather than coercing every `Err` to `-∞`.
`compute_ode_loglik` already returns `Ok(-∞)` for the common bad-θ case (a
non-finite likelihood term, `runner.rs:852`) and reserves `Err` for structural
failures (missing snapshot, early termination, `runner.rs:867,879`): a numeric
overflow at an exploratory θ is a recoverable rejection (`-∞`); a misconfigured
model surfaces loud. Route both ODE eval closures through the shared
`SimError::is_structural()` discriminator (`rust/crates/sim/src/error.rs`) —
structural → propagate `Err`, per-θ excursion → `-∞`. The existing ODE-MLE eval
(`nlopt_stage.rs:418`) currently blanket-coerces
(`compute_ode_loglik(…).unwrap_or(-∞)`) and gets the same treatment.

### dt-convergence check on the ODE path

The deterministic stage currently sets `dt_check: None` (`nlopt_stage.rs:269`,
with a comment admitting it is deferred), so a user gets no warning if a coarse
`dt` under-resolved a stiff model — a real risk for TB (see Caveats). Wire a
deterministic Richardson check: re-run `compute_ode_loglik` at θ̂ on a
`dt`-halving ladder and warn if the loglik moves more than a small tolerance.
The verdict logic in `dt_check.rs` is backend-agnostic; only its runner is
PF-bound today.

### Model comparison

Posterior samples are necessary but not sufficient to _compare_ models. Today
the only comparison surface, `camdl compare`, reads `prequential.json`, which is
emitted only by the PFilter stage (`record_prequential` is a PF-only `SMCConfig`
field, `rust/crates/sim/src/inference/traits.rs:255`); there is no WAIC / LOO /
marginal likelihood anywhere. Phase 1 adds an ODE-posterior comparison path and
a type that makes invalid comparisons impossible.

**Predictive quantities, by inference class.** The comparison metric is always a
flavour of expected log predictive density (elpd), but the predictive
construction differs by method, and mixing constructions is invalid:

| Fit class          | Predictive construction                                            | Comparison quantity                 |
| ------------------ | ------------------------------------------------------------------ | ----------------------------------- |
| PF (stochastic)    | filtering one-step-ahead `p(yₜ\|y₁:ₜ₋₁)`, integrates process noise | filtering elpd (`prequential.json`) |
| ODE-MLE (point)    | plug-in `p(yₜ\|xₜ(θ̂))`                                             | AIC, BIC                            |
| ODE-MH (posterior) | posterior pointwise `lppdₜ = log meanₛ p(yₜ\|xₜ(θₛ))`              | **WAIC, PSIS-LOO**                  |

These are different statistical objects: the PF scores the stochastic likelihood
through a sequential filtering predictive; the ODE scores the deterministic
skeleton through a plug-in or posterior predictive. Their elpds are not
comparable even though both "look like" predictive densities.

**Phase 1 builds the ODE-MH path: WAIC and PSIS-LOO** (Watanabe 2010; Vehtari,
Gelman & Gabry 2017). Both consume the same artifact — a `[S draws × n obs]`
matrix of pointwise `log p(yₜ | xₜ(θₛ))`, i.e. `compute_ode_loglik`'s per-obs
term (`runner.rs:845`) evaluated at each posterior draw. From that matrix, WAIC
and PSIS-LOO (with per-obs Pareto-k̂ diagnostics) are pure post-processing.

**The typed like-with-like enforcement.** Each fit emits a basis-tagged
artifact; `camdl compare` refuses to compare mismatched bases:

```rust
enum ModelComparisonInput {
    // method-specific, in-sample — compare only within the same variant
    OdePosterior { train_hash: Hash, log_lik: Vec<Vec<f64>> },   // WAIC / PSIS-LOO
    OdeMlePlugin { train_hash: Hash, total_loglik: f64, n_params: usize, n_obs: usize },
    PfFiltering  { train_hash: Hash, per_obs_elpd: Vec<f64>, crps: Vec<f64>, pit: Vec<f64> },

    // universal, out-of-sample — compare ACROSS methods iff the test set matches
    HeldOutPredictive { test_hash: Hash, per_point_lpd: Vec<f64> },
}
```

The comparison rule:

```rust
fn comparable(a, b) -> Result<(), CompareError> {
    match (a, b) {
        // cross-method allowed iff scored on the same held-out test set
        (HeldOutPredictive{test_hash: h1, ..}, HeldOutPredictive{test_hash: h2, ..})
            if h1 == h2 => Ok(()),
        // in-sample: same variant AND same training data only
        (OdePosterior{train_hash: t1, ..}, OdePosterior{train_hash: t2, ..}) if t1 == t2 => Ok(()),
        (OdeMlePlugin{train_hash: t1, ..}, OdeMlePlugin{train_hash: t2, ..}) if t1 == t2 => Ok(()),
        (PfFiltering {train_hash: t1, ..}, PfFiltering {train_hash: t2, ..}) if t1 == t2 => Ok(()),
        _ => Err(CompareError::IncommensurableBases { a: a.basis(), b: b.basis() }),
    }
}
```

The variant mismatch _is_ the type incompatibility: an ODE-posterior fit and a
PF-filtering fit can never be compared on their in-sample criteria, because they
score different predictive objects. The data hashes ensure comparison is
conditional on identical data (camdl already has run-id/IR hashing infra).

**The universal escape hatch.** Out-of-sample log predictive density on a
_shared held-out test set_ is the one comparison that is method-agnostic: any
fitted model — PF, ODE-MLE, ODE-MH, PGAS — can produce "my log predictive
density at these held-out points," and scoring genuinely-unseen data with a
proper scoring rule (Gneiting & Raftery 2007) is a legitimate cross-method
comparison even when the predictive machinery differs. The `HeldOutPredictive`
variant reserves that seat: it is the only one the rule allows to compare across
methods, keyed on `test_hash`. **Phase 1 implements only `OdePosterior`
(WAIC/PSIS-LOO);** the held-out path is designed-in but built later. Adding it
is a new emitter per method plus the already-present cross-variant rule — no
retrofit.

### Phase 1 gates

MH MAP agrees with the `nl-sbplx` MLE under flat priors; on a worked TB latency
case the posterior is proper and R̂/ESS healthy; `camdl compare` returns a ΔWAIC
(and ΔLOO with Pareto-k̂ warnings) between two TB model structures, and
**refuses** to compare an ODE-MH artifact against a PF artifact with a clear
error. A low-count rounded-vs-continuous check (Phase 0 fixed flow; confirm a
low-count TB-shaped posterior is stable) before declaring MH trustworthy.

## Phase 2 — The gradient spine (symbolic forward sensitivities)

NUTS (Phase 4) and gradient-MLE (Phase 3) both need `∇_θ log p(y | θ)`. This
phase builds it once. No user-facing method ships here.

### The chain rule — per temporal kind

For a deterministic ODE, the gradient depends on what the observation _is_:

- **Prevalence (Instant)** — the observed quantity is the compartment state
  `x_t`. Then `∇_θ log p = Σ_t (∂ log p(y_t|x_t)/∂x_t)ᵀ S(t)`, where
  `S(t) = ∂x(t)/∂θ`.
- **Incidence (Interval)** — the observed quantity is an **accumulated-and-reset
  flow** over the obs interval, `accₖ = ∫ stochₖ·rate(x(s),θ) ds`. Its
  sensitivity is a _different object_:
  `∂accₖ/∂θ = ∫ (J_{θ,k} + J_{x,k} S(s)) ds`, integrated over the interval and
  **reset on the same per-stream schedule** as `reset_due_acc`
  (`multi_stream_obs.rs:908`). Chaining the obs score against `S(t)` here would
  be silently wrong.

camdl already distinguishes these via `TemporalKind::{Interval, Instant}`; the
gradient assembly must carry **two** sensitivity accumulators — `S(t)` for
Instant streams, and a per-Interval-slot `∂acc/∂θ` that integrates `J_θ + J_x S`
and resets per stream. The gradient-check oracle (below) **must** include an
incidence stream with ≥2 reset intervals — a prevalence-only check passes with
the wrong formula (the gh#187 silent-gap class).

The obs-score factor `∂ log p(y_t|x_t)/∂x_t` is built from
`∂ log p/∂μ · ∂μ/∂projected · ∂projected/∂(state or acc)`. Only the first factor
exists today (`obs_loglik.rs:98-215`, the score w.r.t. the distribution mean);
the obs path currently treats `projected` as constant (`obs_model.rs:150-156`
documents `∂projected/∂θ` is dropped). The remaining factors are new.

### Forward sensitivities — shared-stage RK4

`S` solves `Ṡ = J_x S + J_θ` alongside `ẋ = f(x,θ)`, with `f = stoich·rates`,
`J_x = stoich·∂rates/∂x ∈ ℝⁿˣⁿ`, `J_θ = stoich·∂rates/∂θ ∈ ℝⁿˣᵈ` (`n`
compartments, `d` estimated params). The augmented `(x, S)` system must be
integrated with a **single RK4 whose four stages share** the same intermediate
states: `J_x`, `J_θ`, and the `S`-stages advance in lockstep with the state
stages (`rust/crates/sim/src/ode.rs:99-150`). A decoupled "state step, then a
separate S step at the endpoint" is only first-order in `S` → the gradient is
`O(dt)`-inconsistent with the `O(dt⁴)` value, degrading NUTS Hamiltonian
conservation and producing spurious divergences. `OdeSim` is already generic
over a `Vec<f64>` state, so the augmented system needs no type surgery, only a
larger allocation. Forward mode is `O(d)`; adjoint (`O(1)` in `d`) is deferred —
see Caveats.

### `state_grad` from the compiler — generalize the differentiation target

`autodiff.ml` differentiates a rate expression w.r.t. a named parameter:
`differentiate : expr -> string -> … -> deriv` (the target is a bare param
name). `J_θ` already exists as the emitted `rate_grad`. `J_x` needs
`∂rate/∂Pop(Cₖ)` — a new `state_grad` field. Generalize the target:

```ocaml
type diff_target =
  | WrtParam of string   (* existing rate_grad *)
  | WrtPop   of string   (* new state_grad; compartments are referenced by name *)
```

This is not a one-line toggle. The base case at `autodiff.ml:145`
(`Const | Pop | PopSum | Time | Dt | Projected | … -> Const 0.0`) is **fused**
and must be un-fused: for `WrtPop`, `Pop name → [name = target]`,
`PopSum members → [target ∈ members]` (the FOI/coupling terms — the source of
off-diagonal `J_x`), rest → 0; mirrored in `mentions`. The real cost is
**`BindingRef`**: bindings are hoisted state-only subexpressions, today zeroed
and asserted param-free (`autodiff.ml:66`; the invariant is enforced at
`validate.ml` / `expr_analysis.ml` E512). For `WrtPop` the premise inverts —
bindings are functions of state, so `∂binding/∂x` is generally nonzero. `WrtPop`
must thread the model binding table into `differentiate`, resolve-and-recurse
through binding bodies with cycle protection (none exists today), and reconcile
the param-free invariant so it holds for `WrtParam` but not `WrtPop`. Forcings
and tables remain state-free (`∂/∂x = 0`), so `WrtPop` is simpler there.

### IR schema change (atomic)

`state_grad` is a new per-transition IR field. The version guard hard-rejects
any version mismatch (`rust/crates/ir/src/envelope.rs:69-78`; `ir/VERSION` is
`0.14`), so there is **no** backward-compat path: bumping the version means
every golden regenerates atomically, and old IR no longer loads. The "model
lacks state_grad → no gradient method" gate is therefore a **dispatch-time
capability check** on the current-version model (extend `coeff_guard`, which
already lives in `cli/fit` and already refuses NUTS fits whose gradient depends
on an undifferentiated coefficient), **not** deserialization tolerance.
Procedure: `ir/schema.json` + `ir/VERSION` bump → OCaml types → Rust
`rust/crates/ir/src/transition.rs` → `make test-unit` →
`make update-golden && make update-expected` → one atomic commit with the golden
diff reviewed.

### Crate relocation

`compute_ode_loglik` lives in `cli` (`rust/crates/cli/src/fit/runner.rs:776`)
but has zero `cli` dependencies — it references only `sim` types and the `sim`
scoring seam. The gradient assembler (`det_grad`) must live in `sim`, so Phase 2
begins by moving `compute_ode_loglik` down into `sim` and repointing its ~3
`cli` callers. (Aside: CLAUDE.md's "cli → io → observe → sim → ir" is stale —
there is no `observe` crate; real layering is `cli → io → sim → ir`.)

### Continuous obs evaluation (state) and likelihood notes

Phase 0 made incidence flow real-valued. Prevalence still scores through rounded
`i64` counts (`runner.rs:847`, `to_states` rounding `ode.rs:155`); for gradient
smoothness (and for low-count prevalence-observed TB) the prevalence projection
must read the `f64` `int_vals` via the existing `EvalCtx.int_float_override`
pattern (`ode.rs:64`) — a `log_likelihood_continuous(real_counts: &[f64], …)`
entry. Two distribution notes: camdl's `Normal` is the **discretized-count**
likelihood (Φ-difference, `obs_model.rs:85`), smooth in `μ` (which is what we
need) but not a continuous PDF — the gradient is fine, the framing must be
accurate. And `Binomial`/`BetaBinomial` round the denominator `n` to `u64` and
treat it constant w.r.t. θ (`obs_model.rs:214,231`); if `n` is state-derived
(e.g. `n = S+I+R`), a rounded `n` cannot yield a smooth gradient — add a
`coeff_guard` rejection of state-dependent binomial denominators under gradient
methods rather than silently dropping the term.

### Gradient-check oracle (the only FD in the system)

Extend `gradient_check.rs`: assert `‖∇_symbolic − ∇_FD‖_∞ < 10⁻⁴` across all
estimated params, on a model with an incidence stream over ≥2 reset intervals
(exercises the per-Interval sensitivity and reset), a hoisted binding used in a
rate (exercises `BindingRef` state-diff), and a parameter-dependent event
(exercises the sensitivity jump). Red-then-green: write against a deliberately
wrong `J_x` first, confirm failure, then land the correct emission. FD lives
here and nowhere else.

## Phase 3 — Gradient-based MLE (`nl-lbfgs`, `nl-slsqp`)

With the gradient available, stop discarding the NLopt gradient slice. The
public optimizer seam is currently scalar-only —
`optimize_det<F> where F: FnMut(&[f64])
-> f64`
(`rust/crates/sim/src/inference/deterministic.rs:128`) — with no channel for the
caller to return a gradient (the internal callback receives `_grad` and ignores
it, `deterministic.rs:169`). Phase 3 changes the seam to
`FnMut(&[f64], Option<&mut [f64]>) -> f64` (mirroring nlopt's own `ObjFn`,
`nlopt-0.8.1/src/lib.rs:190-201`), widens the `UserData`/callback bounds, and
has the caller fill the slice from `det_grad`. The `NloptAlgorithm` enum
(`deterministic.rs:27`) gains `Lbfgs` and `Slsqp` (`Algorithm::Lbfgs`/`Slsqp`,
both `LD_*`, `lib.rs:60-79`); `nl-slsqp` for box + nonlinear constraints,
`nl-lbfgs` for plain box-constrained smooth MLE. Registry entries
`("nl-lbfgs","ode")`, `("nl-slsqp","ode")`.

A/B experiment (answers "how much does the gradient help?"): on the same
model/data/start, run `nl-sbplx` (gradient-free) vs `nl-lbfgs` (gradient-based)
and report #objective evals to convergence, wall-clock, final loglik, basin. A
diagnostic env var `CAMDL_NLOPT_GRAD_REPORT=1` emits per-eval gradient norm +
eval count. There is **no** env var that substitutes a finite-difference
gradient. Expectation (to test, not assert): gradient-based wins grow with
dimension; at `d ≲ 5` Sbplx is competitive and more robust to rough objectives.

## Phase 4 — `nuts` on `ode`

Gradient-based Bayesian posteriors on the smooth deterministic likelihood.
Statistically simpler than the existing NUTS-in-PGAS — it samples
`p(θ|y) ∝ p(y|θ,ODE) π(θ)` directly: no CSMC, no discrete-event approximation,
no Gibbs-sweep coupling.

`nuts::nuts_step` (`rust/crates/sim/src/inference/nuts.rs:187`) is already
model-agnostic — it takes `&dyn Fn(&[f64]) -> (f64, Vec<f64>)` and contains no
PGAS-specific code. The two reusable pieces PGAS currently inlines are extracted
**here** (their only second consumer):

- `nuts_warmup.rs` — the dual-averaging + Welford mass-matrix adaptation block
  (`pgas.rs:2209-2283`). It is per-rung state entangled with parallel tempering
  (`rungs[rung].*`, `rung == 0` cold-chain prints,
  `mass_adapt_end = 0.7·burn_in`); the extracted `NutsWarmup` is instantiated
  per-rung and parameterized on cold-rung identity. Hard gate: **PGAS posteriors
  byte-identical** before/after, including the f64 accumulation order.
- `posterior_target.rs` — the prior + transform + Jacobian wrapper
  (`pgas.rs:2131-2171`). β-tempering and the data-term must be lifted out so the
  wrapper takes a pluggable θ-space data-term; PGAS plugs in
  `complete_data_loglik_grad`, ODE-NUTS plugs in `det_grad`. Same byte-identity
  gate.

Then `nuts_stage.rs` is a thin driver: `build_nuts_target(det_grad, …)` →
`NutsWarmup` over a contiguous warmup window → sample with `nuts_step`. Registry
entry `("nuts","ode")` + NUTS knobs. Gate: NUTS posterior agrees with an
**independent** high-ESS reference (not merely with the Phase-1 MH chain, which
on a ridge may not have converged — see Caveats) and with PGAS in the low-noise
regime; divergence rate low and not clustered at integer boundaries.

## TB latency: fitness and caveats

- **Identifiability ridges.** TB latency is the canonical weakly-identified
  structure: from incidence alone, slow-progression (large latent pool, low
  reactivation) trades off against fast-progression-then-relapse along a curved
  likelihood ridge. Random-walk / adaptive-covariance MH mixes through _curved_
  ridges poorly (adaptive covariance corrects a rotated Gaussian, not a banana).
  Phase-1 MH will _diagnose_ the ridge (low ESS) and is fine for identifiable or
  simple structures, but real TB comparison may hit mixing walls that need NUTS
  (Phase 4) or reparameterization (sampling the trade-off as a log-ratio). The
  Phase-4 NUTS gate is therefore against an independent reference, not the MH
  chain.
- **Stiffness and equilibration.** TB has extreme timescale separation (fast
  infection/recovery O(0.1–1)/day vs reactivation O(10⁻⁶–10⁻⁴)/day; stiffness
  ratio ~10⁵). Explicit RK4 is _stable_ for sub-unit/day rates (no blow-up), so
  "RK4 is fine" holds for stability — but equilibrating a TB model to endemic
  equilibrium via a long pre-data span costs ~10⁴–10⁵ RK4 steps per likelihood
  evaluation, and MH needs many evaluations. The Phase-1 dt-check guards against
  silent under-resolution; a true stiff (implicit) solver is the right long-term
  answer for TB and is the first deferred item, not a generic nicety. For the
  near-term goal, recommend modest models or steady-state initialization.
- **Low-count regimes.** TB comparison data is often low-count (annual
  notifications in the tens). Phase 0 fixes incidence flow; confirm posteriors
  are stable under continuous vs rounded eval at low counts before trusting
  them, and pull continuous prevalence eval (Phase 2 item) forward if prevalence
  is observed at low counts.

## Out of scope (v1)

- **Adjoint-mode sensitivities.** Forward mode is `O(d)`; adjoint is `O(1)` in
  `d` but needs a backward solve + checkpointing. For `d ≲ 30` forward is the
  simpler correct choice. Flag for hierarchical TB (shared hyperparameters
  across many strata, `d` in the hundreds): forward mode becomes prohibitive and
  adjoint is mandatory.
- **Stiff ODE solvers.** The right long-term answer for TB; deferred behind the
  near-term goal.
- **Reactive interventions under gradient methods** (parameter-dependent event
  times via an implicit condition). Gated out via `coeff_guard`, not
  mis-handled.
- **Cross-method held-out comparison.** The `HeldOutPredictive` type seat is
  designed-in; the per-method emitters are built when needed.
- **`--method auto`.** Algorithm choice between deterministic and stochastic
  likelihoods is too high-stakes for silent selection.

## Risks

- **Continuous-eval regressions.** Enabling the state-derivative arm of the obs
  gradient must not perturb the existing PGAS gradient (which relies on it being
  zeroed); distinct entry points, gated by a PGAS byte-identity test.
- **`state_grad` for `BindingRef`** is the subtlest part of the autodiff change;
  the gradient-check on a model using a binding in a rate is the gate.
- **Incidence sensitivity reset** is the subtlest part of the gradient assembly;
  the gradient-check on an incidence stream with ≥2 reset intervals is the gate.
- **NLopt C-FFI on Windows** (Phase 3) — Linux/macOS-arm64 verified; the `ode`
  feature gates the dependency if Windows breaks.
