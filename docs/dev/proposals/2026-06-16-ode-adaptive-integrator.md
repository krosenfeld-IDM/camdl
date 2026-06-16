# Adaptive (RK45) ODE integrator for long-horizon deterministic fits

- **Status:** Draft (Phase 0 RFC)
- **Issue:** gh#166
- **Relates:** gh#52 / gh#227 (Richardson dt-check), gh#54 (`Expr::Dt`),
  [`2026-06-15-ode-gradient-inference.md`](2026-06-15-ode-gradient-inference.md)

## Problem

The ODE backend integrates with a **fixed-step RK4** at the user's `dt`, clipped
only to land on output and intervention boundaries. For a long-horizon,
mostly-quiescent epidemic the trajectory is near-flat for long inter-outbreak
stretches, yet fixed-step RK4 pays the full per-step cost everywhere — and a
deterministic MLE/profile fit pays that cost across hundreds of objective
evaluations. The reported case: a 23-patch × 2-age cVDPV2 model over 4382 days,
AFP monthly, 4 estimated params, `nl-sbplx` on `backend = "ode"`, dt=1 — **~1107
s for one converged MLE** (single chain). Coarsening `dt` is not a clean lever:
it trades trajectory accuracy and risks misplacing SIA-pulse intervention times,
which fall on arbitrary integer dates.

An adaptive, error-controlled integrator takes large steps where the trajectory
is smooth and small steps only where it is stiff or rapidly changing — the right
shape for this cost profile, without the dt-coarsening accuracy tax.

## Current state (verified against code)

- `rust/crates/sim/src/ode.rs:99` `rk4_step` — classic fixed-step 4-stage RK4,
  four RHS evaluations per step, state clamped `≥ 0` after each step.
- Step driver `run_ode` (`ode.rs:166`): builds
  `Schedule::new(cfg.dt, …, StepPolicy::Exact, output_times, intervention_times)`
  and advances by `schedule.substep = dt.min(next_boundary − t)` (`ode.rs:232`).
  The step is the nominal `cfg.dt` **clipped to land exactly on the next output
  / intervention / event boundary**. Snapshots are emitted on the fixed
  `model.output.times` grid; effects (events then interventions) are applied at
  boundaries through `EffectBatch`.
- Flow (incidence) accumulation is **explicit Euler** (`ode.rs:277`):
  `flow_acc[i] += rate(t_start)·dt`, one left-rectangle evaluation per substep —
  global order O(dt). The integrated _state_ is O(dt⁴) (RK4); the _flow_ is
  O(dt). This asymmetry is latent today and becomes load-bearing here.
- There is no integrator abstraction: `rk4_step` is a free function and
  `run_ode` is monolithic.
- ODE capabilities: `REAL_COMPARTMENTS | RUNTIME_DT`. `Expr::Dt` rates (gh#54)
  evaluate at the realized substep length `dt_actual`.
- IR surface (`rust/crates/ir/src/model.rs:68` `SimulationConfig`;
  `ir/schema.json` `simulation_config`):
  `{ t_start, t_end, time_semantics, dt?,
  rng_seed? }`. DSL `simulate {}`
  accepts only `from`, `to`, `dt` (`ocaml/lib/compiler/parser.mly:829` rejects
  any other key by name).

## The seam: most of the hard part is already solved

The genuinely hard part of adaptive-ODE-with-events is landing exactly on output
and scheduled-intervention times while letting the integrator choose its own
step elsewhere. camdl already owns that machinery — `Schedule` / `Cursor` /
`EffectBatch` with `StepPolicy::Exact`. An adaptive integrator changes only how
a single `[boundary, next_boundary)` interval is crossed: instead of one
`dt`-clipped fixed RK4 step, run an error-controlled stepper that proposes,
accepts, or rejects sub-steps and clips its final accepted step to the boundary.
Everything outside that inner loop — boundary detection, effect application,
snapshot emission, the `compute_ode_loglik` obs-time alignment — is unchanged.

Concretely, factor the per-interval advance behind a small trait:

```rust
/// Advance (int_vals, real_vals, flow_acc) from `t` by at most `h_max`
/// (the distance to the next boundary), returning the step actually taken.
/// Fixed RK4 always takes `h_max`; an adaptive stepper takes ≤ h_max and
/// is re-entered until the boundary is reached.
trait OdeStepper {
    fn step(
        &mut self,
        model: &CompiledModel, params: &[f64],
        t: f64, h_max: f64,
        int_vals: &mut [f64], real_vals: &mut [f64], flow_acc: &mut [f64],
    ) -> Result<f64 /* h_taken */, SimError>;
}
```

`run_ode` becomes a thin driver over `OdeStepper`; `Rk4Fixed` is the current
behaviour (h_taken == h_max), `Dopri5` is the new adaptive stepper. The fixed
path stays byte-identical (same call, same clamp, behind the trait).

## Key design decision: flow accumulation must match the integrator order

A high-order adaptive _state_ integrator is wasted if incidence stays O(dt)
Euler — and silently so, because most fits read prevalence (state) where the
high order shows up, while incidence (flow) quietly lags. Two options:

1. **Augment the cumulative flow as integrated state.** Append one
   `c_i = ∫ rate_i dt` variable per transition to the integrated system and
   advance it with the same RK45 stages. Incidence then converges at the
   integrator order, and the per-interval flow handed to the obs likelihood is
   `Δc` over the interval — exact to the integrator.
2. **Embedded quadrature** over the RK stage rate evaluations (Simpson-like).
   Cheaper to bolt on but couples the quadrature to each tableau.

Recommendation: **option 1**. It is the clean, integrator-agnostic answer; it
_also_ removes the existing O(dt) Euler incidence wart on the fixed path if we
choose to route fixed RK4 through the same augmented-state mechanism; and it
dissolves the prevalence-vs-incidence threshold caveat now documented on the ODE
dt-check (`run_richardson_ladder_ode`). This is the part to specify and test
first.

## Surface (DSL + IR + CLI)

Adaptive integration is **opt-in**. ODE has no RNG, so the adaptive trajectory
is deterministic given `(model, θ, atol, rtol)`, but it is **not
byte-identical** to fixed-step RK4 — making it the default would move every
`ir/expected/*.tsv` ODE golden. Default stays fixed-RK4; adaptive is selected
explicitly.

DSL (`simulate {}` gains keys; `dt` becomes the fixed-RK4-only knob):

```camdl
simulate {
  from = 0 'years
  to   = 40 'years
  integrator = "rk45"      # "rk4" (default) | "rk45"
  atol = 1e-8              # absolute tolerance (rk45 only)
  rtol = 1e-6              # relative tolerance (rk45 only)
}
```

CLI: `camdl simulate … --integrator rk45 --atol 1e-8 --rtol 1e-6`, and the same
override available to `fit run` so deterministic stages (`nl-sbplx`,
`nl-bobyqa`, `mh`) can opt in.

IR: `simulation_config` gains `integrator: "rk4" | "rk45"`, `atol?`, `rtol?`.
This is an **IR schema change** — it requires the atomic update (CLAUDE.md
"Changing the IR schema"): `ir/schema.json` + bump `ir/VERSION` (0.14 → 0.15),
OCaml `ir/` types + (de)serialize, Rust `ir/` types, then
`make update-golden && make update-expected`, all in one commit. The DSL change
(new `simulate` keys, dimensionless `atol`/`rtol`) touches `lexer.mll` /
`parser.mly` / `dimcheck.ml` and must give the spec's named-key error for any
unknown key, plus a migration line in `docs/language-changes.md`.

## Capability interaction: `Expr::Dt`

Under adaptive stepping there is no single nominal `dt`, so a rate that
references `Expr::Dt` (gh#54, `RUNTIME_DT`) has no well-defined value. Phase 1:
**capability-gate `RUNTIME_DT` models out of the adaptive integrator** with an
honest hard error ("model uses `dt` in a rate; the rk45 integrator has no fixed
step — use `integrator = \"rk4\"`"), rather than silently redefining `dt` as the
varying accepted step.

## Algorithm choice

- **Phase 1: DOPRI5 (Dormand–Prince RK45)** — explicit, embedded 4th/5th-order
  error estimate, standard PI step-size controller. This directly addresses the
  reported pain (near-quiescent stretches between outbreaks); explicit methods
  are simple, allocation-light, and need no Jacobian.
- **Stiffness:** TB latency (per-decade reactivation vs fast progression) and
  SIA pulses create real timescale separation. Explicit DOPRI5 copes by
  shrinking steps in stiff regions (partial loss of speedup, never wrong). A
  genuine stiff/implicit method (Rosenbrock / BDF) needs the **state Jacobian**
  ∂(dX/dt)/∂X. Reality check: `ocaml/lib/ir/autodiff.ml` currently
  differentiates rates **with respect to parameters only** — compartment counts
  are treated as constants (`autodiff.ml:26`, `:57`: `Pop _ -> false`). So the
  Jacobian is _not_ available today. The symbolic-diff engine exists and could
  gain a differentiate-wrt-`Pop` target — a contained extension, not a new
  autodiff — but that is **Phase 2**, gated on a model that explicit DOPRI5
  actually chokes on.

## Validation & external oracles

External validation against independent implementations is a first-class
deliverable, following the existing `tests/external/cases/` pattern (R-generated
reference, cached fixtures committed so CI needs no R — as in the He-2010
pfilter loglik gates).

1. **Primary oracle — R `deSolve::lsoda`** (Hindmarsh/Petzold LSODA, the
   reference adaptive ODE solver). Encode a handful of canonical models — SIR,
   SEIR, the 2-stage-latency TB model, and one model with a mid-horizon
   intervention pulse — as the same ODE RHS in R, solve at tight tolerance, and
   assert the camdl `rk45` trajectory agrees to a stated tolerance at the
   observation grid. The intervention-pulse case is the one that exercises
   exact-boundary-landing under adaptive stepping.
2. **Secondary — scipy `solve_ivp(method="RK45")`** for algorithm-level
   agreement on the same models (DOPRI5 vs DOPRI5), and `method="LSODA"` as a
   second independent adaptive reference.
3. **Internal consistency gate** (no external dep): fixed-RK4 at a fine `dt` and
   `rk45` at tight `(atol, rtol)` must agree on both prevalence **and**
   incidence to a stated tolerance — this is what pins the augmented-flow design
   (option 1 above) and catches an incidence/flow regression that a
   prevalence-only check would miss.
4. **Determinism gate:** same `(model, θ, atol, rtol)` → byte-identical
   trajectory across runs (ODE has no RNG; this guards against nondeterministic
   step acceptance).

The cVDPV2 ~1107 s → target wall-clock is a **benchmark, reported in the PR**,
not a correctness gate (it is hardware-dependent and must not gate CI).

## Phasing

- **Phase 0** — this RFC. Pins the `OdeStepper` seam, the augmented-flow
  decision, the opt-in surface + schema change, the `Expr::Dt` gate, and the
  validation plan.
- **Phase 1** — `Dopri5` stepper: error control + PI controller, step-clipped to
  existing boundaries, flows as augmented state, opt-in via
  `integrator =
  "rk45"`. Lands with the deSolve external-validation case, the
  fixed-vs-adaptive internal gate, and the cVDPV2 speedup benchmark in the PR.
- **Phase 2 (optional)** — stiff/implicit option via a differentiate-wrt-`Pop`
  autodiff target for the Jacobian, gated on a real stiff model.

## Coupling to the Richardson dt-check (gh#227)

The dt-check's `run_ladder` driver (`rust/crates/cli/src/fit/dt_check.rs`) is
integrator-agnostic: it builds a ladder of per-rung loglik evaluations and
reduces it to a Pass/Marginal/Fail verdict. For the adaptive integrator the
"halve `dt`" ladder becomes a **"tighten tolerance" ladder** (halve
`atol`/`rtol`, check the loglik is stable) — the same driver, a different
per-rung knob. So the convergence-audit machinery already in tree extends to
rk45 without a second implementation, and the user story closes cleanly: the
dt-check flags that fixed-RK4 at a given `dt` is discretization-dependent → the
user switches to `integrator = "rk45"` → error control removes the dependence.

## Open questions

1. Route fixed RK4 through the augmented-flow mechanism too (fixing the existing
   O(dt) incidence wart, but moving every ODE golden), or leave fixed RK4
   exactly as-is and only the rk45 path gets augmented flows? Cleaner long-term
   vs. smaller blast radius.
2. Default tolerances (`atol`/`rtol`) — pick values that make rk45 agree with
   fine-`dt` RK4 to ~sub-nat loglik on the validation models, and state the
   calibration.
3. Step-controller details (PI gains, min/max step, max rejections before
   surfacing a `SimError`) — standard, but record the choices for
   reproducibility.
4. Does any current model rely on `Expr::Dt` such that the Phase-1 capability
   gate is more than theoretical? (Audit `RUNTIME_DT` usage in goldens/tests.)
