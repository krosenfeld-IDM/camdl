# Proposal: Human vs. Pathogen Migration in the Lineage Layer

**Status:** draft for discussion. Backed by a scoping experiment (§3) run before
drafting. **Scope:** represent each tracked individual's **deme trajectory**
(deme as a function of time) in the lineage layer, so that human migration
(individuals moving between strata) is handled as correctly as pathogen
migration (cross-patch transmission) already is. Plus paired golden examples and
a deferred design seam for the structured-coalescent migration term.

---

## 1. Summary

camdl's spatial models couple patches in exactly one way today: through the
**force of infection** (a contact/coupling matrix in the rate). That is
**pathogen migration** — patch _p_'s susceptibles are infected by patch _q_'s
infectives, and _nobody moves_. It is fully supported and tested in the lineage
layer (cross-deme transmission edges, per-stratum attribution).

**Human migration** — individuals physically relocating between strata
(`I[a] → I[b]`) — is a different process. The DSL already _compiles_ a
cross-stratum move transition, and the forward simulation + line-list recorder
_track it correctly at the event level_ (an individual's identity follows it
across demes; each event records the deme at that time). But the projection
layer collapses each individual to a **single, birth-time deme**
(`IndividualSummary.deme`, `tree.rs:389/393`), which:

1. mis-assigns migrants for **stratified sampling** (`Stratified::sample`,
   `tree.rs:499` — samples a migrant by where it was _infected_, not where it
   is), and
2. **corrupts the central scientific signal** distinguishing the two mechanisms
   (§3).

This proposal replaces the single deme with a **full per-individual deme
trajectory**, derived from the line-list events the runtime already records. No
DSL syntax is added (the explicit cross-stratum transition is enough). The deme
trajectory is also exactly what a future structured-coalescent **migration
term** needs, so we design for it now and defer the implementation.

### The result that motivates this (scoping experiment, §3)

Two 2-patch SIR models — **P** (pathogen migration: cross-patch FoI coupling)
and **H** (human migration: local transmission + infectives move) — calibrated
to **closely matched incidence** (both ~92–94% final attack rate, patch-b ~46%,
peaks ~25–36 d) produce **structurally opposite genealogies**:

|            | final size | patch-b | peak day | cross-deme transmission frac |
| ---------- | ---------- | ------- | -------- | ---------------------------- |
| P, κ=0.005 | 9229       | 4618    | 31       | 0.011                        |
| P, κ=0.02  | 9316       | 4654    | 28       | 0.040                        |
| P, κ=0.05  | 9423       | 4724    | 25       | 0.092                        |
| H, m=0.005 | 9258       | 4638    | 36       | **0.000**                    |
| H, m=0.02  | 9205       | 4603    | 30       | **0.000**                    |
| H, m=0.05  | 9284       | 4646    | 29       | **0.000**                    |

Incidence cannot distinguish them; the **genealogy can** — Model H has _exactly
zero_ cross-deme transmissions (all spread is local; mixing happens by movement,
which is not a transmission edge), while Model P always has some. This is the
canonical phylodynamic value proposition made concrete, and the same "genealogy
breaks an identifiability degeneracy" thread as the seed-timing proposal's §8.

**And the fix is load-bearing for the lesson:** computed with the current
birth-deme behavior, Model H (m=0.05) spuriously shows a **0.143** cross-deme
transmission fraction (a migrant born in _a_, moved to _b_, transmitting locally
in _b_, is mis-scored as an _a→b_ transmission). Only the deme-trajectory
representation keeps H ≡ 0.

---

## 2. Background: two mechanisms, two genealogical signatures

**Pathogen migration (Model P).** Cross-patch coupling lives in the _rate_:
`importation[p,q] : S[p] → I[p] @ κ·W[p,q]·S[p]·I[q]/N[q]`. The infective in _q_
never moves; the _force of infection_ crosses. In the genealogy, a lineage's
deme is constant over an individual's infectious life, and **deme changes occur
at transmission nodes** (a _q_-infective infects a _p_-susceptible → a
cross-deme edge). Cross-deme transmission fraction > 0.

**Human migration (Model H).** Transmission is _local_
(`infection[p] : S[p] →
I[p]`); individuals move
(`migration[p,q] : I[p] → I[q] @ m·I[p]`). A lineage changes deme **along a
branch** (mid-infection), and every transmission is within-deme. Cross-deme
transmission fraction = 0; the structure is carried by **migration events on
branches**, decoupled from coalescence.

These are genealogical opposites, and the distinction is precisely what the
structured coalescent formalizes (migration as a branch-wise process distinct
from coalescence). Representing it correctly is the foundation for both the
teaching example and the eventual migration-aware inference likelihood.

---

## 3. Scoping experiment (run 2026-05-21, before drafting)

Models (2-patch SIR, `β=0.5`, `γ=0.2`, `S[a]=4990, I[a]=10, S[b]=5000`, 150
days, chain-binomial dt=1, 3 seeds averaged):

- **P**: `infection[p]` (local, `#[lineage]`) + `importation[p,q]` (cross-patch
  FoI, `#[lineage]`) + `recovery[p]`.
- **H**: `infection[p]` (local, `#[lineage]`) + `recovery[p]` +
  `migration[p,q] : I[p] → I[q] @ m·I[p] where p≠q`.

Findings (table in §1):

1. **Incidence is matchable.** Across κ (P) and m (H), final size, patch-b
   attack rate, and peak timing are closely comparable — the two mechanisms are
   confounded by case data.
2. **Trees diverge decisively.** Cross-deme transmission fraction (computed from
   the line-list **event-time** demes — `parent_deme` is the infector's
   _current_ deme, `deme` is where the infection occurred): P grows with κ
   (0.011 → 0.092); **H is exactly 0 at every m**. Structural, robust.
3. **The birth-deme bug corrupts (2).** Re-scoring H (m=0.05) with each
   individual's _birth_ deme — the current `IndividualSummary.deme` — yields a
   spurious 0.143 cross-deme fraction. The deme-trajectory fix is required for
   the signal to be correct.

The experiment scripts/models are the basis for the paired goldens (§7).

---

## 4. Current state (verified against the code)

**(C1) Pathogen migration: supported + tested.** The contact-matrix /
importation pattern; the lineage classifier emits per-stratum `parent_pools`,
and realize samples the infector from the correct deme. `spatial_lineage`
fixture + `lineage_stratified.rs` (Tier-2b asymmetric-matrix attribution).

**(C2) Human migration: forward + line-list correct; projection wrong.**

- The DSL compiles `migration[p,q] : I[p] → I[q] where p≠q` (a cross-stratum
  move; verified). The expander emits a `RouteInfo` with
  `source_deme ≠
  destination_deme`.
- `realize` handles it: the progression arm removes the ID from the
  `(source_deme, I)` pool and pushes it to `(destination_deme, I)`, recording
  the **same individual** with `parent_kind = none` (not a transmission) and
  `deme =
  destination` (`realize.rs`). An individual's event sequence is
  therefore its deme trajectory. **The per-event `deme`/`parent_deme` columns
  are event-time correct.**

**(C3) The collapse to birth deme.** `IndividualSummary` carries a single
`deme`, set at the individual's **earliest** event and never updated
(`tree.rs:389,393`). Consequences:

- `Stratified::sample` (`tree.rs:499`) samples a migrant at its _birth_ deme's
  rate — wrong: surveillance samples people where they are, not where they were
  infected.
- Any tree coloring / per-deme statistic built from `IndividualSummary.deme`
  mis-labels migrants, producing the spurious cross-deme fraction in §3(3).

**(C4) Inference-side migration term: not built.** The structured-coalescent
likelihood `p(T | x)` is future work (seed-timing §8); its migration term is
explicitly deferred. It will require deme-at-time along lineages — i.e. exactly
the trajectory this proposal introduces.

---

## 5. Design: the deme trajectory

### 5.1 Representation

Replace the single `deme` on `IndividualSummary` with a **deme trajectory**: the
ordered sequence of `(time, deme)` an individual occupies, derived from its
line-list events.

```rust
/// An individual's deme over time: (entry_time, deme), sorted ascending.
/// `segments[0].0` is the individual's birth/infection time; each later entry
/// is a migration. A non-migrating individual has exactly one segment.
pub struct DemeTrajectory { segments: Vec<(f64, DemeId)> }

impl DemeTrajectory {
    pub fn birth_deme(&self) -> DemeId;        // segments[0].1
    pub fn deme_at(&self, t: f64) -> DemeId;   // last segment with entry_time <= t
}

pub struct IndividualSummary {
    pub id: IndividualId,
    pub infection_time: f64,
    pub removal_time: Option<f64>,
    pub trajectory: DemeTrajectory,   // replaces `deme`
    pub never_removed: bool,
}
```

**Derivation (no schema change to the line list).** The runtime already records
the deme at every event. In `summarize`, accumulate each individual's events in
time order; each event's `deme` is a trajectory segment. A migration event
(`parent_kind = none`, destination deme ≠ current) appends a segment; an
infection sets `segments[0]`; recovery/removal sets `removal_time`. Backward
compatible for the common case: a non-migrating individual has a one-segment
trajectory and `birth_deme()` reproduces today's `deme`.

> **Why full trajectory, not just sampling-time deme** (decision taken): the
> sampling-time deme alone fixes stratified sampling, but the structured
> coalescent (C4) needs deme-at-arbitrary-time along each lineage, and correct
> tree coloring needs the change-points. The trajectory subsumes both and is
> free to derive (the data is already in the line list).

### 5.2 Consumers

- **`Stratified::sample`** uses `trajectory.deme_at(sampling_time)` — the deme
  the individual is in when sampled (its removal time, or the horizon). This is
  the C3 fix.
- **Tree coloring / Newick deme annotation** uses the trajectory (a tip's deme
  is its deme at the sampling time; branch deme is piecewise).
- **Cross-deme transmission statistic** (§8) is computed from the line list's
  event-time `parent_deme` vs `deme` — already correct, formalized as a first-
  class projection so the chapter and tests don't hand-roll it.

### 5.3 What does _not_ change

- The line-list schema (events already carry per-event deme).
- `realize` / the recorder (already event-time correct).
- The DSL (no migration sugar — see §6).
- Non-migrating models: byte-identical line lists and one-segment trajectories;
  `Stratified` behaviour unchanged where nobody moves.

---

## 6. DSL: no new syntax (decision taken)

Human migration is expressed with the explicit cross-stratum transition that
already compiles:

```camdl
migration[p in patch, q in patch] : I[p] --> I[q]  @ m * I[p] where p != q
```

No movement-matrix sugar (parallel to the contact matrix `C[p,q]`) for v1. A
movement-matrix `M[p,q]` rate table is expressible today via `tables {}` +
`@ m * M[p,q] * I[p]`, so the sugar would be pure convenience; deferred.

---

## 7. Paired golden examples + calibration methodology

Commit two calibrated goldens (the §3 models, finalized):

- `model_pathogen_migration` — cross-patch FoI coupling (κ).
- `model_human_migration` — local transmission + infective movement (m).

**Calibration recipe (documented with the fixtures).** Fix `β, γ` (same local
R₀). Tune κ and m so the two match a chosen observable — patch-b attack rate and
peak timing (§3 shows κ≈0.01 vs m≈0.005 land within a few % on both). The
goldens pin one matched operating point; the chapter sweeps around it. Both seed
patch a only (`I[a]=10`, patch b naive), so b's epidemic is _entirely_
import/migration- driven — making the seeding-mechanism contrast the whole
story.

> **Scope note for the chapter:** v1 moves infectives only (`I[a]→I[b]`), the
> minimal model that produces deme-change-along-branch. Moving S and R too is
> more realistic but adds transitions without changing the qualitative lesson;
> note it as an extension.

---

## 8. Tree statistics that reveal the difference

Add these as first-class line-list/tree projections (so tests and the chapter
share one correct implementation):

1. **Cross-deme transmission fraction** —
   `#{transmissions: parent_deme ≠ child
   deme} / #transmissions`, from
   event-time demes. The decisive statistic (P > 0, H = 0). _Note: must use the
   trajectory's deme-at-transmission, never birth deme — that is the §3(3) bug._
2. **Migrations per lineage** — count of migration events (deme changes on
   branches). H > 0, P = 0. The mirror image of (1).
3. **Deme–topology association** — a parsimony / association-index score of deme
   labels on the pruned tree (lower for H, where clades are deme-mixed by
   movement; structured for P). Optional, richer.

(1) and (2) together are the clean headline: **P moves the pathogen at
branching; H moves the host along branches.**

---

## 9. Deferred but drafted: the structured-coalescent migration term

Not implemented here; designed-for. The structured coalescent likelihood
`p(T | x)` (seed-timing §8) decomposes per time interval into a **coalescent**
rate (∝ 1/I_deme(t)) and a **migration** rate between demes. The migration term
needs, per lineage, **deme-as-a-function-of-time** — exactly `DemeTrajectory`.
Two model-faithful sources of the migration rate:

- **Human migration**: the migration rate is the model's movement rate
  `m·M[p,q]` — a _mechanistic_ migration term, not a free phylogeographic
  parameter.
- **Pathogen migration**: there is no host movement; "migration" of a lineage
  between demes happens only at transmission, so the structured-coalescent
  migration term is **driven by the cross-patch transmission rate**, not a
  movement rate.

So the two mechanisms feed the _same_ likelihood through _different_ rate
channels — which is why the trajectory representation (this proposal) is the
shared substrate, and why getting it right now unblocks migration-aware
inference later without rework. Detailed likelihood design is out of scope; the
commitment here is that `DemeTrajectory` is the boundary the term will read.

---

## 10. Implementation plan (incremental commits)

1. **`DemeTrajectory` + `summarize`** — derive trajectories from line-list
   events; `IndividualSummary.deme` → `trajectory` with `birth_deme()` /
   `deme_at()`. Byte-identity guard: non-migrating fixtures produce one-segment
   trajectories; existing tree/sampling tests unchanged.
2. **`Stratified` sampling fix** — sample at `deme_at(sampling_time)`. Add a
   migration fixture test showing a migrant sampled at its _current_ deme.
3. **Tree statistics** — cross-deme transmission fraction + migrations-per-
   lineage as first-class projections (CLI `lineage` subcommand or a documented
   column), from event-time/trajectory demes.
4. **Paired goldens + `lineage_migration` test** — assert P > 0 vs H = 0 cross-
   deme fraction, and the birth-deme-vs-trajectory contrast as a regression
   guard.
5. **(Deferred)** structured-coalescent migration term — separate proposal/PR.

Each commit: `cargo test` green, byte-identity on non-migration fixtures, and
the migration-specific assertions.

## 11. Evaluation

- Non-migration models: byte-identical line lists; one-segment trajectories;
  `Stratified` unchanged.
- Migration model: a migrant's trajectory has ≥2 segments; `deme_at(sampling)` =
  its post-migration deme; stratified sampling applies the _destination_ deme
  rate.
- Paired goldens: cross-deme transmission fraction P > 0, H = 0 (the §3 result),
  stable across seeds; birth-deme scoring of H is non-zero (regression guard for
  the bug).

## 12. Open questions

1. **Statistic surfacing** — new `camdl lineage stats` subcommand vs extra
   columns on the line list vs book-side computation? (Recommend a small
   `lineage` projection so tests and chapter share one implementation.)
2. **Tip deme when never removed** — use `deme_at(horizon)` (the last known
   deme). Confirm this matches the sampling-time convention already used for
   removal time.
3. **Moving S/R** — keep v1 to infective movement, or include S/R movement in
   the golden (more realistic, more transitions)? (Recommend infective-only v1.)
