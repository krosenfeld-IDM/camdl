# Deterministic ODE integration: fixed RK4, augmented flow, adaptive DOPRI5

Date: 2026-06-16
Project: camdl
Tags: ode, integrator, rk45, dopri5, incidence, numerics, gh#166

A reference for the deterministic backend's numerics — what it integrates, how,
and why the defaults are what they are. Written for someone arriving at the ODE
methods cold.

## What the ODE backend is

A compartmental model is, in general, a continuous-time Markov jump process: at
any instant each transition fires with a propensity (rate) that depends on the
current state, parameters, and time. The stochastic backends (Gillespie,
chain-binomial) simulate that process directly. The **ODE backend integrates its
mean-field / large-population limit** — the deterministic skeleton `dX/dt =
Σ_transitions stoichiometry · rate`. It is the right tool when N is large enough
that demographic stochasticity is negligible, and it is the engine behind the
deterministic-MLE / MAP inference path (`compute_ode_loglik`), where one ODE
solve replaces a particle filter.

camdl's ODE backend is technically a **PDMP** (piecewise-deterministic Markov
process): compartments may be integer-typed (read at their full f64 value
between snapshots — see *de-quantization* below) or real-typed with explicit
`dC/dt` equations, and scheduled interventions/events apply discrete jumps at
exact boundary times.

## The two quantities: prevalence and incidence

Every snapshot reports two things, and they are integrated differently:

- **Prevalence** — the compartment levels `X(t)` (how many S, I, R right now).
- **Incidence** — the per-interval *flow* through each transition since the last
  output (e.g. new infections this week). This is the quantity a likelihood
  scores against case data.

Incidence is implemented as an **augmented state variable**: alongside each
compartment we carry `c_i` with `dc_i/dt = rate_i(X(t), t)`, integrated by the
*same* stepper as the compartments and reset to 0 at each output boundary. This
is the textbook quadrature-by-augmentation trick (SUNDIALS/CVODES, Hindmarsh et
al. 2005; R `deSolve`), and in epidemiology specifically it is pomp's
**accumulator variable** idiom — a cumulative-incidence compartment zeroed at
each observation (King, Nguyen & Ionides 2016, *J. Stat. Soft.* 69(12) §2.1).
Because nothing in `dX/dt` reads `c_i`, augmenting the integrated vector does not
perturb the compartment trajectory: **prevalence is independent of the flow
accounting.**

## Two integrators behind one seam

`run_ode` is integrator-agnostic. It walks the merged timeline (the `Schedule`
spine: output times, intervention/event boundaries) and, for each
`[boundary, next_boundary)` interval, hands the *raw distance to the next
boundary* to an `OdeStepper` and re-enters until the boundary is reached:

```rust
trait OdeStepper { fn advance(&mut self, …, t, h_max, &mut OdeState) -> Result<f64 /*h_taken*/>; }
struct OdeState { int: Vec<f64>, real: Vec<f64>, flow: Vec<f64> }
```

- **`Rk4Fixed`** (default) takes `min(dt, h_max)` per call — the user's fixed
  step, clipped to land exactly on each boundary. Bit-identical to the
  pre-seam driver.
- **`Dopri5`** (opt-in) takes an adaptive step `≤ h_max`, re-entered. Its
  controller's natural step is clipped to the boundary so SIA pulses and outputs
  still land exactly.

The seam means "support both integrators" is a shared substrate, not a fork: the
driver, boundary handling, effect application, and output emission are written
once. (`rust/crates/sim/src/ode.rs`.)

## The previous method, and why it changed (the augmented-flow unification)

Before this work the ODE backend integrated **state** with RK4 (O(dt⁴)) but
accumulated **incidence** with a separate first-order **Euler** rule —
`flow_acc += rate(t)·dt`, a left-rectangle quadrature evaluated at the
start-of-step (rounded) state, via a standalone propensity evaluation distinct
from the RK stages. So prevalence converged at O(dt⁴) while incidence converged
at O(dt): a silent accuracy asymmetry (prevalence looks converged while the
incidence that drives the likelihood lags), and two flow code paths.

The fix unifies both onto augmented flow: `dc_i/dt = rate_i` rides through the
RK4 stages, reusing the stage propensities the integrator already computes
(dropping a 5th propensity eval per step, **5 → 4**). It is simultaneously *more
accurate, one mechanism, and slightly faster*. The cost was a one-time, reviewed,
oracle-validated movement of the ODE trajectory goldens (the trajectory hash
mixes flows; prevalence hashes are byte-identical — proven by a state-only gate).

**Exception — `Expr::Dt` rates.** A rate may reference the runtime step size
`dt` (gh#54) — the discrete-time discretization-correction idiom, e.g. pomp's
`(1 − exp(−λ·dt))/dt`. Augmented flow has no single `dt` to thread through the
stages, so such models (the `RUNTIME_DT` capability) **keep the Euler flow** on
fixed RK4, and are **rejected outright on rk45** (adaptive stepping has no fixed
step). Note this construct is a discrete-time artifact: on the continuous ODE
backend it is the dt→0 limit, where the correction vanishes — so it is a fixed-
step-only special case by nature, surfaced with a loud warning rather than
silently treated.

## Adaptive DOPRI5

`Dopri5` is the classic **Dormand–Prince RK4(5)** (Dormand & Prince 1980,
*J. Comp. Appl. Math.* 6(1):19–26; Hairer, Nørsett & Wanner 1993, *Solving ODEs
I*, Table 5.2):

- **7-stage explicit RK** with an embedded 4th-order solution. The local error
  estimate is `y5 − y4` (the b − b̂ weights are derived in code, not hand-
  transcribed, to avoid sign/typo errors). The tableau is consistency-verified
  (every a-row sums to its c; b and b̂ each sum to 1; FSAL).
- **PI step-size controller** (Gustafsson): `h_new = h · safety · err^(−α) ·
  err_prev^(β)` with `safety = 0.9`, `α = 0.7/5`, `β = 0.4/5`, step growth capped
  at 5× and shrink at 0.2× per step; `H_MIN = 1e-10 · span` underflow guard;
  max 10 rejections before a hard error. These are the standard HNW §II.4
  defaults; they affect *efficiency* (the step sequence), never *correctness*
  (the error control bounds accuracy regardless of the gains).
- **Flows ride along** as augmented state through the 7 stages (5th-order, not
  error-controlled — a quadrature whose accuracy follows the state step).
- **Stiffness:** TB latency (per-decade reactivation vs fast progression) and SIA
  pulses create timescale separation. Explicit DOPRI5 copes by shrinking steps
  in stiff regions (partial loss of speedup, never wrong). A genuine stiff/
  implicit method (Rosenbrock/BDF) needs the state Jacobian ∂(dX/dt)/∂X, which
  the symbolic-diff engine does not emit today (it differentiates wrt parameters
  only) — a contained Phase-2 extension, gated on a model DOPRI5 actually chokes
  on.

Fixed RK4 stays the **default**: it is the byte-identical golden reference, the
only integrator for `Expr::Dt` models, and inspectable (a fixed step, not an
adaptive controller's sequence). rk45 is opt-in via
`simulate { integrator = rk45 { atol = …, rtol = … } }`.

A useful side effect surfaced here: the ODE RK4 stages now run under the
model's **binding cache** (each shared binding — N_p, spatial FOI, … — evaluated
once per stage, not per `BindingRef`). On a spatial-FOI model this took the ODE
path from 0 → ~90k cache hits/run — the lever that matters for coupled
national-scale fits.

## Default tolerances (atol = 1e-8, rtol = 1e-6) and their justification

**What other packages default to** (general-purpose ODE solving):

| Package | rtol | atol |
| ------- | ---- | ---- |
| scipy `solve_ivp` (RK45/LSODA) | 1e-3 | 1e-6 |
| MATLAB `ode45`, Julia DiffEq | 1e-3 | 1e-6 |
| R `deSolve` (lsoda) | 1e-6 | 1e-6 |
| SUNDIALS CVODE | *(user sets; manual suggests ~1e-4)* | |

The general-purpose default (1e-3/1e-6) is calibrated for trajectory accuracy,
not likelihood fidelity. For inference, the relevant question is how much the
integrator perturbs the **loglik** a fit scores. Treating fine-dt fixed RK4 as
ground truth and scoring its incidence under rk45-at-tolerance (Poisson loglik
proxy), the integration-induced |Δ loglik| across the canonical models:

| model | scipy 1e-3/1e-6 | deSolve 1e-6/1e-6 | **default 1e-6/1e-8** | tight 1e-8/1e-10 |
| ----- | --------------- | ----------------- | --------------------- | ---------------- |
| SIR   | 4.9e-7 | 3.5e-8 | 2.6e-8 | 7.6e-10 |
| SEIR  | 4.8e-5 | 2.1e-7 | 9.1e-8 | 6.9e-10 |
| TB    | 5.1e-8 | 5.1e-8 | 5.1e-8 | 2.5e-9 |

Every candidate is **sub-nat by 4–9 orders of magnitude** — even the loose
ecosystem default. So tolerance choice is nowhere near threatening a fit, and no
model wants tighter than the default. We adopt **atol = 1e-8, rtol = 1e-6**: a
deliberate margin *tighter* than scipy's general-purpose default (cheap
insurance for models larger/stiffer than this trio — e.g. the 23-patch cVDPV2),
without overtuning to any one model. (The sweep is an `#[ignore]` calibration
test, `rk45_tolerance_calibration.rs`, so the evidence is reproducible in-repo.)

## Validation

The adaptive work moves incidence numbers, so correctness is a first-class
deliverable, validated three ways:

1. **Analytic** — a `rate = β·time` model where `∫ = β·T²/2` is closed form: RK4
   integrates the linear integrand exactly (augmented flow matches to machine
   precision), where the old Euler rule is O(dt)-wrong.
2. **External oracle** — camdl vs **scipy `solve_ivp` (RK45 + LSODA)** *and* R
   **`deSolve::lsoda`** on SIR / SEIR / 2-stage-latency TB, for both prevalence
   and incidence at the output grid. Both fixed RK4 (fine dt) and adaptive rk45
   (tight tol) match all three references — incidence to ≪0.1% of tolerance,
   prevalence to within the ±0.5 integer-snapshot rounding. Cached fixtures, so
   CI needs neither Python nor R (`tests/external/ode_oracle/`).
3. **Internal agreement** — fixed RK4 (fine dt) vs rk45 (tight tol) agree on
   prevalence and incidence; same `(model, θ, atol, rtol)` is byte-identical
   across runs (`rk45_agreement.rs`).

## Engineering details / gotchas

- **De-quantization** (the reason ODE beats a naive integer integrator at small
  N): integer compartments are read at their full f64 value during substeps via
  `int_float_override`; rounding to i64 happens only when snapshotting. The naive
  alternative quantizes state every substep, producing O(1/N) relative error and
  premature extinction.
- **Snapshot rounding** is therefore the *only* source of prevalence-vs-oracle
  discrepancy (≤ ±0.5); incidence (a `Flows::Real`, never rounded) tracks the
  oracle to integrator precision — a sub-unit flow (slow TB reactivation)
  survives instead of quantizing to 0 → −∞.
- **rk45 ≠ rk4 byte-for-byte**, so it is opt-in: making it the default would
  move every ODE golden (a second migration). Criteria for a future default
  flip: broad RK4-vs-rk45 agreement across the corpus, a soak, and softening the
  `Expr::Dt` hard-error to a warn+fallback.
- **Run-id stability:** the integrator is content-hashed only when non-default,
  so default-rk4 models keep their pre-existing run-id (no cache churn) while
  rk45 / explicit tolerances get a distinct address.

## References

- Dormand, J.R. & Prince, P.J. (1980). *J. Comp. Appl. Math.* 6(1):19–26.
- Hairer, Nørsett & Wanner (1993). *Solving ODEs I: Nonstiff Problems*, 2nd ed.,
  Springer, §II.4–II.5 (tableau, PI controller).
- King, A.A., Nguyen, D. & Ionides, E.L. (2016). *J. Stat. Soft.* 69(12) §2.1
  (pomp accumulator variables; Euler-multinomial discretization).
- Hindmarsh et al. (2005). *ACM TOMS* 31(3):363–396 (SUNDIALS quadrature).
- Press et al., *Numerical Recipes* 3rd ed., ch. 17.2 (adaptive-stepsize RK).
- Proposal: `docs/dev/proposals/2026-06-16-ode-adaptive-integrator.md` (gh#166).
