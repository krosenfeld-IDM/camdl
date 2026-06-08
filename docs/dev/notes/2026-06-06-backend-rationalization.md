# Backend rationalization: which simulation backends camdl needs, and what each costs the shared surface

Status (2026-06-07): tau-leap dropped — scheduling-spine-v2 §D / Step 4.

Date: 2026-06-06
Project: camdl
Tags: backends, scheduling, simulation, hazard-surface, consolidation

## Context / question

camdl has four simulation backends — chain-binomial, tau-leap, ODE, Gillespie —
and the scheduling/effect-consolidation work
([scheduling-effect topology](../proposals/2026-06-06-scheduling-effect-topology.md))
is about giving them a shared timeline substrate and a shared within-substep
lifecycle. Each backend that participates is another copy of the lifecycle that can
drift (the `M1` event/intervention inversion is exactly that drift across the four
copies). So the natural question: **does every backend earn its keep, or does cutting
one reduce the hazard surface enough to be worth it?**

This note answers that generally — the role of each backend, what it costs the
*shared* surface, and the one cut worth making — so the decision is reusable beyond
the current proposal.

## The reframe: hazard lives in lifecycle duplication, not backend count

The instinct "drop a backend to reduce risk" mostly mis-locates the hazard. The
bug-prone thing is not that four backends *exist*; it is that the within-substep
effect lifecycle is **hand-rolled four times**, so the four copies can disagree
(and do — `M1`). Once a single `apply_effects_in_lifecycle_order(snapshot, current,
effects)` is extracted and every backend routes through it, the inversion class
disappears *regardless of how many backends exist*. So:

- The primary de-risking lever is the **shared lifecycle extraction**, not the
  backend count.
- Dropping a backend *before* that extraction saves that backend's share of the
  extraction work; dropping it *after* saves only ongoing kernel maintenance.
- A backend is therefore worth keeping iff its **kernel** carries capability no
  other backend does. The lifecycle is shared either way.

With that lens, the question becomes: which kernels are genuinely distinct, and which
is redundant?

## The kernel-equivalence finding (verified)

**tau-leap's kernel is byte-for-byte chain-binomial's** — it is *not* classic
unbounded-Poisson τ-leaping. Both backends draw, per source-compartment group, a
binomial number of total exits at the competing-risks probability `1 − e^{−rate·dt}`,
then split that total multinomially across the competing transitions, with a gamma
multiplier for overdispersion and Poisson/negative-binomial only for *ungrouped*
transitions.

```
$ rg -n 'rng\.binomial|p_total|p_split|Match chain-binomial' chain_binomial.rs tau_leap.rs
chain_binomial.rs:378:  let (p_total, _q) = prob_q_from_rate_dt(total_rate, dt);   # 1 − e^{−rate·dt}
chain_binomial.rs:398:  rng.binomial(n_src as u64, p_total)                        # total exits
chain_binomial.rs:415:  let c = rng.binomial(n_events, p_split);                   # multinomial split
tau_leap.rs:172:  //  Match chain-binomial's Euler-multinomial:                    # explicit comment
tau_leap.rs:219:  let mut n_events = rng.binomial(n_src as u64, p_total);          # identical draw
tau_leap.rs:227:  let c = rng.binomial(n_events, p_split);                         # identical split
```

The differences between the two are *not* in the kernel:

| Axis | chain-binomial | tau-leap |
| --- | --- | --- |
| transition draw | binomial competing-exit + multinomial split | **identical** |
| step policy | `Snap` (full `dt`, effects via `fire_steps` in `step_one`) | `Exact` (clips to boundaries, effects via `apply_interventions_at`) |
| `BALANCE` capability | yes | no |
| inference (`ProcessModel`) | yes — the only backend | no (forward only) |

So tau-leap ≈ **chain-binomial running `StepPolicy::Exact`**. Its sole behavioural
differentiator — landing exactly on off-grid boundaries under stochastic dynamics —
is now a `Schedule` policy knob, not a separate algorithm. (`Snap`/`Exact` and the
`fire_steps`-vs-`apply_interventions_at` firing mechanism are exactly what the
topology work unifies.)

## The four backends

| Backend | Kernel | Distinct math? | Inference? | Unique capability | Verdict |
| --- | --- | --- | --- | --- | --- |
| **chain-binomial** | Euler-multinomial (binomial competing-exit) | reference | **yes** (the only one) | production engine + `balance` | **keep — it is everything** |
| **tau-leap** | *same* Euler-multinomial ("match chain-binomial") | **no — identical** | no | "exact boundaries" = now a `StepPolicy` | **retire → fold into chain-binomial `Exact`** |
| **ODE** | RK4, deterministic, `f64` | yes | reserved (trajectory-match) | deterministic limit, large-`N`, smooth gradient | keep |
| **Gillespie** | exact SSA, event-driven, `i64` | yes | no | exact small-count dynamics + the validation oracle | keep |

Why ODE and Gillespie earn their keep, in epidemiological terms:

- **ODE** is the deterministic skeleton. You reach for it constantly — large
  populations where stochasticity is negligible and the stochastic backends are
  slow, quick sanity checks, and (the reserved seam) gradient/sensitivity inference
  that the integer backends cannot give. It is the *only* `f64` backend, which is
  also the source of the one real type break (below) — but the break is contained.
- **Gillespie** is the exact, event-driven gold standard. It is correct in the
  small-count regime — early outbreak, importation, extinction — where chain-binomial's
  `dt`-discretization introduces an `O(dt)` rate-freezing bias, and it is the oracle
  you validate the fixed-step backends against as `dt → 0`. Dropping it would remove
  both the small-count capability and the validation reference.

## Where unification stops, and why

| Shared abstraction | Unifies across | Holdout | Binding reason |
| --- | --- | --- | --- |
| Time substrate (`Schedule`) | all | — | landed |
| **Lifecycle order/apply (`Stage`)** | **all 4** | — | pure fn of `(snapshot, current, effects)`; Gillespie calls it at a `clip` boundary |
| Substep **cadence** | chain-binomial, ODE | **Gillespie** | event-driven — no "substep" unit; uses `clip`, not `substep` |
| Effect **application** (`Action`) | i64 backends | **ODE** | `Action` is `i64`-typed, ODE is `f64` → the `{IntDelta\|RealDelta}` apply-seam |
| Balance (`Constrain`) | chain-binomial | the rest | residual-compartment needs a substep + integer end-state |
| Scoring (`log_likelihood`) | all 4 algorithms | — | landed |

The headline: the **lifecycle order/apply unifies across all four backends**,
including Gillespie (which invokes it at a `clip` boundary rather than on a substep
grid). The only thing that does *not* unify is the substep **cadence**, and that is
Gillespie's single, honest holdout.

## Hardness that survives the unification

After the shared lifecycle is extracted, what intrinsic difficulty remains per
backend?

| Backend | Residual hardness |
| --- | --- |
| chain-binomial | none — it is the reference |
| **tau-leap** | none — but **redundant** (same kernel as chain-binomial) → pure cost |
| **ODE** | one bounded fix: the `f64/i64` apply-seam (the deepest *type* break, but contained) |
| **Gillespie** | **permanent semi-separation**: event-driven cadence, `clip` not `substep`, propensity-recompute after any mutation |

So **Gillespie is the hardest to reconcile** — and stays that way — but its hardness
is *quarantined* behind `clip`: it opts out of the shared cadence rather than
polluting it. **tau-leap is the most disproportionate cost**: it is a third copy of
the lifecycle for a kernel that is already chain-binomial's. **ODE's break is real
but one-and-done** (the apply-seam closes it).

## Recommendation

1. **Retire tau-leap as a separate backend; fold it into chain-binomial running
   `StepPolicy::Exact`.** Capability is preserved (the kernel is identical; the
   exact-boundary behaviour becomes a policy). This is the only backend cut that is
   pure win.
2. **Do it as a consequence of, and a test for, the unification** — not as a separate
   pre-step. The fold's gate is the proof `chain-binomial + Exact == tau-leap`
   byte-for-byte on the corpus; passing it both validates the StepPolicy/lifecycle
   work *and* earns the deletion. Then repoint goldens and remove the backend (alpha:
   surface changes are allowed; backwards-compatibility is a non-goal).
3. **Keep chain-binomial, ODE, Gillespie** — three genuinely-distinct kernels.
4. **End state: three kernels calling one lifecycle**, splitting only on cadence
   (fixed-step: chain-binomial, ODE / event-driven: Gillespie) and on the `f64`
   apply-arm (ODE). That is the minimal honest surface.

## Caveats to verify before deleting tau-leap

- tau-leap declares `LINEAGES` and `REAL_COMPARTMENTS`; confirm chain-binomial
  under `Exact` covers both (it declares them too — verify the `Exact` path
  exercises them).
- chain-binomial fires interventions via `fire_steps` inside `step_one` (Snap),
  tau-leap via `apply_interventions_at` at the clipped boundary (Exact). The
  byte-identical fold depends on the Layer-2 shared apply making these the *same*
  firing under `Exact`; the equivalence proof is therefore downstream of the
  lifecycle extraction, not independent of it.
- `balance` is chain-binomial-only and currently Snap-coupled; folding means
  chain-binomial-`Exact` must also support `balance`, or `balance` + `Exact` is a
  declared-unsupported combination.

## Next

Fold the conclusion (the four tables + the retire decision) into the topology
proposal's executive summary; carry the full reasoning here. Sequence the tau-leap
retirement onto the StepPolicy/lifecycle work as its validation gate.
