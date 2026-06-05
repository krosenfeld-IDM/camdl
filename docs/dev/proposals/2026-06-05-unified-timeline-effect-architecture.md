---
date: 2026-06-05
status: proposal
related:
  - 2026-06-05-observation-data-binding.md
  - 2026-05-14-reactive-interventions-and-evsi.md
  - archive/pre-alpha/2026-05-04-ode-inference-three-phase.md
area: simulation engine / inference / DSL
issue: TBD
---

# Unified timeline-effect architecture

## Problem

A simulation in camdl is a single thing: a latent state advancing through time,
with **events on a timeline** — observations are read off it, interventions and
cohort entries are written onto it, conservation constraints are imposed on it,
and (soon) reactive policies fire when it crosses a threshold. Today that one
notion is expressed through several parallel surfaces, each with its own loading
path, its own application logic, and — critically — its own way of mapping
continuous time onto integrator steps. Every such surface is a place where the
forward simulator and the inference engine can quietly disagree, and a place a
bug can enter without a test noticing.

This proposal consolidates those surfaces. The thesis is that exactly one thing
is legitimately special — the **process**, which advances the true latent state
with randomness — and everything else (observation, intervention, event,
balance, reactive intervention) is a **triggered effect on a shared timeline**,
expressible through one set of types. The forward simulator and the inference
engine then become two thin *drivers* over that shared substrate, diverging only
where they genuinely must, for reasons we can name.

## The existing infrastructure, and how it is spread

Before the design, the inventory — what exists, and where it is fragmented.
Citations are to the current tree; these are the facts the design must honour.

### Process: two trait hierarchies, one kernel

The dynamics are advanced through **two** trait families:

- `Simulate` (`sim/src/lib.rs`) — the forward path. Implemented by `OdeSim`,
  `TauLeapSim`, `GillespieSim`, `ChainBinomialSim`. Each owns its own stepping
  loop and records a full `Trajectory`.
- `ProcessModel` / `DensityProcess` (`sim/src/inference/traits.rs:40,141`) — the
  inference path. Implemented by exactly **one** type, `ChainBinomialProcess`
  (`chain_binomial_process.rs:52`). Inference is therefore chain-binomial-only;
  `DensityProcess` (the transition density PGAS needs) is chain-binomial by
  design.

The important fact: the transition math is **not** duplicated. Both paths call
the same kernel, `step_one` (`chain_binomial.rs:269`) — `ChainBinomialProcess::step`
(`chain_binomial_process.rs:91-98`) resolves `fire_steps` and delegates straight
to it. What differs is the *loop around the kernel* and an allocation contract:
`ProcessModel::step` is the hot inner loop (called `n_particles × n_substeps ×
n_obs` times, parallel across particles) and **must not allocate** — it threads
a reusable `Scratch` (`traits.rs:62`). The forward driver allocates freely
(`flows = vec![...]`, per-snapshot `clone()`).

### Stepping: three idioms

Continuous time is mapped to integrator steps three different ways:

1. **Forward — merged boundary.** `tau_leap.rs:111-116` and `ode.rs:210-215`
   compute `next_boundary = min(t_end, next_output_time, next_intervention_time)`
   and step `dt.min(next_boundary − t)`, advancing sorted `output_times` /
   `iv_times` cursors. Output and intervention times are hit **exactly**.
2. **Inference filters — step-to-obs.** `particle_filter.rs:231-244`,
   `if2.rs` (two sites), `correlated_pf.rs` — `while t_local < obs_time { step_dt
   = dt.min(obs_time − t_local); … }`. Four hand-rolled copies of the same loop.
   Observations are hit exactly; interventions are handled *inside* `step_one`
   via rounded `fire_steps`, not by the loop.
3. **PGAS — uniform grid.** `pgas.rs` stores its reference trajectory as
   `Trajectory { substeps: Vec<SubstepRecord> }`, one record per uniform `dt`
   step, and maps observations to substeps by rounding
   (`build_obs_at_substep:261` → `interval_steps`). This is the only path that
   **snaps** observation times to a grid (and `insert`s without a collision
   check at `:269`).

`time_to_step(t,dt) = round(t/dt)` (`time.rs:34`) is the shared rounding
primitive. Fire times are rounded the same way in both forward and inference
(`resolve_fire_steps`), so the firing logic is shared — but the *step-boundary*
treatment is not: tau/ode truncate to land on a fire time, while
chain-binomial/PGAS only ever land on the `dt` grid. An off-grid intervention is
therefore hit exactly in tau/ode and rounded by up to `dt/2` in chain-binomial,
in both its forward and inference use — a cross-backend divergence.

### Observation: scoring unified, loading scattered

Scoring is a single seam: `ObservationModel<S>` (`traits.rs:89`), whose one
required method `log_likelihood(&self, state: &S, obs_idx, params) -> f64`
(`:94`) is what *all four* algorithms call for particle weighting. The
production implementor is `MultiStreamObsModel` (`multi_stream_obs.rs:246`); the
projection ADT `StreamProjection = FlowSum | IntCompSum | Expr` (`:72`) selects
incidence (`flow_accumulators`) vs prevalence (`counts`), with
`resets_after_observation()` true only for `FlowSum` (`:87`).

Loading and construction, by contrast, are duplicated: `pfilter.rs`,
`profile.rs`, `fit/runner.rs`, and `survey.rs` each re-resolve `--data`, build
their own per-stream series, run the shared-grid check, and canonicalize
`observations = per_stream_obs[0]`. Observation scoring is **read-only** on state
(`&S`, never `&mut`), with two couplings: `sample()` (diagnostics) consumes RNG
(`obs_model.rs:300`), and the flow-accumulator **reset** is driven by the
*algorithm loop* at observation boundaries (`particle_filter.rs:401-403`) — the
obs model only *declares* the intent via `resets_after_observation()`.

### Interventions, events, balance: shared schedule, separate constraint, fixed ordering

- `Intervention { schedule, actions, always_active }` (`ir/src/intervention.rs:70`).
  `always_active = true` is an **event** (`events {}`, every-substep);
  `false` is a scheduled **intervention** (`interventions {}`). They share the
  `InterventionSchedule` enum (`AtTimes | AtTimesExpr | Recurring`, `:17-29`) and
  the `Action` vocabulary (`FractionTransfer | AbsoluteTransfer | Set | Add`,
  `:59-66`).
- **Balance** is *not* an intervention. It is a `ResolvedBalance` on
  `CompiledModel` (`compiled_model.rs:406`), a structural constraint that
  overwrites one target compartment to satisfy a conservation expression.
- The within-substep application order is fixed and semantic
  (`chain_binomial.rs::step_one`): **transitions → events → interventions →
  balance**. Events apply as deltas computed from the *start-of-step snapshot*;
  interventions mutate the *post-transition current state*; balance is last as
  the consequence of all prior mutations. None of the three consumes RNG.
- Gillespie has a special obligation (spec §2.3.1, `gillespie.rs:174`): after any
  state mutation it must recompute all propensities and draw a fresh exponential
  — it cannot carry remaining exponential time across a mutation.

### Reactive: proposal only

`docs/dev/proposals/2026-05-14-reactive-interventions-and-evsi.md` (and the
narrowed, forward-sim-only 2026-06-03 follow-up) specify a `reactive_interventions
{}` block with a boolean `when` trigger, `after`/`cooldown`/`once`, and firing-
history reads. **Nothing is implemented**: there is no `reactive` symbol in the
Rust or OCaml trees, and `InterventionSchedule` has no state-conditioned variant.

### Where bugs enter

Every non-consolidated surface is a divergence risk:

- the three stepping idioms — an off-grid time means different things to
  different backends;
- four copies of the step-to-obs loop — a fix to one need not reach the others;
- the scattered observation loaders — the silent obs-drop and the homogeneity
  assert live in five places (the subject of the observation-data proposal);
- the global flow-accumulator reset — correct only because all streams share one
  schedule;
- the cross-backend fire-time rounding;

These are not hypothetical: they are the shapes of gh#53 (intervention timing),
the PGAS silent-overwrite, and the §5.2.1 sparse-incidence reset.

## The unified model

### Process stays special

The process is the only component that advances the *true latent state*, and it
is the only one that consumes randomness to do so. That specialness is real and
must be preserved: the alloc-free kernel, the RNG draw *ordering* (on which
paired-seed common-random-number coupling depends, and which feeds the
`gamma_used` / `binomial_z` hooks PGAS and the correlated PF rely on), and — for
gradient inference — the transition density and its derivative.

What is *not* special is the loop around the kernel. The forward merged-boundary
stepper and the inference step-to-obs loop are the same algorithm written four
times. They consolidate into one **schedule** (below); the kernel `step_one` is
already singular.

One boundary on what "one process" can mean: a single fixed-step `step(dt)`
contract subsumes chain-binomial, tau-leap, **and ODE** — ODE steps in `dt`
(RK4 sub-stepping within each step) and simply ignores the `rng` argument
because it is deterministic. It does **not** subsume Gillespie, whose advance is
event-driven (continuous-time, no fixed `dt`). All four still share the
`Schedule` — Gillespie already steps to boundaries (`gillespie.rs`); what
differs is its internal kernel. ODE's *inference* diverges further because it is
deterministic — the trajectory-match driver below — but that is a driver
concern, not a dynamics one.

### Everything else is a typed timeline of triggered effects

Observation, intervention, event, balance, and reactive intervention are all
*effects evaluated at points on the timeline*. They differ along three axes, and
the design's central claim is that **all three axes must be types, not
conventions**, because each is a place a generic "effect" abstraction would leak:

1. **Trigger** — *when* the effect fires (a time, every substep, a state
   condition, an observation time), and what inference contracts that firing
   satisfies.
2. **Relation to state** — whether the effect *reads* state (observation),
   *mutates* it (intervention/event/reactive), or *constrains* it (balance). This
   is the read/write distinction the type system should enforce, so that "an
   observation mutates the dynamics" is unrepresentable.
3. **Phase** — *where in the substep* a mutation applies, and whether it reads
   the start-of-step snapshot or the current state.

## The concrete types

What follows is a design sketch (proposed, not yet implemented). Names are
indicative; the shapes are the proposal.

### The timeline spine (shared)

The merged schedule replaces all three stepping idioms. It is a lazy k-way merge
of sorted boundary streams — the internal `dt` grid, observation times, and
scheduled effect times — yielding the next boundary and what fires there.

```rust
/// A point on the simulation timeline and what happens there.
/// Multiple kinds can coincide at one time (e.g. an obs at an intervention).
pub enum Boundary {
    Substep,                 // an internal dt step: process advances, nothing else
    Output(usize),           // forward only: record a trajectory snapshot
    Observation(usize),      // obs_idx: inference scores here; forward emits here
    Effect(EffectId),        // a scheduled Mutate/Constrain fires here
}

/// Merged, sorted boundary timeline. Cursors advance monotonically (O(1)
/// amortized). `next` returns the next boundary time and the (possibly several)
/// things due there; the caller steps `dt.min(t_next - t)` to land on it.
pub struct Schedule { /* arithmetic dt-grid + sorted obs/effect cursors */ }

impl Schedule {
    pub fn next(&mut self, t: f64) -> (f64 /*t_next*/, SmallVec<[Boundary; 4]>);
}
```

This is the generalization of what `tau_leap.rs:111-116` already does for
`{t_end, output, intervention}`, lifted to a first-class object and extended to
observations. Once it exists, observation times are exact boundaries by
construction — the off-grid and dt-collision problems disappear rather than being
guarded.

### Triggers and capabilities (shared)

```rust
pub enum Trigger {
    AtTimes(Vec<f64>),              // intervention (AtTimesExpr resolved once)
    Recurring(RecurringSchedule),   // periodic intervention/event
    EverySubstep,                   // event, balance
    StateCondition(ResolvedExpr),   // reactive: a boolean over state
    ObservationTime,                // observation
}

/// The inference contracts a trigger does or does not satisfy. This is what
/// lets the engine accept a trigger for gradient/PGAS inference or reject it
/// with a clear error — rather than silently producing a wrong posterior.
pub struct TriggerCaps {
    pub differentiable: bool,  // outcome is smooth in params at this trigger
    pub markov: bool,          // firing needs no path-dependent history
}

impl Trigger {
    pub fn caps(&self) -> TriggerCaps {
        match self {
            // A threshold is piecewise-constant: comparison ops differentiate to
            // 0 (autodiff.ml:116-117) and firing history is latent state that
            // breaks CSMC-AS. Reactive satisfies neither contract.
            Trigger::StateCondition(_) => TriggerCaps { differentiable: false, markov: false },
            _                          => TriggerCaps { differentiable: true,  markov: true  },
        }
    }

    /// Boundary times contributed to the Schedule. Empty for EverySubstep /
    /// StateCondition — those are evaluated at *every* substep, not pre-scheduled.
    pub fn scheduled_times(&self, dt: f64, params: &[f64]) -> Vec<f64>;
}
```

### The Read / Mutate / Constrain split (shared)

The relation-to-state axis is three distinct types, not one `apply(&mut state)`.
This is deliberate: it makes the read-only-on-state guarantee a compile-time
fact, preserves the snapshot-vs-current distinction, and keeps the phase ordering
explicit instead of implicit in declaration order.

```rust
/// Within-substep application order. Lower applies first. SEMANTIC, not
/// cosmetic: events read the pre-transition snapshot; interventions read the
/// post-transition current state; balance is the consequence of all prior.
#[derive(PartialEq, Eq, PartialOrd, Ord)]
pub enum Phase { Transition, Event, Intervention, Balance }

pub enum ReadFrom { Snapshot, Current }

/// READ effect — observation. Gets `&State`, never `&mut`. The projection is
/// shared with today's StreamProjection; the temporal kind governs the
/// accumulator coupling.
pub struct Observe {
    pub trigger:    Trigger,           // ObservationTime
    pub projection: StreamProjection,  // FlowSum | IntCompSum | Expr
    pub kind:       TemporalKind,      // Interval (resets flow accumulators) | Instant
    pub likelihood: ResolvedLikelihood,
}

/// MUTATE effect — intervention, event, reactive. Gets `&mut State`. Consumes
/// NO RNG (verified for all three today). The trigger's caps decide whether it
/// is admissible under gradient/PGAS inference.
pub struct Mutate {
    pub trigger:   Trigger,            // AtTimes | Recurring | EverySubstep | StateCondition
    pub phase:     Phase,              // Event or Intervention
    pub read_from: ReadFrom,           // Snapshot (events) | Current (interventions)
    pub actions:   Vec<Action>,        // Add | Transfer | Set
}

/// CONSTRAIN effect — balance. Structural, always last, overwrites a target.
pub struct Constrain {
    pub target: usize,
    pub expr:   ResolvedExpr,
}
```

These three are what the `Schedule`'s `Effect(EffectId)` boundaries resolve to.
A driver applies, at each substep, the due `Mutate`s in `Phase` order (reading
snapshot or current per `read_from`), then the `Constrain`s; and at an
`Observation` boundary, the `Observe` for that stream.

### The drivers — generate, filter, trajectory-match

All drivers consume the **same** compiled model (process kernel + effects +
schedule). They diverge only in what they *do* at each boundary — and there are
**three**, not two, because the inference question itself forks on whether the
dynamics carry process noise.

**Forward — generate.** Allocates freely; records the full trajectory; emits
observations (samples `y ~ p(y|x)`, consuming RNG); handles all trigger kinds,
including reactive. One pass.

```rust
pub fn run_forward(model: &Compiled, params: &[f64], seed: u64, cfg: &SimConfig)
    -> Trajectory
{
    // for each (t_next, boundaries) from schedule.next(t):
    //   advance process: step_one | integrate (state, params, t, t_next - t, rng, scratch)
    //   evaluate StateCondition triggers against the realized state
    //   apply due Mutate effects in Phase order, then Constrain
    //   Output(_)       => record Snapshot
    //   Observation(i)  => sample y ~ p(y | state, params)  [RNG] and record
}
```

**Stochastic filter — evaluate by filtering.** The alloc-free hot loop. Scores
observations (`log p(y_obs|x)`, no RNG); rejects reactive triggers at
construction. PF / IF2 / PMMH need only `ProcessModel`; PGAS additionally needs
`DensityProcess` and records a per-substep reference for CSMC-AS. State is
`ParticleState` (integer counts); gradients come from `rate_grad` threaded
through the filter.

```rust
pub fn run_filter<P: ProcessModel>(
    process:  &P,
    obs:      &dyn ObservationModel<P::State>,   // the scoring seam (traits.rs:89)
    effects:  &CompiledEffects,                  // Mutate/Constrain, reactive pre-rejected
    schedule: &Schedule,
    /* particles, weights, rngs, … */
) -> Loglik
{
    // per particle, for each (t_next, boundaries) from schedule.next(t):
    //   process.step(state, params, t, t_next - t, rng, scratch)   // alloc-free
    //   apply due Mutate (Phase order, no RNG), then Constrain
    //   Observation(i) => w += obs.log_likelihood(state, i, params) // no RNG
    //   (PGAS: record SubstepRecord onto the schedule index for CSMC-AS)
}
```

**Deterministic trajectory-match — evaluate by integration.** When the dynamics
are deterministic (the ODE backend), the latent trajectory is a function of `θ`
and the initial conditions alone — there is no process noise to filter, so a
particle filter is structurally redundant: every particle is identical, and the
filter burns 100×–1000× the compute on copies. The right driver integrates the
ODE once per `θ`, scores the observation likelihood of that single trajectory,
and optimises or samples `θ` directly. Gradients come from the **forward
sensitivity equations** — evolve `∂x/∂θ` alongside `x`, using the same symbolic
`∂f/∂x`, `∂f/∂θ` the autodiff pass already emits — not from a particle filter.
This is the deterministic-skeleton / trajectory-matching path (pomp's
`traj_match`, King, Nguyen & Ionides 2016; Stan's ODE models), and it is what
unlocks endemic age-stratified fits — malaria EIR-by-age, typhoid SIRC — where
the filter is pure waste. Its algorithms (NLopt MLE, Metropolis–Hastings,
NUTS-via-sensitivity-ODE) are worked out in
`archive/pre-alpha/2026-05-04-ode-inference-three-phase.md`; the unified
architecture gives them a home beside the filter rather than as a parallel stack.
The *full* ODE-inference design (integrator choice, sensitivity-equation
derivation, NLopt/MH/NUTS stage wiring) is deferred to its own proposal; this
design only **reserves the driver seam** — a deterministic `ProcessModel` plus a
non-filter driver — so that nothing in the shared substrate forecloses it.

```rust
pub fn run_trajmatch(
    ode:      &OdeProcess,                      // State = OdeState (Vec<f64>), deterministic
    obs:      &dyn ObservationModel<OdeState>,  // same scoring seam
    effects:  &CompiledEffects,                 // reactive pre-rejected
    schedule: &Schedule,                        // same merged timeline
    params:   &[f64],
) -> (Loglik, Option<Grad>)                     // grad via forward-sensitivity ODE
{
    // integrate ONCE over schedule boundaries (no particles, no process RNG):
    //   advance ODE: integrate(state [+ ∂state/∂θ], params, t, t_next - t)
    //   apply due Mutate (Phase order), then Constrain
    //   Observation(i) => L += obs.log_likelihood(state, i, params); accumulate grad
    // then NLopt / MH / NUTS over θ on (L, grad).
}
```

So the divergence axis is not forward-vs-inference but **generate / filter /
integrate** — three interpretations of one timeline, picked by the question
(simulate? fit a stochastic process? fit a deterministic skeleton?), over one
schedule, one set of effects, and one family of dynamics kernels.

### Why the drivers diverge (the six reasons, plus determinism)

The divergence is confined to the driver, and every part of it is forced:

1. **Allocation.** Forward records a full `Trajectory` and may allocate per step;
   inference is a hot inner loop that must thread a reusable `Scratch`
   (`traits.rs:62`). The kernel is shared; the surrounding bookkeeping is not.
2. **Generate vs evaluate.** At an observation, forward *samples* `y ~ p(y|x)`
   (consuming RNG); inference *scores* `log p(y_obs|x)` (no RNG). Same projection,
   opposite direction — and the RNG difference is load-bearing for coupling.
3. **Density and gradient.** PGAS needs the transition density (`DensityProcess`)
   and its derivative (`rate_grad`); forward never does. This is why
   `DensityProcess` is a separate, chain-binomial-only trait.
4. **RNG ordering / CRN.** Inference's paired-seed coupling and the
   `gamma_used`/`binomial_z` hooks pin the order of draws; reordering breaks
   byte-identical trajectories. Forward is indifferent. Any shared driver must
   not reorder draws across the two.
5. **Capability gating.** A reactive (`StateCondition`) trigger is fine in
   forward simulation but is non-differentiable and non-Markov; it must be
   rejected — with a clear error pointing at `events {}` for known campaigns —
   before it can reach a gradient or PGAS fit. The gate is `trigger.caps()`.
6. **Reference trajectory.** PGAS conditions on a stored reference path
   (`SubstepRecord`) and re-scores it under each particle's prefix; the forward
   driver and the bootstrap PF do not. The schedule must therefore expose a
   stable per-substep index for PGAS even though the other drivers ignore it.

What is *shared*, then, is everything that matters for consistency: the model,
the schedule, the trigger/effect types, and the dynamics kernel. What diverges is
a thin, enumerable set of per-boundary behaviours. That is the whole point — the
drivers can no longer disagree about *when* things happen or *what* the effects
are, only about generate / filter / integrate, which is their actual job.

The trajectory-match driver adds two divergences beyond the six, both from
determinism: with no process noise the filter degenerates, so it integrates once
rather than resampling N identical particles; and its gradient comes from the
forward sensitivity equations (`∂x/∂θ` evolved with `x`) rather than from
`rate_grad` threaded through a filter. Same scoring seam, same schedule, same
effects — a third way to turn `θ` into a likelihood.

## How the pieces relate and flow

Existing types are kept and mapped into the new effect types; one compiled model
feeds three pluggable drivers:

```
 EXISTING (kept)                     NEW (this proposal)
 ───────────────                     ───────────────────────────────
 step_one ............ kernel        Schedule ......... merged timeline
 ParticleState (i64 counts)          Boundary = Substep|Output|Obs|Effect
 OdeState      (f64)                 Trigger  = AtTimes|Recurring|EverySubstep
 DensityProcess (PGAS only)                     |StateCondition|ObservationTime
 ObservationModel / log_likelihood   TriggerCaps{ differentiable, markov }
                                     Phase = Transition<Event<Intervention<Balance
                                     ReadFrom = Snapshot | Current

   existing IR types        ── map ──►   new effect types
   ─────────────────                     ────────────────
   StreamProjection, Likelihood ──────►  Observe   (Read,  &state)
   Intervention/Action/Schedule ──────►  Mutate    (&mut state, Phase)
   (reactive trigger: new)               Constrain (structural, last)
   ResolvedBalance ───────────────────►  ──┘

                    ┌──────────────────────────────────┐
                    │          COMPILED MODEL           │
                    │  kernel + [effects] + Schedule    │
                    └──────────────────────────────────┘
                                   │
          ┌────────────────────────┼────────────────────────┐
          ▼                        ▼                         ▼
     run_forward              run_filter               run_trajmatch
     GENERATE                 FILTER (stochastic)      INTEGRATE (determ.)
     emit + record            score + resample         integrate + score
     all triggers (reactive)  PF/IF2/PMMH/PGAS         NLopt / MH / NUTS
     chain/tau/ode/gillespie  chain-binomial           ODE
                              ParticleState            OdeState + ∂x/∂θ
                              grad: rate_grad          grad: sensitivity ODE
```

The per-boundary control loop is the same for every driver; only the marked
lines differ:

```
  ┌─► Schedule.next(t) → (t_next, [Boundary…])
  │       │
  │       ▼   advance kernel  t → t_next         (step_one | integrate)
  │       ▼   reactive? eval StateCondition       [forward / trajmatch only]
  │       ▼   apply Mutate by Phase:  Event(Snapshot) → Intervention(Current)
  │       ▼   apply Constrain (balance) — last
  │       ▼   at Boundary:
  │             Output(i)      → record snapshot            [forward]
  │             Observation(i) → forward:   sample y~p(y|x)  [RNG, emit]
  │                              filter:    w += log p(y|x)  [score]
  │                              trajmatch: L += log p(y|x)  [score]
  │       ▼   (PGAS only: record SubstepRecord at schedule index)
  └───────┘   loop to next boundary
```

And the trait/type view — which traits exist, what implements them, and where
the new effect types and drivers sit:

```
 TRAITS (interfaces)                     implemented by
 ──────────────────                      ──────────────
 Resettable                              ParticleState, OdeState
   fn reset_accumulators(&mut self)

 ProcessModel : Send + Sync              ChainBinomialProcess (State=ParticleState)
   type State : Clone+Send+Resettable    OdeProcess           (State=OdeState) [future]
   type Scratch
   fn initial_state(&self, θ) -> State
   fn step(&mut State, θ, t, dt, rng, scratch)   ← shared kernel (step_one|integrate)
   fn new_scratch(&self) -> Scratch

 DensityProcess : ProcessModel           ChainBinomialProcess only
   fn log_transition_density(…)            (PGAS / gradient)

 ObservationModel<S> : Send + Sync        MultiStreamObsModel  (S=ParticleState)
   fn log_likelihood(&self,&S,i,θ)->f64   ← scoring seam, READ-ONLY &S
   fn sample(&self,&S,i,θ,rng)->Vec<f64>    (forward only; RNG)

 NEW EFFECT TYPES (structs; method signatures enforce read vs write)
 ───────────────────────────────────────────────────────────────────
 Observe   { trigger, projection, kind, likelihood }
   fn project(&self, state: &State,     t) -> f64   ← &State  (cannot mutate)
 Mutate    { trigger, phase, read_from, actions }
   fn apply  (&self, state: &mut State, t)          ← &mut State, in Phase order
 Constrain { target, expr }
   fn enforce(&self, state: &mut State, t)          ← &mut State, last

 NEW TIMELINE TYPES
 ──────────────────
 Schedule    fn next(&mut self, t) -> (f64, [Boundary])
 Boundary  = Substep | Output(i) | Observation(i) | Effect(id)
 Trigger   = AtTimes | Recurring | EverySubstep | StateCondition | ObservationTime
               fn caps(&self) -> TriggerCaps { differentiable, markov }
 Phase     = Transition < Event < Intervention < Balance     (Ord)
 ReadFrom  = Snapshot | Current

 DRIVERS (free functions over the traits + effects + schedule)
 ─────────────────────────────────────────────────────────────
 run_forward  (&Compiled, θ, seed)                                 -> Trajectory
 run_filter   <P: ProcessModel>(&P, &dyn ObservationModel<P::State>,
                                &Effects, &Schedule)               -> Loglik
                // PGAS branch additionally requires  P: DensityProcess
 run_trajmatch(&OdeProcess, &dyn ObservationModel<OdeState>,
                            &Effects, &Schedule)        -> (Loglik, Grad)  [future]
```

## Leaky abstractions the types must honour

These are the failure modes a naïve "one effect trait" would introduce; the types
above are shaped to prevent each:

- **Read/write erasure.** A single `apply(&mut state)` would let an observation
  mutate the dynamics. Prevented by the `Observe` / `Mutate` / `Constrain` split:
  `Observe` only ever sees `&State`.
- **Ordering collapse.** Flattening effects to declaration order corrupts the
  snapshot-vs-current distinction. Prevented by explicit `Phase` + `ReadFrom`.
- **RNG reordering.** Prevented by keeping `sample` (forward, RNG) out of the
  scoring path entirely (inference never draws to score) and by not reordering
  process draws in the shared driver.
- **Interval/Instant + accumulator reset.** Incidence is an interval integral
  whose window closes at observation boundaries; instantaneous effects are not.
  Preserved by `TemporalKind` on `Observe` and a per-stream reset keyed to that
  stream's observation boundaries (the §5.2.1 work, which the schedule makes
  natural).
- **Gillespie propensity invalidation.** A mutation invalidates the SSA's pending
  exponential. Expressed as a per-backend hook the driver calls after applying
  `Mutate`s, not assumed away.
- **Reactive non-differentiability and non-Markovianity.** Irreducible; handled
  by the capability gate, not by cleverness. Reactive lives in forward/batch;
  policy search over thresholds is grid search, not NUTS.

## Future entry points (three axes to design now)

The architecture should expose three extension axes so future features slot in
as variants rather than new bespoke surfaces:

1. **Trigger** as a first-class enum. Adding `StateCondition` (reactive, gh#175-
   adjacent), windowed `set(param)` with auto-revert (gh#50), and per-stream
   activation dates (gh#171) are then trigger variants on a shared action surface.
2. **Projection** as a composable expression supporting stratum-subset sums and
   effort weighting (`forcing(t) · projection`). This closes the gh#171
   sentinel/environmental-surveillance geometry with no new block.
3. **Reduction** — a *trajectory-functional* axis distinct from the point
   projection. A summary statistic (`peak`, `time-to-peak`, `n_episodes`,
   `final size`) is a function of the whole series, not `f(state(t))` at a point.
   This is the one axis the current `{trigger, action, projection, scoring}`
   model lacks, and it is the substrate for summary-statistic targets and
   synthetic-likelihood / ABC scoring (gh#172). Designing it now prevents a
   bespoke `summary {}` surface later.

Explicitly **out of the unification**: vital dynamics and spatial coupling change
the *transition graph* (compartment topology), not the timeline; treating them as
timeline effects would be the worst leak of all. Reporting delays are a
convolution over time (scoring-with-memory), a borderline case to design
separately.

## Relationship to the observation-data proposal, and sequencing

The observation-data binding proposal
(`2026-06-05-observation-data-binding.md`) and this one are complementary, not
competing, and they interleave rather than strictly sequence:

- The binding proposal's **data layer** — `LongRow` parsing, `bind`, `BoundObs`,
  the cardinality/typing rules, the `Counted` denominator, the NaN guard — is
  about getting observation *data* into the system correctly (the
  stream/stratum/value axes). It is **independent** of how the timeline steps and
  can proceed in parallel; it is the construction of the `Observe` effects'
  observed series.
- The binding proposal's **temporal/scoring layer** — off-grid policy
  (`--snap-observations`), the union-axis scoring, the per-stream `Interval`
  reset — is exactly where the two proposals meet. The **merged schedule here
  dissolves it**: observation times become exact boundaries, so the off-grid
  apparatus (`OffGridInterval`/`OffGridInstant`, the snap question, the
  dt-induced collisions) is *not built*. What remains is the per-stream reset,
  which the schedule makes clean.

So the precise answer to "is this implemented before the observation proposal?":
**the schedule spine of this proposal lands before the observation proposal's
temporal/scoring tier, and removes most of it; the observation proposal's data
layer is independent and proceeds in parallel.** The observation proposal is not
superseded — its data work survives intact and becomes the loader for `Observe`
effects. It should be re-scoped to drop the off-grid machinery once this
proposal's schedule direction is accepted.

## Migration (phased, each step green)

1. **Extract the `Schedule`** from the forward backends into a shared type; route
   the forward backends through it (refactor, byte-identical — golden trajectories
   must not move).
2. **Route the inference filters through the `Schedule`** — collapse the four
   step-to-obs copies (`particle_filter`, `if2`×2, `correlated_pf`) into one
   driver loop. This also makes interventions exact boundaries in inference,
   retiring the cross-backend rounding divergence (task #9) and the gh#53 class.
   Gate: CRN coupling preserved (paired-seed byte-identical), full `make test`.
3. **Introduce the `Observe` / `Mutate` / `Constrain` types** + the `Phase` /
   `ReadFrom` / `TriggerCaps` machinery; re-express today's interventions, events,
   balance, and observations as instances. No behaviour change; the capability
   gate is additive.
4. **Migrate PGAS's reference trajectory onto the `Schedule`** (the high-risk
   tier): `SubstepRecord`, `build_obs_at_substep`, `csmc_as`,
   `log_transition_density_substep`, and `complete_data_loglik_grad` move off the
   uniform grid together. Gate: the Richardson `dt`-ladder + pomp external
   validation, the gradient FD battery, no posterior drift on the golden fits.
5. **Reactive as a capability-gated `StateCondition` trigger** — forward/batch
   only, rejected from gradient/PGAS fits with a hint. (Implements the held
   reactive proposal's forward-sim scope.)
6. **The reduction axis + composable projection** — the future-feature substrate
   (gh#171, gh#172).

Steps 1–3 are mechanical and low-risk; step 4 is the inference-math-core change
and the reason this is a proposal rather than a refactor PR.

## Out of scope

- Gillespie's *internal advance* under a fixed `step(dt)` contract — it is
  event-driven (continuous-time, exact), so its kernel is "advance to the next
  event or boundary," not a fixed step. It still rides the shared `Schedule`;
  only its dynamics primitive differs. ODE is **not** excluded — it steps in
  `dt` and gets the trajectory-match driver above.
- Vital dynamics, spatial coupling — structural (transition-graph) changes.
- Reporting-delay convolutions — scoring-with-memory, designed separately.

## Testing

This is correctness-critical, refactor-heavy code: most of the migration moves
working logic behind a new abstraction *without intending to change a single
result*, and the one place it does change results — the PGAS reference
trajectory — is the place a silent error corrupts a posterior with no symptom.
The strategy is built around that asymmetry: **differential parity is the
spine**, and the irreducible behaviour change gets the strongest external
oracles. Nothing here lowers a bar to go green; a failure is a cause to find.

### Differential parity is the spine

Steps 1–3 are refactors. The forward `Schedule` extraction, the filter-loop
unification, and the effect-type re-expression must each be **bit-for-bit
identical** to the code they replace.

- **Keep the old path live behind a flag** during each step (the
  `CAMDL_EVAL_UNRESOLVED` differential-oracle pattern from the pre-resolution
  work) and assert the new path matches the old, byte-for-byte, on a corpus of
  models, *before* deleting the old path. A refactor whose two sides are not
  provably identical is not done.
- **Golden trajectories do not move.** All `ir/expected/*` and golden fits are
  re-run; any diff is a regression until proven intended. An intended diff (e.g.
  an intervention previously rounded, now exact) is isolated to its own step,
  documented, and the golden updated in that commit alone.
- **RNG draw order is an invariant, not an accident.** A harness that logs the
  sequence of RNG draws (kind, count, order) asserts it is identical old-vs-new.
  This is what protects paired-seed CRN coupling and the `gamma_used` /
  `binomial_z` PGAS hooks; a reordering that shifts a posterior by 1e-12 still
  fails here — which is the point.

### Per-step gates

| step                         | what changes            | gate                                                                                                                  |
| ---------------------------- | ----------------------- | --------------------------------------------------------------------------------------------------------------------- |
| 1. forward `Schedule`        | refactor                | golden trajectories byte-identical; RNG-order invariant                                                               |
| 2. filter unification        | refactor (4 loops → 1)  | PF/IF2/PMMH/PGAS log-likelihoods byte-identical on the corpus; paired enable/disable byte-identical pre-intervention; correlated-PF coupling preserved |
| 3. effect types             | refactor (re-express)   | golden unchanged; the phase-ordering pin; the capability-gate negative test                                          |
| 4. PGAS reference → schedule | **behaviour may change** | the external-oracle battery below — the only step where parity is *not* the bar                                       |
| 5. reactive trigger          | additive (forward only) | capability-gate rejection in `fit`; runs under `simulate`/`batch`                                                     |
| 6. reduction axis            | additive                | new-feature tests; no movement on existing                                                                            |

### Cross-cutting invariants (property-based)

- **Schedule correctness.** Over randomly generated sets of obs / effect / `dt`
  times: every observation time is a boundary hit exactly; no two *distinct*
  times collide onto one boundary; the merged sequence is sorted and strictly
  monotone; and where the old `interval_steps` grid and the new schedule agree
  (on-grid times), substep counts match. Proptest.
- **Phase ordering.** A model exercising transitions + events + interventions +
  balance, hand-computed: events read the pre-transition snapshot, interventions
  the post-transition state, balance last and exempt from the negative-count
  check. Asserted on the actual counts, not a proxy.
- **Read/write at the type level.** That `Observe` cannot mutate state is a
  compile-time fact (its method takes `&State`). A `trybuild` test that a
  mutating `Observe` *fails to compile* makes the guarantee regression-proof.
- **Cross-backend fire-time.** Same model, off-grid intervention: chain-binomial
  and tau-leap score consistently, and the discrepancy → 0 as `dt → 0` (the
  task #9 test). Red-first against current code — it must fail today.
- **Gillespie propensity invalidation.** A model where skipping the
  post-mutation recompute yields a detectably wrong next-rate; assert it fires.

### External oracles for the irreducible change (step 4)

The PGAS reference-trajectory migration is the only step that can change a
posterior and the only one where a bug is silent. It does not ship on internal
parity alone:

- **pomp cross-check** — the He et al. (2010) measles benchmark already used to
  catch gh#52/gh#53 (`time.rs` header): the PGAS MLE/posterior must match pomp's
  to the established tolerance after the migration.
- **Richardson `dt`-ladder** — log-likelihood and posterior summaries converge
  along a `dt` refinement ladder at the expected order; a scheduler bug that
  mis-tiles a window breaks the *convergence rate*, not just the value.
- **FD gradient battery** — every gradient arm (hierarchical, obs-parameter)
  still matches central finite differences to ≤ 1e-6; the gradient is
  re-validated *after* the reference moves, not assumed stable.
- **Posterior non-drift** — on the golden fits, posterior means within the
  pre-migration credible bands, with a KS check on the marginals to catch a
  distributional shift a point estimate would miss.

### Red-first for the bugs this retires

Each bug the unification fixes gets a failing test first, against current `main`,
then shown green: the gh#53 intervention-rounding class, the cross-backend
off-grid divergence (#9), and the sparse-`Interval` per-stream reset window error
(the §5.2.1 trap). A "fix" with no red baseline is not trusted.

### What "done" means

No step merges without a clean full `make test` (unit + golden + integration);
the parity steps additionally require the differential-oracle corpus green and
the RNG-order invariant intact; step 4 additionally requires the external-oracle
battery. No `--no-verify`, no widened tolerance, no skipped gate.
