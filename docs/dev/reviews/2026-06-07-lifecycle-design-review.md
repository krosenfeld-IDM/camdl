## The highest-priority issues I would fix before the tau fold

### 1. `dt` is overloaded in ways that can silently fire effects at the wrong time

Right now the same `dt` argument is used for at least three different concepts:

1. the **actual numerical substep length**,
2. the **schedule/grid resolution** used to map times to fire steps,
3. the **`EvalCtx.dt` value** exposed to model expressions.

Those are not equivalent under `Exact`.

The sharpest instance is the inference path. `ChainBinomialProcess::step`
resolves `fire_steps` using `self.dt`, then calls `step_one(..., dt, ...)` with
the actual substep `dt`. Inside `step_one`, `propose_event_deltas` ultimately
calls `inject_event_deltas`, which computes
`current_step = time_to_step(t_end, dt)`. If inference clips a substep to an
observation time, the fire-step lookup can now be keyed by the clipped `dt`,
while the fire-step table was built from the base `self.dt`.

That is exactly the kind of silent wrong answer you are trying to eliminate.

I would introduce a small explicit time object and stop passing raw `(t, dt)`
through the lifecycle:

```rust
struct StepClock {
    t0: f64,
    t1: f64,
    dt_actual: f64,   // numerical integration / hazard interval
    grid_dt: f64,     // schedule/fire-key resolution
    eval_t: f64,      // rate/forcing evaluation time if different
}
```

Then make a conscious decision about `EvalCtx.dt`: is it `dt_actual`, `grid_dt`,
or something else? Today chain-binomial mostly uses actual `dt`; tau-leap often
uses `cfg.dt` for propensity/draw-expression evaluation while using actual
clipped `dt` for probabilities. That alone can break tau equivalence for models
whose expressions reference `dt`.

This is the deepest abstraction leak I saw.

### 2. `fire_steps` should probably not be the core runtime firing API anymore

`apply_interventions_at` takes a tolerance, but ignores it and maps `t` to
`current_step` via `time_to_step`. That means the new `Schedule` knows exact
boundary times, but the actual firing path still re-derives “what is due”
through a separate step-index convention.

That is fragile for:

- `Exact` stepping,
- off-grid effects,
- parametric schedules,
- multiple close effect times,
- reactive schedules,
- particle-specific schedules,
- observations that enqueue interventions.

The cleaner seam is:

```rust
struct EffectBatch {
    t: f64,
    intervention_ids: Vec<usize>,
    event_ids: Vec<usize>,
}
```

Let `Schedule` or an `EffectAgenda` decide which effect IDs are due. Then
`apply_interventions_at` should not rediscover due-ness by converting time back
into a step index. It should apply a known batch.

That also gives you a natural place to preserve document order and coalesce
times under a tolerance.

### 3. Chain-binomial real state looks broken in the attached excerpt

In `chain_binomial.rs`, `StepScratch` owns a `real_s`, but I do not see it
synchronized from the run’s `real_s` before propensity evaluation or event
evaluation. `step_one` copies `counts` into `scratch.int_s`, but not the current
`real_s`. Then it evaluates propensities with `scratch.real_s`.

Also, `apply_post_advance` mutates `scratch.real_s`, but the caller only copies
`scratch.int_s.counts` back into `counts`; the run’s real state is not copied
back.

So, unless the full crate has a mechanism outside this excerpt, chain-binomial’s
transition rates, event expressions, and real-compartment interventions may read
or mutate stale scratch real state.

That matters for tau deletion because your “same kernel” proof is only
trustworthy over the model surface that is actually synchronized. I would add a
real-compartment coupling fixture before deleting tau.

### 4. ODE still violates the stated event snapshot rule

The canonical order says events are proposed from the start-of-step snapshot and
fused with advance. In `ode.rs`, after `rk4_step`, the code converts the
**post-RK4** state with `to_states`, then calls `apply_events_at`. That means
value-dependent events read the post-advance rounded state, not the
start-of-step continuous state.

The comments acknowledge that ODE event fusion is deferred because the current
event delta path is i64-only. That is fair as an intermediate step, but then ODE
is not actually participating in the canonical lifecycle yet. I would not
document “all four backends route through the lifecycle” without carving out
that exception.

The right fix is the f64 effect seam: events should produce
representation-appropriate deltas from the pre-step ODE snapshot, then those
deltas should be applied to the continuous state at the advance boundary without
rounding the whole trajectory.

### 5. Gillespie is not "exact SSA" when propensities vary continuously in time or depend on real compartments

The Gillespie code draws one exponential waiting time from the current
`lambda_total`, then advances real compartments with RK4 to the event/boundary.
That is exact only when propensities are piecewise constant between
events/boundaries. If rates depend on seasonal forcing, clock time, or evolving
real compartments, the next-event distribution is not exponential with frozen
`lambda_total`.

The code already has a TODO about PDMP thinning. I would make this explicit in
the capability surface. Gillespie remains worth keeping, but as an oracle for
autonomous integer CTMCs or piecewise-constant hazards, not as a general exact
reference for all camdl models.

That affects your backend-rationalization language: “exact small-count dynamics”
is true only under the right model restrictions.

## Dilemma 1: lifecycle shape

I would choose **closure-taking driver**, but make it a first-class lifecycle
driver, not a loose helper.

Something like:

```rust
fixed_step_substep(
    state,
    clock,
    effect_agenda,
    scratch,
    |snapshot, event_batch, current, scratch| {
        // backend-specific ADVANCE
    },
)
```

The driver should own:

1. snapshot capture,
2. event proposal from snapshot,
3. backend advance,
4. atomic fusion of transition/event deltas,
5. scheduled interventions,
6. balance,
7. postcondition checks.

The backend closure should not be allowed to reorder lifecycle stages. It should
only implement the kernel-specific advance.

A scoped trait over the three fixed-step backends is mechanically equivalent,
but I would not introduce it yet. The current backends are free functions,
Gillespie cannot honestly implement the trait, and the only shared method would
be the default lifecycle order. That is exactly the case where a closure driver
is clearer and cheaper.

For Gillespie, use a separate boundary helper:

```rust
apply_boundary_effects_without_fixed_step(...)
```

That function can share event/intervention/balance application without
pretending Gillespie has a substep advance.

I would remove the `// → FixedStepLifecycle` comments for now. They bias the
code toward a trait before the shape has earned it.

## Dilemma 2: i64/f64 state split

I would use an **enum/state-view apply seam**, not broad generics.

But I would avoid a raw `IntStore { Discrete(Vec<i64>), Continuous(Vec<f64>) }`
that makes callers reason about rounding. Instead, put the rounding policy
behind methods:

```rust
enum CountStoreMut<'a> {
    Discrete(&'a mut [i64]),
    Continuous(&'a mut [f64]),
}

impl CountStoreMut<'_> {
    fn add_raw(&mut self, idx: usize, raw: f64) -> Result<(), SimError>;
    fn set_raw(&mut self, idx: usize, raw: f64) -> Result<(), SimError>;
    fn fraction_transfer(&mut self, src: usize, dst: usize, frac: f64) -> Result<(), SimError>;
    fn absolute_transfer(&mut self, src: usize, dst: usize, raw: f64) -> Result<(), SimError>;
}
```

Then the action interpreter evaluates `resolved_val` once and calls the
appropriate method. The discrete implementation preserves current i64
rounding/flooring exactly. The continuous implementation applies exact f64
deltas.

That keeps representation variation local, avoids generic `<I>` plumbing through
the simulator, and prevents the event producer from needing to know
representation details.

The tests should pin i64 byte identity action-by-action:

- `Add`: round for discrete, exact for continuous.
- `Set`: round for discrete, exact for continuous.
- `FractionTransfer`: floor for discrete, exact for continuous.
- `AbsoluteTransfer`: round/min for discrete, exact/min for continuous.
- event proposal from snapshot, intervention application on current.

One correctness trap: **do not round the whole ODE state just because one action
fires**. Today `to_states` does exactly that. The new seam should mutate the
continuous ODE count vector in place.

## Dilemma 3: drop tau-leap?

Yes. I would drop tau-leap as a separate backend.

But I would make the deletion gate stricter than “the main integer golden corpus
passes.”

The fold is safe only after you prove these cases:

1. integer-only models,
2. off-grid interventions,
3. always-active events that read the source compartment,
4. simultaneous event + intervention + output,
5. overdispersed transitions,
6. deterministic transitions,
7. competing exits,
8. ungrouped inflows,
9. tiny positive rates near `RATE_EPSILON`,
10. models whose expressions reference `dt`,
11. real compartments coupled into transition rates,
12. lineage observer enabled,
13. balance enabled or explicitly rejected under `Exact`,
14. inference stepping to off-grid observations.

Right now tau and chain are close, but not perfectly equivalent in the attached
code. Examples: chain uses `RATE_EPSILON = 1e-15`; tau checks `rate <= 0.0` in
at least one competing-exit path. Tau uses `cfg.dt` in some evaluation contexts
where chain uses actual `dt`. Chain has CPM/PGAS scratch machinery that tau does
not. The fold should therefore extract one shared Euler-multinomial kernel
rather than relying on two copies remaining “similar.”

I would replace the user-facing backend with something like:

```text
backend = "chain_binomial"
step_policy = "snap" | "exact"
```

Optionally keep `"tau_leap"` as an alias for one release cycle if external users
already have configs. Internally, it should not map to a separate
implementation.

## Deeper smells and corrective suggestions

### A. The schedule needs boundary kinds, not just cursors

`Schedule::substep` returns only a step size. The caller then separately checks
output/effect due-ness. That is why each loop still has subtle boundary
handling.

I would rather have:

```rust
enum BoundaryKind {
    Output,
    Effect,
    Observation,
    End,
}

struct NextStep {
    t0: f64,
    t1: f64,
    dt: f64,
    hit: SmallVec<[BoundaryKind; 3]>,
}
```

Then the lifecycle driver can say:

1. advance to `t1`,
2. if `Effect`, apply effects,
3. if `Output`, record output,
4. if `Observation`, score/reset/react.

This also removes the zero-`dt` boundary loops in tau/ODE and reduces
infinite-loop risk.

### B. `Schedule` should own `t_start`

The constructor takes `t_end` but not `t_start`. It therefore cannot assert or
filter output/effect/observation times before the run window. If a cursor starts
on a boundary before `t_start`, `substep` can return negative durations or the
backend can record snapshots labeled before the simulation start.

Make `Schedule` an interval object: `[t_start, t_end]`. Filter or reject
boundaries outside that interval at construction.

### C. `Schedule::substeps` is unsafe if effect boundaries are present

The `Substeps` iterator calls `schedule.substep(&self.cursor, self.t)` but never
advances the effect/output/obs cursor inside the iterator. If
`StepPolicy::Exact` sees an effect boundary before the observation boundary, the
iterator can land on it and then yield zero-length steps forever.

Maybe inference currently passes an obs-only schedule. If so, encode that in the
type. Do not let an iterator that cannot advance effect cursors see effect
boundaries.

### D. Post-effect negative checks are incomplete

The negative-count check after transition/event deltas is good, but scheduled
interventions can still create invalid states.

Examples in the attached code:

- `Action::Set` can set an integer compartment negative.
- `Action::AbsoluteTransfer` with a negative resolved value can become a
  negative transfer, effectively reversing direction or subtracting from the
  destination.
- non-finite resolved values can silently cast to integers.
- `Add` negative errors, but the error uses `t: 0.0` even though
  `apply_intervention` has `t` in scope.

I would add one centralized post-lifecycle invariant:

```rust
validate_no_negative_counts_except_balance_target(...)
```

Run it after `INTERVENE + BALANCE`, not just after transition deltas. Also
validate every action’s resolved value is finite before applying it.

### E. Balance should be a constrained postcondition, not just a write

`apply_post_advance` evaluates balance, rounds, writes the target, and warns if
negative. That may be okay for exploratory runs, but for public-health
simulations a negative balance compartment is usually a model inconsistency, not
just a log message.

I would make the behavior explicit in config:

```text
balance_negative = "error" | "warn"
```

Default to error for forward/public runs. Inference can downgrade selected
recoverable errors to `-Inf` where appropriate.

### F. Output/observation ordering needs to be declared before reactive campaigns

Current forward backends apply effects before recording outputs at the same
time. That means an output at `t` observes the post-intervention state at `t`.

For reactive campaigns, you need a precise convention:

```text
advance process to t
apply scheduled process effects at t
balance
score/observe/output at t
reset accumulators
run reaction policy
enqueue future effects
```

Or choose a different order, but choose it deliberately. The dangerous case is
an observation at `t` that triggers an intervention also at `t`. Does that
intervention affect the likelihood at `t`, only the state after scoring, or the
next interval? This should not be emergent behavior from cursor order.

My recommendation: **reactive interventions triggered by observations should
enqueue effects strictly after the observation boundary**, unless the DSL has an
explicit “immediate after observation” stage. Same-timestamp immediate effects
should be a separate stage, not mixed with pre-existing scheduled interventions.

### G. Reactive schedules need an agenda, not a static sorted vector

`all_intervention_times(model, params)` gives you a static list. That is not
enough for reactive SIAs where an observation changes intervention state, future
schedule, or parameters.

You probably want:

```rust
trait EffectAgenda {
    fn next_effect_time(&self, state: &StateView, params: &[f64], t: f64) -> Option<f64>;
    fn due_effects(&mut self, t: f64, state: &StateView, params: &[f64]) -> EffectBatch;
    fn observe_and_update(&mut self, obs: ObservationEvent, state: &StateView, params: &mut [f64]);
}
```

For non-reactive campaigns, this is just a precomputed sorted agenda. For
reactive campaigns, it can enqueue/cancel future effects after observations. For
inference, be careful: if the agenda depends on particle latent state, particles
may no longer share the same boundary sequence, which affects CRN assumptions.
If the agenda depends only on external observed data, the boundary sequence can
remain shared.

This is the place to make that distinction explicit.

## The target architecture I would aim for

I would split the system into four seams:

```text
Schedule / Clock
    Owns time, boundary ordering, exact/snap policy, cursor movement.

EffectAgenda
    Owns which events/interventions are due at a boundary.
    Can be static today, reactive later.

Lifecycle
    Owns canonical stage order and postconditions.
    Exposes fixed_step_substep(...) and boundary_only(...).

Kernel
    Owns math only:
      chain Euler-multinomial
      ODE RK4
      Gillespie SSA/PDMP
```

That gives you the consolidation you want without inventing a god abstraction.

The rule should be:

```text
Backends may differ in cadence and kernel.
Backends may not differ in effect ordering, firing due-ness, action semantics, or postcondition checks.
```

## My concrete recommendation

1. **Stop planning a lifecycle trait.** Build a closure-taking
   `fixed_step_substep` driver after the f64 effect seam lands.
2. **Introduce `StepClock` or equivalent.** Separate actual substep length,
   fire-grid resolution, and expression `dt`.
3. **Replace time-to-step firing inside apply with due effect batches.** Do not
   let `apply_interventions_at` rediscover due-ness.
4. **Fix the chain-binomial real-state synchronization issue before any tau
   fold.**
5. **Fix ODE event proposal so events read the true start-of-step continuous
   snapshot.**
6. **Add post-intervention/post-balance invariant checks.**
7. **Retire tau-leap only after extracting the shared Euler-multinomial kernel
   and proving chain-exact equivalence on edge fixtures.**
8. **Keep Gillespie, but narrow its “exact oracle” claim to models where
   frozen-propensity SSA is actually exact.**
9. **Design reactive campaigns as an `EffectAgenda`/boundary problem, not as
   another backend special case.**

Bluntly: the overall direction is good, and killing tau is the right call. The
main risk is declaring the lifecycle unified while `dt`, fire lookup, ODE event
snapshots, and real-state handling are still leaking through backend-specific
paths. Fix those seams now; they are exactly where reactive interventions and
inference will hurt you later.

---

# Round 2 — follow-up replies (naming, code quality, ambition, reactive)

*(Maintainer note: replies to a second round of questions. Decisions adopted are
tracked in [`../lifecycle-consolidation-todo.md`](../lifecycle-consolidation-todo.md);
our agreement is summarized at the end of this section.)*

## On the name: "boundary" is defensible, but `BoundaryKind` is not the best name

A "boundary" is right **from the integrator's point of view**: a time at which the
current integration interval must end — an intervention/output/observation/end time
is a **cut point** where the stepper has to stop, apply/record/score, then resume.

But a single time can be due for several reasons at once (output + observation +
intervention + end). So name the **point** and the **reasons** separately:

```rust
struct TimelineStop {
    t: f64,
    reasons: SmallVec<[StopReason; 4]>,
}
enum StopReason { Output, ScheduledEffect, Observation, End }
```

Preference: **`TimelineStop` + `StopReason`**. "Stop" makes the intent painfully
clear (the stepper must stop here); "reason" is stronger than "kind" because a stop
can have multiple reasons. Avoid **`Event`** (already overloaded: domain events,
always-active events, interventions, Gillespie reactions, observation events) and
**`TimeSlice`** (sounds like an interval; you need a point that terminates one).

Vocabulary: `substep`/`interval` = `[t0,t1]`; `timeline stop`/`boundary` = `t1`;
`stop reason` = why `t1` matters; `scheduled effect` = the action due at `t1`.

## Overall code quality

Good, but carrying serious architectural pressure. Not sloppy — comments are
unusually careful, invariants named, clear concern for byte-identical behaviour, CRN
coupling, RNG draw order, inference reuse, silent wrong answers. But it's at an
inflection point: several invariants are still held by convention and comments rather
than types. The big smell:

```text
Time stepping, due-time detection, effect firing, expression dt,
observation clipping, and backend cadence are still too entangled.
```

Summary: good kernel engineering, good awareness of inference hazards, good
test/golden instincts — but lifecycle and timeline semantics are not yet first-class
enough.

## Is supporting all these backends too ambitious?

Three **forward** kernels is ambitious but reasonable. All of them as **inference**
kernels would be too ambitious. Keep:

```text
chain-binomial: production stochastic + inference kernel (the centre of gravity —
                PGAS, PMMH, IF2, PF, correlated PF, densities, paired seeds,
                particle scratch, flow accumulators all orbit it)
ODE:        deterministic forward / sanity / large-N kernel
Gillespie:  small-count / validation / event-driven kernel
tau-leap:   fold into chain-binomial Exact policy
```

The `ProcessModel`/`DensityProcess` split is basically correct — PGAS needing
transition density is a real capability boundary. Forcing ODE/Gillespie to implement
that density surface for symmetry is abstraction theater. **Do not make this true:
"every backend supports every inference algorithm."** Keep the capability matrix
honest; use explicit capability errors for unsupported combinations.

## The two biggest architectural concerns

**1 — too many places answer "what time is it?" indirectly.** `Schedule::substep`
decides where the step stops, but `apply_interventions_at` re-derives due-ness via
`time_to_step(t,dt)`; `fire_steps` resolve on a `dt` grid; Exact may clip `dt`;
`EvalCtx.dt` may mean actual or grid `dt`; Gillespie carries an `iv_resolution_dt`
despite having no integrator `dt`. That is too much semantic load on raw `f64`.
Introduce a `StepClock` separating **actual numerical interval length**,
**schedule/fire-grid resolution**, and **expression `dt`**. "In public-health
simulation software, accidental equivalence is not good enough."

**2 — the schedule should return "what is due," and effect application should not
re-discover due-ness.** Move toward `agenda.due_effects(stop.t) -> EffectBatch` then
`apply_effect_batch(batch, …)`, so apply stops doing two jobs (decide due-ness +
apply). This also makes reactive campaigns much easier.

## Reactive campaigns are the real future stress test — three cases

- **Case 1 — exogenous reactive** (observed cases cross a threshold → campaign at
  t+14; all particles see the same observation): a shared agenda updated at
  observation times. Manageable.
- **Case 2 — parameter-dependent schedules** (`campaign_day = θ`): each PMMH
  proposal / IF2 particle may resolve a different schedule. Manageable, but a globally
  shared immutable schedule is only valid when times are *not* particle-specific.
- **Case 3 — latent-state-reactive** (`if this particle's latent prevalence > X →
  SIA`): each particle gets a different future schedule → the agenda becomes **part of
  particle state**; resampling must clone it, PGAS ancestor-tracing must account for
  it, and **CRN/paired-seed coupling breaks** (particles no longer walk the same
  boundary sequence). Supportable, but must be explicit:

```rust
enum AgendaScope { SharedExogenous, ParameterDependent, ParticleLocal }
```

**Do not let reactive campaigns sneak in as "just another intervention schedule" —
that will create silent wrong answers.**

## The smell to fix soonest: chain-binomial real-state

`step_one` takes `counts: &mut [i64]` but not the current `RealState`; inside, it
uses `scratch.real_s`, yet the run advances real state *outside* `step_one` and never
copies it into `scratch.real_s` before propensity/event evaluation (nor copies
`apply_post_advance`'s real mutations back out). So chain-binomial can read stale real
compartments inside the stochastic kernel — directly conflicting with declaring it
supports real compartments. **Bigger than the trait-vs-closure debate. Fix before
folding tau** (tau carries its real state through the loop more directly, so deleting
tau while chain's real path is stale may delete the better-behaved implementation).

## Ordering at a shared timestamp — declare it once

```text
1. finish process advance to t
2. propose/apply process events due at t
3. apply scheduled interventions due at t
4. balance / validate
5. output snapshot
6. score observation
7. reset accumulators
8. run reactive policy, enqueue future effects
```

Especially for observation-triggered interventions, "before scoring / after scoring /
next interval" is scientifically meaningful — it must not be emergent from cursor
order. Default: observations at `t` see state after pre-existing scheduled effects at
`t`; reactive effects *caused by* that observation are scheduled after scoring (a
separate explicit `PostObservationReaction` stage, not mixed with scheduled effects).

## What I would not do

- No big backend trait Gillespie must pretend to implement (it's not fixed-step).
- No generalizing inference over ODE/Gillespie yet — keep the trait spine
  chain-binomial-centred until a real second backend demands it.
- A closure-taking driver / explicit lifecycle function is cleaner now than a trait
  whose only shared content is the lifecycle order.

## Blunt verdict

Not too ambitious if the end state is: three honest kernels · one shared
timeline/stop vocabulary · one shared effect lifecycle · one chain-binomial-centred
inference kernel · explicit capability errors. It *is* too ambitious if: all backends
pretend to share one cadence · all pretend to support inference · all schedules are
assumed shared across particles · all `dt` meanings stay packed into one `f64`. The
code quality supports the project; the architecture needs one tightening pass before
reactive campaigns. The thing to get right is making *"why did time stop here, what is
due here, and in what order does it happen?"* a first-class object — ship
**`TimelineStop` / `StopReason`**.

---

## Maintainer response (adopted)

Agreed on substance throughout. **Adopted:** `TimelineStop`/`StopReason` for the new
type (Tier 3); the capability-matrix-honesty rule (three forward kernels, inference
chain-centred, no fake genericity); `StepClock`; the schedule-returns-due-effects
reshape; `AgendaScope` for the reactive future; and #3 as the first fix (it blocks the
tau fold). Sequencing — critical bugs (Tier 1) → the consolidation we'd already
designed (Tier 2) → the timeline tightening + v2 proposal (Tier 3) → reactive +
`EffectAgenda` (Tier 4) — is in
[`../lifecycle-consolidation-todo.md`](../lifecycle-consolidation-todo.md). The only
push-back is *pace*: the `StepClock`/`TimelineStop`/`EffectAgenda` reshapes each ride
on the CRN-draw-order and PGAS-density invariants the reviewer couldn't see, so each
lands behind a byte-identical A/B gate rather than in one pass.
