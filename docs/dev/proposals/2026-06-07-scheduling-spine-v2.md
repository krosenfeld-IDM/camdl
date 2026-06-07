# Scheduling spine v2 — timeline tightening

Date: 2026-06-07
Status: Proposed
Supersedes: the scheduling-effect topology map (`2026-06-06-scheduling-effect-topology.md`)
as the *target* architecture — that document mapped the terrain; this one specifies the
remaining reshapes against the now-landed Tier-1/2 implementation.
Out of scope (named siblings): inference real-state support; reactive interventions
(`EffectAgenda`); reactive parameters (gradient blocker). See "Out of scope".

## Two step lengths a reader must not conflate

A simulator substep has two lengths that are usually equal but mean different things.
The model declares a **nominal** step — `dt`, say 1 day. But the scheduler often has to
**stop short**: if an intervention or observation falls at *t* = 2.5 while `dt` = 1, the
substep starting at *t* = 2 is clipped to **0.5 day**, because the integrator must land
*exactly* on the effect/observation time. So at that substep:

- the **actual** length the integrator advanced is 0.5 — call it `dt_actual`;
- the **nominal grid** the model declared is still 1 — call it `grid_dt`.

They answer different questions, and conflating them is a silent wrong answer:

- *How much did the world change this substep?* — the stochastic noise that accumulated,
  the transition probability `1 − exp(−rate·dt)`, the gamma overdispersion `shape = dt/σ²`,
  **and the rate evaluation itself** — is over the **actual** elapsed 0.5. Everything
  numerical uses `dt_actual`.
- *Which scheduled step is this?* — mapping an effect's fire-time onto the model grid to
  decide if it fires now — uses the **nominal** `grid_dt`.

The chain-binomial kernel and the PGAS density **already** do exactly this (eval and physics
both on the clipped `dt_actual`; firing keyed on the base `dt`). The defect is that the
distinction is carried by hand-threaded conventions at each call site rather than by a type,
and one backend — tau-leap — evaluates rates on the nominal `cfg.dt` while drawing on the
clipped `dt`, an inconsistency that silently changes results for any model whose rate
references `dt`. Making the two lengths a first-class object is the spine of this proposal.

## What already landed (this builds on solid ground)

- **The merged `Schedule` spine** — `substep` / `substeps` / `window_end` / `drain_outputs`
  / `clip`, one time→step mapping with an explicit `StepPolicy { Snap, Exact }`
  (`schedule.rs`).
- **The effect-resolution purity seam** — `resolve_intervention`/`resolve_events`
  (`StateRef` → typed `Int`/`Real` deltas) + trivial `apply_effects` (`effects.rs`). This
  *resolves the i64/f64 dilemma* the design review framed as `CountStoreMut`: representation
  rides the delta type, the arithmetic has one home, ODE applies exact f64, events-on-real
  apply. `CountStoreMut` is absent from the code and not coming.
- **The shared post-advance lifecycle tail** — `lifecycle::apply_post_advance` already owns
  INTERVENE + BALANCE + the negative-count check in fixed order for chain / tau / gillespie.
- **Tier-1 correctness guards** — finite/negative effect values (`finite_action_value`), the
  off-grid event-misfire guard (`schedule::reject_event_misfire`), the real-coupled inference
  gate (gh#191).

So the spine, the effect layer, and the post-advance tail exist. What remains is making the
*timeline semantics* first-class instead of convention-held.

## Problem: three things are still held by convention, not types

1. **`dt` is overloaded** (the two lengths above) into one `f64` passed through the
   lifecycle. The discipline "use the clipped `dt` for physics + eval, the base `dt` for
   the firing key" is correct in the code today but *hand-maintained per call site*:
   `effects::resolve_events` computes the firing key as `time_to_step(t + dt, dt)`
   (`effects.rs:252`), and the Exact backends pass the base `cfg.dt` into it on purpose
   (`tau_leap.rs:316`, `apply_events_at` `intervention.rs:169`) so the key lands on the grid
   despite a clipped substep. A type would make that structural; a convention can be broken
   by the next edit. (The Tier-1 guard currently *rejects* the dangerous Exact + off-grid +
   always-active-event combination rather than running it correctly — its own doc-comment
   names `StepClock` as the full fix.)
2. **Due-ness is re-derived after the schedule already decided it.** `Schedule::substep`
   computes where the step stops, but `apply_interventions_at` re-discovers "what is due"
   via `time_to_step(t, dt)` against `fire_steps` (`intervention.rs:133`). Two mechanisms
   answer "what is due," and they can disagree under Exact / off-grid / parametric / close
   times.
3. **The within-substep order is only partly structural.** `apply_post_advance` shares the
   INTERVENE + BALANCE tail across chain/tau/gillespie, but the PROPOSE stage
   (`resolve_events`) is still called directly in each backend, ODE runs a separate
   continuous path (`apply_boundary_effects_continuous`), and the canonical order is named
   by the `// → FixedStepLifecycle` comments in `lifecycle.rs` rather than enforced by a
   driver.

## The design (types first)

### A. `StepClock` — name the two step lengths

```rust
struct StepClock {
    t0: f64,         // substep start (== rate/forcing evaluation time)
    t1: f64,         // substep end == the TimelineStop (below)
    dt_actual: f64,  // = t1 - t0. The realized substep. Physics + rate eval (EvalCtx.dt).
    grid_dt: f64,    // the nominal model dt. Fire-key resolution (time_to_step) ONLY.
}
```

The load-bearing decision, stated once: **`EvalCtx.dt = dt_actual`** (rate evaluation uses
the actual elapsed length, consistent with the noise/probability physics), and
**`time_to_step` keys on `grid_dt`** (scheduling uses the nominal grid). This is what chain
and PGAS already do, so threading `StepClock` is **byte-identical** for the discrete + ODE
forward backends and for the chain-binomial inference kernel — it codifies the existing
hand-threaded discipline. The only code whose numbers were ever on the *other* convention is
tau-leap (rates/σ² at `cfg.dt`), and that deviation is resolved when tau folds into chain (D
below). These `dt`s are numerical/grid — the calendar→time conversion (`docs/dates.md`) is
upstream and untouched.

### B. `TimelineStop` + `StopReason` — the schedule says where to stop and why

```rust
struct TimelineStop { t: f64, reasons: SmallVec<[StopReason; 4]> }
enum  StopReason  { Output, ScheduledEffect, Observation, End }
```

A single time can be due for several reasons (output + obs + effect + end). The `Schedule`
returns the next `TimelineStop`; the driver handles its reasons in one declared canonical
order. The effect application then consumes a **known due batch** instead of re-deriving
due-ness:

```rust
struct EffectBatch { intervention_idx: SmallVec<[usize; 4]>, event_idx: SmallVec<[usize; 4]> }

impl Schedule {
    fn next_stop(&self, cursor: &Cursor, t: f64) -> Option<TimelineStop>;
    // The due effects at a stop with a ScheduledEffect reason — read from the cursor's
    // effect position, NOT re-derived via time_to_step. Deterministic given the schedule.
    fn due_effects(&self, cursor: &Cursor, stop: &TimelineStop) -> EffectBatch;
}
```

`apply_interventions_at` is replaced by `apply_effect_batch(batch, …)` — application stops
deciding due-ness (it applies a list the schedule handed it) and does one job. The cursor
already holds the effect position (`effect_idx`); `due_effects` reads it, removing the
`time_to_step(t, dt)` re-derivation at `intervention.rs:133`, `effects.rs:252`, and
`effects.rs:382`. (Static schedule only here; a *reactive* `due_effects(t, state, params)`
that depends on latent state is the Tier-4 sibling.) Vocabulary, going forward (no churn to
existing prose): `substep`/`interval` = `[t0,t1]`; `timeline stop`/`boundary` = `t1`;
`stop reason` = why `t1` matters; `scheduled effect` = the action due at `t1`. ("Stop" over
"Event" — `Event` is already overloaded five ways; "reason" over "kind" — a stop has several.)

### C. The closure-taking lifecycle driver (D1)

```rust
fixed_step_substep(
    state, clock, due_effects, scratch,
    |snapshot, event_batch, current, scratch| {
        // backend-specific ADVANCE only (the kernel draw). MUST NOT consume RNG before it.
    },
)
```

The driver owns the canonical order — snapshot capture · event PROPOSE from snapshot ·
backend ADVANCE · atomic fusion of transition+event deltas · scheduled INTERVENE · BALANCE ·
postcondition checks. It already owns the tail (`apply_post_advance`); this folds in the
PROPOSE call (`resolve_events`) and routes ODE through the same order (ODE keeps its exact-f64
*apply*, but the *order* is shared). The backend closure implements **only** the kernel
advance and **cannot reorder stages**; the driver guarantees the snapshot is captured before
the first RNG draw and that no RNG runs between snapshot and the closure (the invariant that
keeps event PROPOSE — which is RNG-free — order-neutral w.r.t. the draws). No
`FixedStepLifecycle` trait (Gillespie can't honor a fixed-step advance; the only shared
content is the order — exactly when a closure beats a trait); the `// → FixedStepLifecycle`
comments are deleted. Gillespie keeps its boundary path (it already routes effects through
`apply_post_advance`); it shares effect application without pretending to have a substep
advance. Structure for an invariant currently held by comments; byte-identical, with a
per-backend A/B and an assertion that the closure receives the pre-draw snapshot.

### D. Drop tau-leap (D3) — fold into chain's `Exact` policy, adopting chain's conventions

tau-leap is the same Euler-multinomial kernel as chain under a different policy, **plus two
genuine convention differences**: it evaluates rates/σ² at `cfg.dt` not the clipped `dt`
(`tau_leap.rs:180`), and it skips transitions at `rate ≤ 0` where chain skips at
`rate ≤ RATE_EPSILON = 1e-15` (`tau_leap.rs:229` vs `chain_binomial.rs:23`). So the fold is
**not** a pure byte-identical equivalence — it makes the retired tau behaviour **adopt
chain's conventions**. The gate reflects that honestly:

- For the cases where chain+`Exact` and tau already agree, prove **byte-identical A/B**:
  integer-only · off-grid interventions · always-active events reading the source ·
  simultaneous event+intervention+output · overdispersed · deterministic draws · competing
  exits · ungrouped inflows · lineage observer on · inference stepping to off-grid obs ·
  **real compartments coupled into rates** (unblocked — #3 fixed, real-coupling tests
  landed).
- For the cases where they **provably differ** — tiny rates in `(0, RATE_EPSILON]` (different
  RNG consumption → different stream) and expressions referencing `dt` (eval at `cfg.dt` vs
  `dt_actual`) — pin **chain's chosen numbers with red→green tests**, with a one-line
  rationale for why chain's convention is the correct one. Do NOT assert a false "A == B" on
  cases that cannot be equal.
- The `balance + Exact` case is an **open design decision** (chain runs balance under Snap
  today; there is no balance-under-Exact semantics yet) — decide and document it, don't list
  it as a test to pass.

Then extract the one shared kernel under `step_policy`, route chain (`Snap`) and the retired
tau behaviour (`Exact`) through it, delete `TauLeapSim` + the CLI arm (no alias — house
policy), repoint goldens. **Blocked on A–C** (the fold needs `StepClock`'s `dt` decision and
the closure driver to be the single kernel host).

### E. `Target = Parameter` — the NPI axis (forward half)

`Action` gains a `{ Compartment | Parameter }` target; the resolver gains a `ParamDelta` peer
to `Int/RealDelta` (the `Arena { Int, Real }` dispatch in `resolve_action` admits an
`Arena::Param` additively), and `apply` writes the parameter arena. **Forward simulation
only.** A param effect inside a forcing/TimeFunc needs the gh#186 fix (params baked at
compile) or a compile-error guard. The inference + reactive halves are deferred: a mid-run
parameter change makes the PGAS/NUTS gradient inconsistent (the time-invariant-θ assumption;
see the effect-purity proposal's "Out of scope"). So: forward `set`/`scale` of a parameter at
a scheduled time, nothing more, this proposal. This step is additive and could ship
independently of A–D.

## Invariants (every reshape must preserve)

- **RNG draw order / paired-seed CRN** — A–C are byte-identical (verified by A/B gate, not
  just a golden pass); the tau fold (D) deliberately moves tau's numbers to chain's on the
  two divergent cases (pinned, not asserted-equal).
- **PGAS complete-data density + gradient** — `shape = dt_actual/σ²` and `p = 1−exp(−rate·dt_actual)`:
  the density's `dt` is `dt_actual`. `StepClock` routes `dt_actual` to physics + eval (what
  the density already uses), so the density is unmoved; the producer's source-group draw order
  is fixed.
- **i64 byte-identity** — the discrete backends stay byte-identical through A–C; only the
  tau→chain fold (D) moves tau's numbers (to chain's, proven/pinned), and Target=Parameter
  (E) is additive.
- **Capability matrix honesty** — three forward kernels (chain / ODE / Gillespie) after the
  fold; inference stays chain-binomial-centred; the `ProcessModel`/`DensityProcess` split
  stays. No backend becomes an inference kernel for symmetry.
- **Golden gates** — `gate_trajectory_baseline`, `gate_corner_case_baseline`,
  `gate_pgas_density_baseline`, `gate_inference_baseline`, the lifecycle audit set, **plus
  the two new oracle fixtures below**.

## Sequencing

**Step 0 — the missing oracles (before any reshape).** Today there is *zero* coverage of a
rate expression that references `dt`, and no clipped-substep density baseline: the `Expr::Dt`
node appears in no fixture, and `gate_inference_baseline` runs at `dt = 1` with integer obs so
nothing clips. Add (a) a corner fixture whose rate references `dt`, run by
`gate_pgas_density_baseline` at fractional dt under Exact; (b) an Exact-clipped IF2/PF baseline.
These are the oracles that prove `StepClock` (A) didn't move the inference path and that pin
the tau-fold (D) number-move. Cheap, highest-leverage, currently absent from the plan.

Then, each behind a byte-identical A/B gate:

1. **`StepClock`** — thread it through the lifecycle; `EvalCtx.dt = dt_actual`, `time_to_step`
   keys on `grid_dt`. Byte-identical (codifies chain/PGAS); the Step-0 oracles confirm it.
   Lets the Tier-1 off-grid guard be deleted (the rejected model class becomes correct).
2. **`TimelineStop` / `StopReason` + `EffectBatch`** — the schedule returns the next stop +
   the due batch; `apply_effect_batch` replaces `apply_interventions_at`'s due-derivation.
3. **Closure driver (D1)** — extract `fixed_step_substep`; fold PROPOSE in; route ODE's order
   through it; delete the `// → FixedStepLifecycle` markers.
4. **Drop tau (D3)** — the mixed gate above (byte-identical where equal; red→green chosen
   numbers where divergent); delete `TauLeapSim` + the CLI arm; repoint goldens. Blocked on
   1–3.
5. **Target=Parameter forward (E)** — additive; ships independently.

Steps 1–3 are byte-identical (Step-0 oracles + existing gates are the guard). 4 is the
number-moving one (tau adopts chain's conventions, pinned). 5 is additive.

## Out of scope (named siblings, not this proposal)

- **Inference real-state support** — carry + RK4-advance the real reservoir in `ParticleState`
  across PF/IF2/PMMH/PGAS, so real-coupled models can be *fit* (lifts the Tier-1 inference
  gate). CRN-sensitive (the reservoir joins the particle) and the PGAS density may need to
  account for it. Research-y; its own proposal.
- **Reactive interventions (`EffectAgenda`)** — `due_effects(t, state, params) → EffectBatch`
  where the agenda depends on *latent state* (resampling clones it, PGAS ancestor-tracing
  accounts for it, CRN breaks). The static `EffectBatch` here is the precursor; the reactive
  `AgendaScope` classification is Tier 4.
- **Reactive parameters** — blocked on the gradient time-invariance assumption (effect-purity
  proposal's "Out of scope").
