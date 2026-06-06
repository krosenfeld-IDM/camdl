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

A simulation in camdl is one thing: a latent state advancing through time, with
**events on a timeline** — observations read off it, interventions and cohort
entries written onto it, conservation constraints imposed on it, and (later)
reactive policies fired when it crosses a threshold. Today that one notion is
expressed through several parallel surfaces, each with its own loading path, its
own application logic, and — most consequentially — its own way of mapping
continuous time onto integrator steps. Every such surface is a place the forward
simulator and the inference engine can quietly disagree, and a place a bug can
enter without a test noticing.

This proposal consolidates the surfaces. The thesis: exactly one component is
legitimately special — the **process**, which advances the true latent state with
randomness — and everything else (observation, intervention, event, balance,
reactive intervention) is a **triggered effect on a shared timeline**, applied in
one **canonical substep lifecycle**, expressible through one set of types. The
forward simulator and the inference engine become thin *drivers* over that
shared substrate, diverging only where they genuinely must.

It also makes one long-standing implicit inconsistency explicit and
user-controllable: today the bootstrap particle filter steps *exactly* to
observation times while PGAS *snaps* them to the integrator grid. We expose that
as a single `snap | exact` knob, default `snap`, with a staged path to flipping
it once it is profiled and validated.

## The existing infrastructure, and how it is spread

The inventory the design must honour. Citations are to the current tree.

### Process: two trait hierarchies, one kernel

- `Simulate` (`sim/src/lib.rs`) — the forward path. `OdeSim`, `TauLeapSim`,
  `GillespieSim`, `ChainBinomialSim`, each owning its own stepping loop and
  recording a full `Trajectory`.
- `ProcessModel` / `DensityProcess` (`sim/src/inference/traits.rs:40,141`) — the
  inference path, implemented by exactly **one** type, `ChainBinomialProcess`
  (`chain_binomial_process.rs:52`). Inference is chain-binomial-only;
  `DensityProcess` (the transition density PGAS needs) is chain-binomial by
  design.

The transition math is **not** duplicated: both paths call the same kernel
`step_one` (`chain_binomial.rs:269`); `ChainBinomialProcess::step`
(`chain_binomial_process.rs:91-98`) resolves `fire_steps` and delegates to it.
What differs is the *loop around the kernel* and an allocation contract:
`ProcessModel::step` is the hot inner loop (`n_particles × n_substeps × n_obs`
calls, parallel across particles) and **must not allocate** — it threads a
reusable `Scratch` (`traits.rs:62`). The forward driver allocates freely.

### Stepping: three idioms, two boundary policies

1. **Forward — merged boundary.** `tau_leap.rs:111-116`, `ode.rs:210-215`:
   `next_boundary = min(t_end, next_output, next_intervention)`, step
   `dt.min(next_boundary − t)`. Output and intervention times hit **exactly**.
2. **Inference filters — step-to-obs.** `particle_filter.rs:231-244`, `if2.rs`
   (two sites), `correlated_pf.rs` — `while t_local < obs_time { step_dt =
   dt.min(obs_time − t_local) }`. Four copies. Observations hit **exactly**;
   interventions applied inside `step_one` via rounded `fire_steps`.
3. **PGAS — uniform grid.** `pgas.rs` stores `Trajectory { substeps:
   Vec<SubstepRecord> }`, one record per uniform `dt` step, mapping obs to
   substeps by rounding (`build_obs_at_substep:261` → `interval_steps`). The only
   path that **snaps** observations, and the only one whose density paths
   reconstruct time as `t = t_start + s*dt` (`pgas.rs:568,606`,
   `pgas_grad.rs:397`).

So two boundary policies already coexist *implicitly*: the PF lands exactly, PGAS
snaps. This proposal makes that a deliberate, uniform, named choice.

### Observation: scoring unified, loading scattered

Scoring is one seam: `ObservationModel<S>` (`traits.rs:89`), its required method
`log_likelihood(&self, state: &S, obs_idx, params) -> f64` (`:94`) called by all
four algorithms. Implementor: `MultiStreamObsModel` (`multi_stream_obs.rs:246`);
projection ADT `StreamProjection = FlowSum | IntCompSum | Expr` (`:72`), with
`resets_after_observation()` true only for `FlowSum` (`:87`). Loading is
duplicated across `pfilter.rs`, `profile.rs`, `fit/runner.rs`, `survey.rs`.
Observation scoring reads state (`&S`), but an `Interval` observation *drives* a
state mutation — `state.reset_flows()` (`particle_filter.rs:401`) — through the
algorithm loop. That reset is a real write, and the design represents it.

### Interventions, events, balance: shared schedule, separate constraint, fused ordering

- `Intervention { schedule, actions, always_active }` (`ir/src/intervention.rs:70`);
  `always_active = true` is an **event** (every substep), `false` a scheduled
  **intervention**. Shared `InterventionSchedule = AtTimes | AtTimesExpr |
  Recurring` (`:17-29`, `AtTimesExpr` = gh#69 parametric fire times) and `Action
  = FractionTransfer | AbsoluteTransfer | Set | Add` (`:59-66`).
- **Balance** is a `ResolvedBalance` on `CompiledModel` (`compiled_model.rs:406`),
  a structural constraint overwriting one target compartment.
- Within a substep the order is fixed and semantic (`chain_binomial.rs::step_one`):
  transition deltas and event deltas are **computed from the start-of-step
  snapshot and applied atomically together** (`:424-433`), *then* interventions
  on the post-transition state (`:489`), *then* balance last (`:503`, target
  exempt from the negative-count check). None consumes RNG.
- Gillespie has a special obligation (spec §2.3.1, `gillespie.rs:174`): after any
  state mutation, recompute all propensities and draw a fresh exponential — it
  cannot carry remaining exponential time across a mutation.

### Reactive: proposal only

`docs/dev/proposals/2026-05-14-reactive-interventions-and-evsi.md` specifies a
`reactive_interventions {}` block (state-condition trigger). Nothing is
implemented; `InterventionSchedule` has no state-conditioned variant.

## dt-dependence vs dt-independence

The organizing axis for everything below: whether a backend's result depends on
the *step size*.

- **dt-dependent (stochastic, fixed-step):** chain-binomial, tau-leap. A step
  over interval `h` draws `Binom(N, 1−e^{−λh})` / a Poisson, **freezing the rate
  `λ` at the start-of-step value**. For a state-dependent rate (`λ_SI = βI/N`),
  two steps of `h/2` re-evaluate `λ` at the midpoint — a *finer*, more accurate
  approximation. So the realized trajectory distribution is a function of where
  the step boundaries fall. This is not error vs. correctness; both converge to
  the exact process as `dt → 0` (O(dt) rate-freezing difference).
- **dt-independent:** Gillespie (exact SSA, event-driven, no discretization
  error — indifferent to where you stop) and ODE (deterministic — integrates to
  whatever time you ask, no noise).

The consequence runs through the whole design: **landing exactly on an off-grid
boundary changes the result only for dt-dependent backends.** Gillespie and ODE
land exactly for free. This is precisely why the `snap | exact` choice matters
for chain-binomial/tau-leap and is a no-op for Gillespie/ODE, and why the PGAS
exactness migration (which would introduce non-uniform substeps under a
dt-dependent kernel) is the one genuinely delicate piece.

## The unified model

### Process stays special

The process alone advances the true latent state, and alone consumes randomness
to do so. That specialness is real: the alloc-free kernel, the RNG draw
*ordering* (paired-seed common-random-number coupling and the `gamma_used` /
`binomial_z` hooks PGAS and the correlated PF depend on it), and — for gradient
inference — the transition density and its derivative. What is *not* special is
the loop around the kernel; that consolidates. The kernel `step_one` is already
singular.

A single fixed-step `step(dt)` contract subsumes chain-binomial, tau-leap, and
ODE (ODE steps in `dt` with RK4 sub-stepping; it ignores `rng` because it is
deterministic). It does **not** subsume Gillespie (event-driven). All four share
the `Schedule`; Gillespie's *kernel* differs (it proposes the next time; the
schedule can only clip it — see below).

### Everything else is a typed timeline of triggered effects

Observation, intervention, event, balance, reactive intervention are effects at
points on the timeline, differing along three axes that the design makes **types,
not conventions**, because each is where a generic "effect" would leak:

1. **Trigger** — when it fires, and what inference contracts that firing satisfies.
2. **Relation to state** — read (observation), mutate (intervention/event/
   reactive), or constrain (balance). The read/write split is type-enforced.
3. **Lifecycle stage** — where in the substep it applies, and what state it reads.

### The canonical substep lifecycle

The within-substep order is a first-class, documented object — the analogue of
SLiM's published tick/generation cycle, whose defining virtue is that a modeller
can reason precisely about *when* their script runs relative to reproduction and
selection (Haller & Messer 2019, *MBE* 36:632; the SLiM manual's lifecycle
diagrams). camdl's substep lifecycle, stated to match `step_one` exactly:

```
  ┌─ start of substep: snapshot x_t ───────────────────────────────┐
  │  1. PROPOSE    transition draws (rates frozen at x_t)           │
  │                event deltas (computed from the x_t snapshot)    │
  │  2. ADVANCE    apply transition + event deltas ATOMICALLY → x'  │  fused — one stage
  │  3. INTERVENE  apply scheduled interventions on x' (current)    │
  │  4. BALANCE    enforce conservation (last; target exempt)       │
  │  5. OBSERVE    read projection of post-effect state; score/emit │  read-only
  │  6. RESET      if an Interval obs fired here: zero THAT          │  represented
  │                stream's flow accumulators                       │  write
  └─ end of substep: x_{t+dt} ─────────────────────────────────────┘
```

Two corrections this canonization bakes in. "Events read the snapshot" is a
property of **stage 1** (the delta is *computed* from `x_t`), not a separate
later phase — transitions and events apply *together* in stage 2 (a single,
fused stage, not `Transition < Event`). And the accumulator reset is **stage 6**,
a represented per-stream write, not a hidden side effect of an observation. This
lifecycle belongs in user-facing docs (the language spec / user-features) with a
polished figure; it is how a modeller reasons about "does my intervention see my
event." It ships as its own small documentation PR alongside Stage 0 — it
canonizes `step_one` as it already is (zero unification risk) and fixes the
contract everything else refactors against.

## The concrete types

A design sketch (proposed). Names indicative.

### The timeline spine

```rust
/// A point on the timeline and what is due there. Several kinds can coincide.
pub enum Boundary {
    Substep,                 // an internal dt step: process advances only
    Output(usize),           // forward only: record a snapshot
    Observation(usize),      // inference scores here; forward emits here
    Effect(EffectId),        // a scheduled Mutate/Constrain fires here
}

/// Merged sorted boundary timeline. Cheaply Clone-able / immutable with an
/// external cursor, so the parallel alloc-free hot loop is not serialized
/// behind a &mut cursor.
pub struct Schedule { /* arithmetic dt-grid + sorted obs/effect cursors */ }

impl Schedule {
    /// Fixed-step drivers: the next boundary at or after t, and what is due.
    /// Coincident kinds at one time are returned together; the driver applies
    /// them in lifecycle order (Effect/Constrain before Observation).
    pub fn next_boundary(&self, cursor: &mut Cursor, t: f64) -> (f64, SmallVec<[Boundary; 4]>);

    /// Event-driven (Gillespie): the process PROPOSES t_proposed; the schedule
    /// can only clip it to the nearest earlier boundary (or pass it through).
    pub fn clip(&self, cursor: &Cursor, t_proposed: f64) -> ClipResult;
}
```

Two distinct entry points because the spine genuinely forks: fixed-step drivers
*ask* the schedule for the next time; Gillespie *proposes* a time and the
schedule clips it. Both share the boundary set and cursor; only the query
differs. `t_end` is the schedule's terminal boundary; effect/obs times beyond it
are a load error; an empty schedule yields `(t_end, [])` once and terminates.

### Triggers and capabilities

```rust
pub enum Trigger {
    AtTimes(Vec<f64>),              // intervention; AtTimesExpr resolved once → AtTimes
    Recurring(RecurringSchedule),
    EverySubstep,                   // event, balance — fires every Substep boundary
    StateCondition(ResolvedExpr),   // reactive — evaluated every substep, forward only
    ObservationTime,                // observation
}

/// The inference contracts an effect satisfies. Computed from the effect's
/// ACTUAL expressions, not the trigger variant.
pub struct EffectCaps { pub differentiable: bool, pub markov: bool }

/// Runs Rust-side at driver construction (after `estimated` is resolved — it is
/// a fit-time set, not an IR field). A dedicated SMOOTHNESS predicate, NOT a
/// `differentiate` call: `differentiate` silently returns `Const 0.0` for
/// Floor/Ceil/comparison ops and leaves Cond predicates undifferentiated
/// (autodiff.ml:117,161-162,186) — so a param entering only through a Cond
/// predicate or Floor would pass as differentiable with a wrong ZERO gradient.
/// Sound rule: differentiable = false iff collect_param_refs(expr) ∩ estimated
/// ≠ ∅ (the reachability half — it DOES see Cond.pred / TableLookup,
/// pgas.rs:33-64) AND the expr contains any non-smooth node
/// (Mod | Floor | Ceil | comparison | Cond-pred). StateCondition forces {false,false}.
fn effect_caps(effect: &Effect, estimated: &ParamSet) -> EffectCaps;
```

### The lifecycle stage and the read/write split — two orthogonal axes

**Stage** (when in the substep) is the full 6-valued lifecycle order carried by
*every* effect — it is the sort key the driver uses. **Relation to state**
(read / mutate / constrain) is the method signature — it is the read/write the
type system enforces. Keeping them separate is what the round-1 review's
"lifecycle collapse" leak required: a 3-valued `Stage` that omitted `Observe` and
`Reset` could not order all six steps.

```rust
/// The full substep lifecycle as a total order. EVERY effect carries one.
#[derive(PartialEq, Eq, PartialOrd, Ord)]
pub enum Stage { Advance, Intervene, Balance, Observe, Reset }
//   txn+event FUSED = Advance  <  Intervene  <  Balance  <  Observe  <  Reset

/// READ — observation (Stage::Observe). Gets &State, never &mut.
pub struct Observe { pub trigger: Trigger, pub projection: StreamProjection,
    pub kind: TemporalKind, pub likelihood: ResolvedLikelihood,
    fn project(&self, state: &State, t: f64) -> f64; }

/// EVENT — Stage::Advance. A fused delta CONTRIBUTOR, not an independent &mut:
/// its delta is computed from the start-of-step snapshot and applied atomically
/// with the transition draws (chain_binomial.rs:424-433). Hence propose_delta,
/// not apply — an apply(&mut State) could not be atomic with transitions.
pub struct Event { pub trigger: Trigger /*EverySubstep*/, pub actions: Vec<Action>,
    fn propose_delta(&self, snapshot: &State, t: f64) -> Deltas; }

/// INTERVENTION — Stage::Intervene. Applied to the post-transition current
/// state, after the fused Advance. A genuine &mut. No RNG.
pub struct Intervene { pub trigger: Trigger, pub actions: Vec<Action>,
    fn apply(&self, state: &mut State, t: f64); }

/// CONSTRAIN — balance (Stage::Balance), last among writes; target exempt from
/// the negative-count check.
pub struct Constrain { pub target: usize, pub expr: ResolvedExpr,
    fn enforce(&self, state: &mut State, t: f64); }

/// RESET — Stage::Reset. The Interval flow-accumulator window close, keyed to the
/// firing stream's flow indices (NOT global) — the §5.2.1 per-stream fix.
pub struct ResetWindow { pub flow_indices: Vec<usize>,
    fn reset(&self, state: &mut State); }
```

The event/intervention split is *forced* by the fusion: an event contributes a
delta applied atomically with transitions, so its read-source (the snapshot) is in
the type via `propose_delta`, while an intervention's is the current state via
`apply` — not a convention keyed on a `stage` field. `Action::Set/Add` carrying a
`StreamProjection` must preserve the documented hash-position contract
(`observation.rs:12-16` — variant index is permanent; reshaping churns every
stored `run_id`).

### The drivers — generate, filter, trajectory-match

All consume the same compiled model (kernel + effects + schedule), diverging only
per-boundary. Two ship here — `run_forward` and `run_filter`; `run_trajmatch` is a
*reserved seam* sketched to show the substrate generalizes to deterministic
inference (its full design is the deferred ODE-inference proposal), not shipped
architecture.

```rust
pub fn run_forward(model: &Compiled, params: &[f64], seed: u64, cfg: &SimConfig)
    -> Trajectory;          // GENERATE: emit (RNG) + record; all triggers incl. reactive

pub fn run_filter<P: ProcessModel>(            // EVALUATE (stochastic): score, no RNG
    process: &P, obs: &dyn ObservationModel<P::State>,
    effects: &CompiledEffects, schedule: &Schedule, /* particles … */) -> Loglik;
    // PF/IF2/PMMH need ProcessModel; PGAS additionally requires P: DensityProcess
    // and records SubstepRecord (carrying the realized (t0, dt_substep), below).

pub fn run_trajmatch(                           // EVALUATE (deterministic): integrate once
    ode: &OdeProcess, obs: &dyn ObservationModel<OdeState>,
    effects: &CompiledEffects, schedule: &Schedule, params: &[f64]) -> (Loglik, Grad);
    // [future] grad via forward sensitivity ODE; FlowSum-over-flow_accumulators
    // does NOT exist for OdeState — the trait is shared, the projection impl is not.
```

**Why they diverge** (forced, not arbitrary): allocation (forward records, filter
is alloc-free); generate-vs-evaluate (forward samples `y ~ p(y|x)` with RNG,
inference scores `log p(y|x)` without); density+gradient (PGAS needs
`DensityProcess`); RNG ordering / CRN (filter pins draw order); capability gating
(reactive rejected from gradient/PGAS); reference trajectory (PGAS records and
conditions on one). The trajectory-match driver adds two more from determinism:
the filter degenerates (integrate once, no resampling of identical particles) and
the gradient comes from the forward sensitivity equations, not `rate_grad`. Same
schedule, same effects, same scoring seam — different ways to turn `θ` into a
likelihood. The full ODE-inference design is its own proposal; here we only
reserve the driver seam (a deterministic `ProcessModel` + a non-filter driver),
and note one normative constraint: a `θ`-dependent `Set`/`Constrain` at a
boundary discontinuously reseeds `∂x/∂θ`, so trajectory-match must capability-gate
those until a sensitivity-jump rule is implemented.

## The `snap | exact` boundary policy

The implicit per-algorithm inconsistency (PF exact, PGAS snap) becomes one
explicit, uniform, user-controllable option: `--obs-alignment snap | exact`
(bundled in `fit.toml`).

- **`snap`** — observation/effect times are rounded to the integrator grid
  (today's PGAS behaviour; for the PF, round the obs time before stepping). Keeps
  a single uniform `dt`, fully reproducible, no per-substep bookkeeping.
- **`exact`** — the integrator lands exactly on each boundary (today's bootstrap
  PF behaviour, generalized: tile each inter-boundary window with its own uniform
  sub-grid of `⌈W/dt⌉` equal steps, so boundaries are hit with no tiny remainder
  step). Lossless timing.

For **dt-independent** backends (Gillespie, ODE) the two coincide — there is no
noise to perturb. (Caveat: Gillespie is dt-independent only for time-*homogeneous*
rates; gh#95 is exactly the current implementation's inhomogeneous-rate bias, so
"Gillespie lands exactly for free" is an idealization the code does not yet meet —
do not lean on it as a clean invariant until gh#95 is fixed.) For **dt-dependent**
backends (chain-binomial, tau-leap) they differ at finite `dt` (the rate-freezing
granularity changes) and converge as `dt → 0`; `exact` is the more accurate,
`snap` the more reproducible.

**The gate, and the consolidation it forces.** `exact` is **not** a
`Capabilities` bitflag. The existing `Capabilities` (`sim/src/lib.rs`) is a
*model × backend* axis (`required_capabilities()` scans the IR; each forward
backend's `capabilities()` declares support), but `obs-alignment` is a
*run-option × algorithm* axis — PGAS is an algorithm, has no `capabilities()`, and
never passes through `Simulate`. And the alignment-relevant gating today is *two*
separate call sites — `util.rs:1699` (forward) and `check_model_capabilities`
(`fit/methods.rs`, a hard-coded `match backend { … }`) — neither seeing algorithm
identity. So the gate is a new `(algorithm, obs_alignment)` support check at the
fit-dispatch seam, and **consolidating those two existing gates into it is part of
this work** — otherwise "one clean error / one place" is false today. With that
done, `exact` + PGAS hits one clean error ("not implemented: PGAS supports `snap`
only; use `--obs-alignment snap`, or `algorithm = if2|pfilter` for `exact`"), and
the test asserting it is a positive test that routing is consolidated to a single
seam. (Verify PGAS genuinely *lacks* the `exact` capability — absent, not
defaulted-true.)

## Staging and default policy

Sequencing is **oracle-first, then gh#175, then extract** — you cannot prove a
refactor byte-identical against a baseline that never ran the hard case, and you
cannot trust a PGAS-touching change while the PGAS gradient is broken. The default
is conservative and only flips after evidence.

**Stage 0 — build the comparison oracle (FIRST, before any extraction).** The
existing forward ratchet (`gate_trajectory_baseline.rs`: per model × backend ×
`SEED=42`, FNV-hash the trajectory vs a committed table) is the right *shape* but
covers only forward simulation on an all-on-grid corpus. Stage 0 closes the gap:

- **Corner-case fixtures** (`tests/fixtures/` → `ocaml/golden/`): an off-grid
  observation (e.g. `t = 7.3`), a coincident observation+intervention, a
  `θ`-dependent `set()` at a fractional time, an irregular multi-cadence stream,
  and a fractional `output.end` (the `seir_vaccine_seasonal` 1095.7275 case).
- **Forward baselines** — extend `gate_trajectory_baseline.rs`'s `BASELINES`
  (captured from *current* code on this machine, per its platform caveat).
- **Inference baselines (the missing piece)** — a new `gate_inference_baseline.rs`:
  per model × algorithm (PF/IF2/PMMH/PGAS) × `SEED=42`, pin the marginal loglik
  *and* the per-observation scored contributions, from current code. This is the
  actual oracle, since the refactor rewrites the inference loops; the existing
  ratchet only covers forward `Simulate`.
- **RNG draw-sequence baseline** — a harness logging (kind, count, order) of draws
  per run, so an inserted/reordered draw fails loudly.
- **Runtime collision-guard test** — feed two distinct sub-`dt` obs times and
  assert the *runtime* hard error (not just a proptest generator constraint).

None of this touches production code; it is the ratchet everything else refactors
against, and the gating deliverable — not a testing footnote. The canonical
substep-lifecycle doc/figure ships here too (zero-risk; it fixes the contract).

**Stage 1 — spine, byte-identical.** Extract the `Schedule`, route the forward
backends and the four filter loops through it, install the canonical substep
lifecycle and the corrected effect types. Each path keeps its *current* boundary
behaviour (PF exact, PGAS snap, interventions as today). Strictly byte-identical
against the Stage-0 oracle (forward, inference, and RNG-sequence baselines),
including the off-grid and coincident-boundary fixtures. This is loop
consolidation and the bug-surface win — it *names* the PF/PGAS divergence and
deletes the duplicated loops, but does not yet *close* the divergence (that is
Stage 2).

**Stage 2 — expose the knob, `snap` default.** Add `--obs-alignment`, default
`snap`. Because `snap` and `exact` *coincide where every obs/output time is an
exact `dt` multiple*, the on-grid goldens are unchanged; the only behaviour that
changes is the PF's *implicit off-grid exactness*, now opt-in via `exact`. (Audit
the corpus for hidden fractional times first — `seir_vaccine_seasonal`'s
`output.end = 1095.7275` snaps under the default and must be pinned to `exact` or
re-baselined deliberately.) Any off-grid fit that relied on exactness is pinned to
`exact`, so "reproduces past results" is literal. The capability gate lands here.

**Stage 3 — future, evidence-gated (the eventual clean interface).** After
(a) profiling the `snap`-vs-`exact` performance difference, and (b) validating
across a variety of models that the adaptive (`exact`) stepping is correct and
reproduces references, *consider* flipping the default to `exact` (or hiding the
knob) — this is the clean-interface goal, deliberately *after* both modes work and
match current behaviour, never before. Separately, the **exact-PGAS** migration
moves the uniform-grid assumption out of PGAS so each record carries its realized
`(t0, dt_substep)` and **no path recomputes `s*dt`** — eight reconstruction sites
to convert together (`pgas.rs:568,605,716,869,1079`, `pgas_grad.rs:397`, plus the
`interval_steps` obs mapping at `pgas.rs:268,704`); a single missed site silently
reconstructs the wrong time → wrong rate freeze → wrong density. Gated behind a
mixing PGAS (gh#175 fixed → a trustworthy parity baseline) and a genuine need for
off-grid observations *under PGAS specifically*. Until then, `exact` + PGAS is the
clean capability error.

## Consolidation: substrate, not algorithms

The consolidation reduces cross-backend bug surface at the **substrate** layer —
the schedule, the substep lifecycle, effect application, the kernel — which is
shared by *all* drivers, including the particle filter and PGAS. It does **not**
merge the algorithms: bootstrap filtering and conditional-SMC-with-ancestor-
sampling (the reference trajectory, the density, the gradient) are genuinely
different and stay distinct above the substrate. So PF and PGAS share when/how
the timeline advances and effects apply (the bug-prone part), and keep their own
inference logic (the part that should stay separate). In this push PGAS keeps its
uniform grid (it honours `snap` only); its exactness migration is the deferred
increment, so the substrate consolidation lands without touching the delicate
reference-trajectory path.

Honest accounting of "surface": deleted are the four step-to-obs loops and the
three hand-rolled forward boundary cursors. Added are the `Schedule`/`Boundary`/
`Trigger`/`Effect` types and the driver trio; the existing IR types are kept and
*mapped*, not removed. The win is consolidating control flow into one typed
spine, not reducing the type count — and that is where the cross-backend bugs
live.

## Leaky abstractions the types must honour

- **Read/write erasure** — prevented by `Observe` (`&State`) vs `Mutate`
  (`&mut`); the accumulator reset is a represented `ResetWindow`, not hidden.
- **Lifecycle collapse** — prevented by the canonical lifecycle with the
  transition+event *fusion* modelled honestly and the snapshot read at stage 1.
- **RNG reordering / insertion** — `sample` (forward, RNG) stays out of scoring;
  and a variable last step *inserts* a draw, so the RNG invariant is on draw
  *count and order*, exercised by an off-grid corpus (below). This is why `snap`
  (no inserted draws) is the reproducible default.
- **Interval/Instant + reset** — `TemporalKind` first-class; reset per-stream by
  flow index (stage 6), not global.
- **Gillespie propensity invalidation** — a per-backend hook the driver calls
  after applying `Mutate`s; expressed, not assumed away. And the schedule `clip`
  query (not `next_boundary`) for the event-driven kernel.
- **Off-grid under a dt-dependent kernel** — handled by the `snap | exact` policy
  and the staging, never silently: `snap` is byte-identical on-grid; `exact` is a
  validated behaviour change, not a refactor.

## How the pieces relate and flow

Existing types kept and mapped into new effects; one compiled model feeds three
drivers:

```
 EXISTING (kept)                     NEW (this proposal)
 ───────────────                     ───────────────────────────────
 step_one ............ kernel        Schedule ......... merged timeline
 ParticleState (i64 counts)          Boundary = Substep|Output|Obs|Effect
 OdeState      (f64)                 Trigger  = AtTimes|Recurring|EverySubstep
 DensityProcess (PGAS only)                     |StateCondition|ObservationTime
 ObservationModel / log_likelihood   EffectCaps{ differentiable, markov }
                                     Stage = Advance(txn+event) < Intervene < Balance
                                     snap | exact  (obs-alignment policy)

   existing IR types        ── map ──►   new effect types
   StreamProjection, Likelihood ──────►  Observe   (Read,  &state)
   Intervention/Action/Schedule ──────►  Mutate    (&mut state, Stage)
   ResolvedBalance ───────────────────►  Constrain (structural, last)
   (FlowSum reset, today global) ─────►  ResetWindow (per-stream, stage 6)

                    ┌──────────────────────────────────┐
                    │          COMPILED MODEL           │
                    │  kernel + [effects] + Schedule    │
                    └──────────────────────────────────┘
                                   │
          ┌────────────────────────┼────────────────────────┐
          ▼                        ▼                         ▼
     run_forward              run_filter               run_trajmatch [future]
     GENERATE                 FILTER (stochastic)      INTEGRATE (determ.)
     emit + record            score + resample         integrate + score
     all triggers (reactive)  PF/IF2/PMMH/PGAS         NLopt / MH / NUTS
     chain/tau/ode/gillespie  chain-binomial           ODE
                              ParticleState            OdeState + ∂x/∂θ
                              grad: rate_grad          grad: sensitivity ODE
```

Trait/type view — only new structs, no new traits; drivers are free functions
bounded by the existing traits:

```
 TRAITS (existing)                       implemented by
 ProcessModel : Send+Sync                ChainBinomialProcess (State=ParticleState)
   type State : Clone+Send+Resettable    OdeProcess           (State=OdeState) [future]
   fn step(&mut State, θ, t, dt, rng, scratch)   ← shared kernel (step_one|integrate)
 DensityProcess : ProcessModel           ChainBinomialProcess only  (PGAS / gradient)
 ObservationModel<S> : Send+Sync         MultiStreamObsModel
   fn log_likelihood(&self,&S,i,θ)->f64  ← scoring seam, READ-ONLY &S

 NEW (structs; signatures enforce read vs write)
 Observe{trigger,projection,kind,likelihood}   project(&self,&State,t)->f64
 Mutate{trigger,stage,actions}                 apply(&self,&mut State,&State,t)
 Constrain{target,expr}                        enforce(&self,&mut State,t)
 ResetWindow{flow_indices}                     reset(&self,&mut State)
 Schedule  next_boundary(&self,&mut Cursor,t)->(f64,[Boundary]) ; clip(&self,&Cursor,t)->…
 Trigger / EffectCaps / Stage / Boundary
```

## Testing

Correctness-critical, refactor-heavy code. The spine is a refactor (parity is the
spine); the `exact` policy and the deferred PGAS migration are behaviour changes
(external oracles). The discipline is asymmetric to match.

### Parity is the spine — and the corpus must be off-grid

- **Old path live behind a flag** (the `CAMDL_EVAL_UNRESOLVED` differential-oracle
  pattern); the new path matches byte-for-byte on a corpus before the old path is
  deleted.
- **The corpus MUST include an off-grid observation and a `θ`-dependent effect.**
  On-grid goldens cannot exercise the short-substep / inserted-draw hazards, so
  an on-grid-only corpus passes *vacuously*. This is the single most important
  testing requirement.
- **RNG draw order *and count* invariant.** A harness logging the draw sequence
  asserts it is identical old-vs-new; a variable step that *inserts* a draw fails
  here. Under `snap` (Stage 1/2) the count is invariant by construction.

### Per-stage gates

| stage                          | what changes              | gate                                                                                              |
| ------------------------------ | ------------------------- | ------------------------------------------------------------------------------------------------- |
| 1. spine                       | refactor                  | full golden corpus (incl. off-grid) **byte-identical**; RNG count+order invariant; CRN preserved   |
| 2. `snap` knob + default       | none on-grid              | on-grid identical; off-grid PF pinned to `exact` reproduces its prior result; capability-gate test |
| `exact` for PF/forward         | behaviour (off-grid only) | off-grid: validated against the Richardson `dt → 0` ladder converging to the same limit            |
| exact-PGAS [deferred, Stage 3] | behaviour (silent path)   | external-oracle battery below; gated on gh#175 mixing                                              |

### Cross-cutting invariants

- **Schedule (proptest):** every obs time is a boundary hit exactly; two
  *distinct* times within `dt` collide → **runtime hard error** (not a generator
  invariant — feed two sub-`dt`-separated obs and assert the error); the merged
  sequence is sorted/monotone; on-grid substep counts match `interval_steps`.
- **Substep lifecycle:** a model exercising transitions + events + interventions
  + balance + a coincident obs+intervention, hand-computed — events read the
  stage-1 snapshot, interventions the post-transition state, balance last, and
  the obs scores the post-effect state. Asserted on actual counts.
- **Read/write at the type level:** a `trybuild` test that a mutating `Observe`
  *fails to compile*.
- **Capability gate (consolidation test):** `exact` + PGAS → the clean
  not-implemented error; the same model runs under `simulate`/`pfilter`.
- **Cross-backend fire-time:** same model, off-grid intervention, scored across
  chain-binomial and tau-leap, consistent and → 0 as `dt → 0` (red-first).
- **Gillespie propensity invalidation:** a model where skipping the post-mutation
  recompute is detectably wrong; assert it fires; assert `clip` (not
  `next_boundary`) is the query.

### External oracles for the deferred exact-PGAS (Stage 3)

The only step that can silently shift a posterior. It does not ship on parity:
the He et al. (2010) measles **pomp cross-check** (already caught gh#52/gh#53),
the **Richardson `dt`-ladder** convergence *rate*, the **FD gradient battery**
re-validated after the reference moves, and **posterior non-drift** (means within
prior credible bands + a KS check on marginals). Plus: byte-identical
`(counts_before, flows, gammas)` records between pre- and post-migration
`simulate_reference` *and* between `step_one` and the Schedule-driven free-particle
propagation on the off-grid corpus.

### What "done" means

No stage merges without a clean full `make test`; parity stages additionally
require the differential-oracle corpus (off-grid) green and the RNG count+order
invariant intact; the deferred PGAS step additionally requires the external-oracle
battery. No `--no-verify`, no widened tolerance, no skipped gate.

## Relationship to the observation-data proposal

Complementary, kept as separate documents (the obs-data proposal is re-edited
after this lands). Its **data layer** — `LongRow` parse, `bind`, `BoundObs`,
cardinality, `Counted`, the NaN guard — constructs the `Observe` effects' observed
series and is independent of the timeline; it proceeds in parallel. Its
**temporal layer** — off-grid policy, `--snap-observations`, dt-collisions —
**re-homes here** as the `snap | exact` knob plus the schedule's collision guard;
that machinery is not built in the obs-data proposal. The per-stream `Interval`
reset it deferred is the `ResetWindow` stage here.

## Future entry points (deferred seams)

Three extension axes the architecture leaves open, none built in this push:
**Trigger** as a first-class enum (reactive `StateCondition`, windowed
`set(param)` gh#50, activation dates gh#171); **Projection** composable
(stratum-subset sums, effort weighting, gh#171); a separate **Reduction** axis
(trajectory-functionals — `peak`, `n_episodes`, gh#172 — the substrate for
summary-statistic / synthetic-likelihood scoring). Out of the unification
entirely: vital dynamics and spatial coupling (transition-graph changes, not
timeline effects); reporting-delay convolutions (scoring-with-memory).

## Out of scope

- Gillespie's internal advance under a fixed `step(dt)` contract — event-driven;
  it uses the schedule via `clip`, not `next_boundary`.
- The full ODE-inference design (its own proposal); only the driver seam is
  reserved here.
- exact-PGAS — deferred to Stage 3, gated on gh#175.
- reactive triggers, the reduction axis — their own proposals when consumers exist.
