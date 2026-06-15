# Draining-event residual semantics (chain_binomial co-drain)

- **Status:** proposed
- **Fixes:** gh#217
- **Corrects:** `docs/dev/proposals/2026-06-06-scheduling-effect-topology.md` —
  its stated plan to converge every backend onto chain_binomial's start-of-step
  snapshot read; that direction universalizes the bug below.
- **Sibling:** gh#224 (PMMH/PF error-variant handling) — independent; this
  proposal removes the structural error, gh#224 stops the next one from hiding.

## Problem

When a compartment is drained by **both** a continuous transition **and** a
scheduled `events {}` action in the same step, `chain_binomial` overshoots and
aborts. `ode` and `gillespie` handle the same model correctly.

Reproduction (`event_transition_overshoot.camdl`, the gh#217 minimal case):

```camdl
compartments { A, B, I }
transitions { drain : A --> I @ r * A }
events       { dump  : transfer(fraction = 1.0, from = A, to = B) at [5] }
init { A = N0 }
```

```console
$ camdl simulate event_transition_overshoot.camdl --param r=0.1 --param N0=1000 --backend chain_binomial
error: simulation error: NegativeCount { compartment: "A", attempted_value: -54, t: 5.0, cause: BinomialOvershoot }
```

Structural, not a rare draw: seeds 1/2/3/7/42/99 abort with
`-54/-68/-56/-60/-65/-60` — always exactly `−(drain flow that step)`.
`--backend ode` and `--backend gillespie` exit 0.

### What should happen — continuous-time ground truth

The model approximates continuous dynamics. The `drain` hazard acts _over_ the
interval; the `dump` is a point operation _at_ t=5. ODE computes the correct
answer:

```
       drain acts continuously over the interval        dump fires AT t=5
t=4 ───────────────────────────────────────────────► t=5 ──────────►
A=670        A decays 670 → 607  (≈63 flow to I)        A=607  then  A=0
                                                        └─ dump moves 607 → B
Outcome:  I += 63,  B += 607,  A = 0.    Total conserved at 1000.
```

The `dump` is scheduled `at [5]`; it must act on the state that exists **at**
t=5, which already has the interval's recoveries removed. That post-transition
value is the **residual**.

### The candidate per-step semantics

Let `A_s` = A at the start of the step where the event fires; `d` = the drain
flow drawn that step (the repro's seed gives `d = 54`).

**① Drain-residual (ODE & Gillespie — correct).** Drain acts, then the event
sees what's left.

```
A_s ──drain──► A_s − d ──dump(1.0 × residual)──► 0
                │                                 │
                └─ I += d                         └─ B += (A_s − d)
fraction=1.0 = "move everyone still in A at t=5."   A_final = 0  ✓
```

**② Snapshot-fusion (current chain_binomial — the bug).** Both read `A_s`,
applied together.

```
drain takes d from A_s        ┐
dump takes 1.0 × A_s from A_s ┘  fused →  A_final = A_s − d − A_s = −d   ✗
The d individuals that left to I are removed a SECOND time by the dump.
d = 54  →  NegativeCount{A, -54}.
```

**③ Event-first (snapshot, event applied before the drain — coherent, wrong
timing).**

```
dump moves all A_s → A = 0   then drain acts on 0 → 0
Outcome:  B += A_s,  I += 0   — claims "nobody recovered this interval because the
campaign grabbed them all first"; mis-dates the campaign to the step's START.
```

**④ Competing-risks budget (treat the event as a rate competing with the
drain).**

```
partition A_s among {→I drain, →B dump, stay} via a multinomial.
fraction=1.0 is an instantaneous, infinite-hazard event → wins all competition
→ degenerates to ③ (B += A_s, I += 0). `fraction` has no meaning as a hazard.
```

## Decision

**Draining event actions read and act on the post-transition residual (①).
Inflow event actions keep start-of-step snapshot semantics. There is no separate
"competing event" mode — a process that should compete with transitions over the
interval is a transition.**

Concretely, by the action's effect on each compartment:

| Action effect                                          | Read-state                                                                                                                   | Rationale                                                                                                                                                  |
| ------------------------------------------------------ | ---------------------------------------------------------------------------------------------------------------------------- | ---------------------------------------------------------------------------------------------------------------------------------------------------------- |
| **Inflow** — `add(C, v)` (the inflow construct)        | start-of-step snapshot, fused with the transition apply (byte-identical to today)                                            | importation/births can't over-draw; preserves the `c1abb567` cohort-birth atomicity. A negative-amount `add` used as a drain is a known out-of-scope edge. |
| **Transfer** — `FractionTransfer` / `AbsoluteTransfer` | resolved wholesale against the post-transition residual: amount = `fraction × residual(from)` or `count.min(residual(from))` | the `from` side is the drain; matches ODE/Gillespie; `fraction=1.0` = "all survivors"                                                                      |
| **`set(C, v)`** — absolute assignment                  | applied post-transition, clamped ≥ 0                                                                                         | overwrites the post-dynamics value; assignment can't over-draw, but must see post-transition state to be meaningful                                        |

This is one coherent rule: the read-state differs by direction because the
non-negativity constraint only binds when an action _removes_.

## Why residual is the right choice (not a trade-off)

Three independent arguments converge:

1. **Temporal correctness.** An event `at [5]` must fire _at_ t=5 — on the state
   produced by dynamics up to t=5. ① is the only option that does so. ② is
   broken; ③/④ effectively fire it at the step's start, mis-dating a dated
   campaign by a full `dt` (at measles step sizes, `dt = 1 week`, a systematic
   bias, not a rounding wart).
2. **Epidemiological meaning.** A pulse campaign vaccinates a fraction of
   _whoever is still susceptible when it runs_. ② removes people who already
   left (nonsense); ③/④ claim the campaign preempts the whole interval's
   infections (a specific, usually wrong assertion). ① — "a fraction of the
   survivors" — is the plain reading.
3. **`fraction=1.0` is only well-defined under ①** ("everyone remaining"); under
   ④ it is degenerate.

**Why we don't also need ④.** If infection and a campaign should genuinely
compete for the same susceptibles over the interval, the campaign is a _rate_ —
write a transition (`vacc : S --> V @ vrate`), which shares the competing-risks
multinomial by construction and is properly bounded. The DSL's
event-vs-transition split already _is_ the two modes: events are point
operations on the instantaneous state (residual); transitions are competing rate
processes. We add no second event semantics.

## Implementation

Shared seam, so every cell inherits the fix (forward simulate + every
chain_binomial inference path routes through `chain_binomial::step_one`;
ODE/Gillespie already do ①).

1. **Classify each event action at compile time** as inflow / drain / set (one
   flag per action; `transfer` is drain on `from`, inflow on `to`). No runtime
   cost beyond a branch.
2. **`chain_binomial::step_one`** — split event application:
   - PROPOSE (unchanged): resolve _inflow_ deltas from the start-of-step
     snapshot (`scratch.int_s`) and fuse them into the atomic transition apply.
   - After ADVANCE applies the transition + inflow deltas to `counts`: resolve
     _drain_ and _set_ actions against `counts` (the post-transition residual)
     via the same `effects::resolve_event_batch` / `resolve_action` seam, with
     the residual `IntState` passed in instead of the snapshot. `fraction` ×
     residual; `count`.min(residual).
3. **`effects.rs`** — `resolve_action` already takes the snapshot it reads from;
   the change is _which_ state the caller passes for draining actions
   (post-advance), the same parameterization ODE already uses in
   `apply_boundary_batch_continuous`.
4. Order within a step: transitions (over interval) → inflow events (snapshot,
   fused) → drain/set events (residual) → interventions → balance. BALANCE
   unchanged (last).

Locus: `rust/crates/sim/src/chain_binomial.rs` (~:506–526 PROPOSE/fuse), backed
by `rust/crates/sim/src/effects.rs::resolve_event_batch`.

## Spec changes

1. **`docs/compartmental-ir-spec.md`** (§2.3 / §9.3, the lifecycle/boundedness
   home) — add the normative co-drain rule: event read-state by direction
   (inflow → snapshot; drain → post-transition residual; backends MUST agree on
   post-effect state), and a worked example (the repro). The competing-risks
   multinomial bounds transitions only.
2. **`docs/runtimes.md`** (§"Bounded competing risks") — one sentence closing
   the scope gap: the bound covers transition-vs-transition only; a draining
   _event_ on a transition's source reads the residual (it is not in the
   multinomial).
3. **`docs/camdl-language-spec.md`** §13.5 — note that draining events act on
   the post-transition residual (so `transfer(fraction=1.0)` = "all remaining"),
   with the co-drain worked example. (No doctest preambles here — edit by hand,
   do not `mdfmt` the language spec.)
4. **`2026-06-06-scheduling-effect-topology.md`** — correct the stated
   convergence direction: draining effects converge on post-transition reads
   (ODE/Gillespie), not chain_binomial's snapshot read.

## Testing (TDD)

1. **Red:** `tests/` case running the repro on `chain_binomial` asserting no
   error and `A = 0` at t=5. Fails today (`NegativeCount`), passes after the
   fix.
2. **Cross-backend agreement fixture** (the test-of-the-spec that was missing —
   every event-block test deliberately avoids co-drain): the repro model run on
   chain_binomial, ode, gillespie. Assert `A = 0` and conservation
   (`A+B+I = N0`) on all three; assert chain_binomial's E[B], E[I] over N seeds
   match ODE's within tolerance (chain_binomial splits B/I stochastically; ODE
   is the deterministic mean).
3. **Inflow non-regression:** a model with `add(S, S*0.1)` inflow + a drain on a
   _different_ compartment in the same step — assert the inflow amount is
   unchanged (snapshot-based), guarding against an over-broad "all events
   post-advance" change.
4. **PMMH loud (gh#224):** out of scope for this proposal, tracked separately;
   once gh#224 lands, add a test that a structural error surfaces loud under
   PMMH rather than collapsing to `-inf`.

## Non-goals

- A competing-risks _event_ mode (④) — use a transition.
- The PMMH/PF error-variant handling (gh#224) — independent; this proposal
  removes the structural error, gh#224 stops the next one from hiding.
