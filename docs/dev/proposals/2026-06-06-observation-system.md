---
date: 2026-06-06
status: superseded by 2026-06-10-observation-data-entry-dsl.md
superseded_by: 2026-06-10-observation-data-entry-dsl.md
supersedes: 2026-06-05-observation-data-binding.md
related:
  - 2026-06-06-scheduling-effect-topology.md
  - 2026-05-14-reactive-interventions-and-evsi.md
area: observation data loading / inference
issue: gh#171, gh#172, gh#98, gh#134
---

# Observation system

## Scope: the data layer, on top of the timeline spine

The observation surface splits into two layers that were previously entangled, and
the split is what lets this land cleanly:

- **The temporal layer** — reconciling observation *times* with integrator steps —
  is owned by the
  [scheduling-effect topology](2026-06-06-scheduling-effect-topology.md). That work
  ships the `Observe` effect (a read-only `&State` projection at an obs boundary),
  first-class `TemporalKind { Interval, Instant }`, the per-stream
  `ResetWindow{ flow_indices }` (the `Stage::Reset` accumulator close), the
  `StepPolicy { Snap, Exact }` off-grid reconciliation, and the runtime sub-`dt`
  collision guard. Anything that is a per-substep state read/write or a time→step
  mapping lives there.
- **The data layer — this proposal** — turns untyped data rows into a
  model-shaped, fully-typed object, and routes every fitting algorithm to consume
  *one* such object instead of each re-loading and re-checking `--data`. Anything
  that is *parsing and validating rows into model cells* lives here, and it touches
  no substep.

The two meet at exactly one mechanical mapping (below): each bound stream becomes one
`Observe` effect, and each interval stream contributes one `ResetWindow`. The data
layer is independent of the timeline and can proceed in parallel with the topology
implementation; only the final union-axis scoring change waits on the per-stream
`ResetWindow` the topology work ships.

## Framing: bind, not join

Loading observation data is not a symmetric **join** (two co-equal relations, the
result the union of both schemas, neither side privileged). It is an asymmetric,
directional **bind**: the model *defines* a fixed lattice of named cells — which
streams exist, which strata, which times — and data values are bound *into* those
named slots, like binding arguments to parameters. The framing privileges the
model's lattice as the authority, and that changes the semantics in exactly the way
a correctness surface needs:

- a **leftover** (a data row with no cell — a typo'd stream, a stratum the model
  lacks, an off-grid time) is "data I have nowhere to put" → usually a mistake;
- a **hole** (a cell with no data) is "this slot got nothing" → often *expected*
  (sparse surveillance).

A symmetric join collapses both into one "unmatched" bucket; a bind keeps the two
directions distinct, with distinct severities. And the result type differs: a join
yields a union of two tables; a bind yields a **model-shaped object with typed holes**
(`Option` cells) — the sparse-geometry representation `gh#171` needs.

## Why this is a correctness surface

A data point that fails to reach scoring — silently — is a wrong likelihood, hence a
wrong posterior. The bind exists to make every malformed input a *named, located*
outcome. Three residual gaps motivate it:

- **NaN/inf are unguarded on the obs value path.** `pfilter.rs:669,699` parse values
  with bare `parse::<f64>()`, which accepts `"NaN"`/`"inf"`; nothing checks
  `is_finite` before the value reaches the log-pmf.
- **Loading is scattered and duplicated.** The
  `--data → per-stream series → MultiStreamObsModel` pipeline is re-implemented in
  `pfilter.rs`, `profile.rs`, `fit/runner.rs`, and `survey.rs` — each resolves
  `--data`, builds its own per-stream series, replicates the shared-grid check
  (≥5 sites), and canonicalizes `observations = per_stream_obs[0]`. That duplication
  is where silent drops and the homogeneity asserts live.
- **Holes cannot be expressed.** Every stream shares one dense `obs_times`, so
  sparse/multi-cadence surveillance (`gh#171`) is rejected, not represented.

(The sub-`dt` collision drop in `build_obs_at_substep` — two obs rounding to one
substep, last-wins — is a *temporal* hazard the topology work's runtime collision
guard closes; the data layer's `Collision` finding catches the load-time version.)

## The data-layer types (`obsdata`)

Data enters as untyped rows and leaves as a value the inference traits already
consume. Nothing downstream is new; the work is to give the existing scoring seam a
single, typed input.

```rust
mod obsdata {
    // ── input: one untyped row per PRESENT observation ──
    struct LongRow { stream: String, stratum: Option<String>, when: RawTime, value: RawValue }
    enum RawTime  { Offset(f64), Date(String) }      // resolved via ir::caltime + model origin
    enum RawValue { Num(f64), Missing, Unparseable(String) }

    // ── output: a model-shaped, fully-typed object ──
    /// PRIVATE ctor — only `bind` constructs one, so no un-validated data can reach
    /// the likelihood. Every leftover/collision/hole is accounted for in a
    /// BindReport before any value reaches scoring.
    pub struct BoundObs { times: Vec<f64>, streams: Vec<StreamCells> }
    struct StreamCells {
        name:  String,
        kind:  TemporalKind,            // the SAME type the runtime carries on Observe
        cells: Vec<Option<ObsCell>>,    // None = hole; one slot per time in `times`
    }
    /// `Scalar` is the common case; `Counted` carries a per-observation denominator
    /// — a Binomial/BetaBinomial n that varies survey-to-survey (the malaria case).
    enum ObsCell { Scalar(f64), Counted { value: f64, denom: f64 } }

    // ── the report: errors are VALUES, not control flow ──
    pub enum Severity { Error, Warn, Info }
    pub struct Finding { kind: BindIssue, stream: String, detail: String, count: usize, severity: Severity }
    pub enum BindIssue {
        LeftoverColumn, LeftoverStratum, OffGridInterval, OffGridInstant,
        Collision, Duplicate, CoarserThanModel, Hole, RejectedValue,
        UnparseableDate, InconsistentTimeColumn,
    }
    pub struct BindReport { findings: Vec<Finding>, verdict: Severity }

    pub fn bind(model: &Model, rows: Vec<LongRow>, dt: f64, cal: &CalendarCtx, policy: &BindPolicy)
        -> (BoundObs, BindReport);   // never panics, never exits — errors are VALUES (gh#181)
}
```

`TemporalKind` is **not** declared here — it is imported from the topology work,
where it is a runtime type carried on the `Observe` effect. The loader *chooses* the
variant from the stream's projection; the runtime *enforces* its semantics. One type,
two responsibilities, no duplicate definition.

## The bind as a cardinality map

`bind` is a partial map `φ : DataRow → ModelCell`, cell = `(stream, stratum, k)`.
Every failure is a departure from "injective and total," with a defined resolution
and a severity that splits by *direction*:

| cardinality | cause | resolution |
| --- | --- | --- |
| 1:1 | — | clean |
| many:1 (non-injective) | `dt` coarser than data, or duplicate row | `Collision`/`Duplicate` → **Error**; `--aggregate=sum\|mean` opt-in (loud, changes likelihood) |
| 1:many | data coarser than model (region vs district) | needs a model aggregate cell (`CumulativeFlowSum`); else `CoarserThanModel` → **Error** |
| 0:1 | cell with no data — a hole | `None` cell; `Hole` → **Info** (sparse) / **Warn** (stream declared dense) |
| 1:0 | data with no cell — a leftover | `LeftoverColumn`/`LeftoverStratum` → **Error**; benign metadata column → **Info** |

Data-has-extra (`LeftoverColumn`) defaults to Info (real files carry `population`,
`notes`); model-cell-unfilled-when-dense and stratum-mismatch default to Error.
**Column *role* is bound by name, never sniffed from content**: a header matching a
model stream/stratum name *is* that stream/stratum; a header matching nothing is a
located `LeftoverColumn`, never content-routed into the likelihood. Time *cell*
typing reuses the existing whole-column typer (`caltime_load::convert_time_column` /
`detect_kind`, `caltime_load.rs:100-221`): all-numeric → day-offsets, all-ISO → dated
(via the model `origin` + `time_unit`), mixed → a hard error naming both offending
rows. Value-cell typing is the part this adds — `RawValue` + the finiteness guard, so
a `NaN`/`inf`/non-numeric cell is a located `RejectedValue` finding, not a silent
coercion into the log-pmf.

## How `BoundObs` binds to the timeline spine

`BoundObs` is the input to types that already exist, plus the new `Observe`/
`ResetWindow` effects the topology work introduces. The mapping is mechanical, and it
is the single place the data layer and the temporal layer connect:

| `BoundObs` | becomes, in the runtime |
| --- | --- |
| `times` (the union axis) | `Schedule::with_obs(times)` — the obs boundaries every driver steps to (Snap or Exact) |
| each `StreamCells` | one `Observe` effect (`Stage::Observe`, read-only `&State`) |
| `StreamCells.kind` | the `Observe`'s `TemporalKind` → its `StreamProjection` (Interval→`FlowSum`; Instant→`IntCompSum`/`Expr`) |
| each `Interval` stream | one `ResetWindow{ flow_indices }` (`Stage::Reset`) keyed to *that* stream's flows |
| `cells[k] = Some(v)` | a scored observation for that stream at `obs_idx = k` |
| `cells[k] = None` (hole) | that stream contributes **no term** to the joint log-likelihood at `obs_idx = k` — skipped, not scored as an observed zero |

That last row is the entire correctness point: a hole is the *absence of a term in the
sum*, not an observed value of zero. The homogeneous path cannot express it because
every stream shares one dense `obs_times`; `Option` cells over a union axis can. And
the per-stream `ResetWindow` is what makes a *sparse* interval stream correct: its
flow accumulator is zeroed only at *its own* obs times, so a weekly-cases observation
no longer truncates a monthly-deaths window (the global-reset corruption the topology
work's `M3` fix removes).

The flow, end to end:

```
  --data PATH  /  --data NAME=PATH                 (raw file: long, or wide-sugar)
        |  parse + ir::caltime  (date -> model-time)
        v
  Vec<LongRow>                                     (untyped: stream, stratum, when, value)
        |  obsdata::bind(model, rows, dt, cal, policy)
        v
  (BoundObs, BindReport) -------------------------> report.verdict
        |  model-shaped, typed, Option cells           Error  -> refuse: SimError::Validation
        |                                               Warn/Info -> proceed, surface findings
        v
  MultiStreamObsModel : ObservationModel<ParticleState>
   + one Observe effect per stream  (TemporalKind, StreamProjection)
   + one ResetWindow per Interval stream  (per-stream flow indices)
   + Schedule::with_obs(times)      (the obs boundaries; StepPolicy snap|exact)
        |  log_likelihood(state, obs_idx, params)      (the one scoring seam, gh#139)
        v
  { particle_filter, if2, pmmh, pgas }              (each generic over ObservationModel)
```

Errors flow *alongside* the data, never as control flow: `bind` always returns the
pair, and the caller decides what `report.verdict` means. `fit`/`pfilter` refuse on
`Error` (a `SimError::Validation` carrying the rendered findings) unless
`--allow-drop[=kind]` downgrades acknowledged kinds; a new **`camdl check-data <model>
--data …`** runs `bind` purely to render the report and set an exit code. The private
`BoundObs` constructor is the invariant that makes the whole flow safe.

## The closed-loop hook: `observed_history`

Reactive interventions (the push after this one) close a loop *through* the
observation layer: a path-B trigger fires on an *observed* quantity with reporting
noise (`observed(weekly_cases) > 50`), not the latent count. The hook is one line of
scope here: the `Observe` stage maintains an **`observed_history`** buffer — the most
recent observed value per stream — and exposes it to rate/trigger expressions via an
`observed(stream)` primitive. In a *fit* the data are given, so reading the actual
observed history is deterministic and free; only *forward / EVSI* simulation draws a
fresh `y ~ p(y | projection)` to feed the trigger. Naming the buffer now (it is a
trivial write at `Stage::Observe`) is what lets reactive path-B land without
re-plumbing the observation layer later. See the topology proposal's closed-loop
section for the augmented-state treatment.

## `Counted`: the per-survey denominator (malaria)

Binomial slide-positivity — "k positive of n examined" — is the rigorous malaria
prevalence datum, and **n varies survey-to-survey**. Today the Binomial/BetaBinomial
denominator is a model expression (`BinomialLikelihood { n: Expr }`,
`ir/src/observation.rs`), so a survey-varying n can only be smuggled in as a forcing
table — splitting one logical observation across the model and the data file. The
`ObsCell::Counted { value, denom }` payload fixes this: the denominator rides *with*
the datum in `BoundObs`, and the likelihood reads it per cell. The fixed-`n: Expr`
path stays the default when no `denom` is supplied. This makes irregular, sparse
binomial positivity a first-class target; only subset-of-strata survey coverage
(cross-sectional surveys rarely cover every cell) waits on the `gh#171` stratum-subset
binder, which is model-side and separate.

## Forward: summary statistics and synthetic likelihood (the reduction axis)

camdl scores every fit through one seam — `ObservationModel::log_likelihood`,
evaluated *per observation time* and combined sequentially by the particle filter.
That per-cell, Markovian shape is what makes the bootstrap filter and PGAS work. A
whole class of methods deliberately abandons it: **synthetic likelihood** (Wood 2010;
King, Nguyen & Ionides 2016, the pomp `probe_match` surface) and **ABC**, which score
how well *summaries* of simulated data (peak height, time-to-peak, final size, growth
rate) match summaries of the observed data. These are the right tool when the
per-observation likelihood is intractable or ill-defined, and they are `gh#172`.

The structural fact: a summary statistic `s(y₁..y_T)` is a function of the *whole
series*, not one `obs_idx`, so it cannot be evaluated inside the sequential filter. It
is **not** a new `ObservationModel` arm — it is a *sibling* scorer consumed by a
different driver (simulate-many-then-compare, not sequential weighting). This is the
"reduction axis" the topology proposal reserves:

```rust
trait SeriesScorer {                                  // whole-series, not Markovian
    fn score(&self, observed: &BoundObs, simulated: &[Trajectory], params: &[f64]) -> f64;
}
enum Objective {
    Likelihood(MultiStreamObsModel),   // sequential; consumed INSIDE the PF (today)
    Synthetic(SyntheticLikelihood),    // simulate M reps → N(s; μ_θ, Σ_θ)  (Wood 2010)
    Abc(AbcDistance),                  // accept iff ρ(s_sim, s_obs) ≤ ε
}
```

`BoundObs` gives these methods their input for free: the observed summary is computed
once from `BoundObs`; the simulated summary is computed from each `Trajectory`
projected through the *same* `StreamProjection`, so the two sides are apples-to-apples
by construction, and holes are handled by the summary itself ("mean over present
cells"). The honest constraint: a summary objective is **incompatible with PGAS/NUTS**
(no per-time conditional density, no latent path, no gradient) — it composes with the
gradient-free outer loops (MH-over-θ, derivative-free optimization). This proposal
does not implement summary scoring; it only keeps the inference entry points reaching
their objective behind that small abstraction, so adding `Synthetic`/`Abc` is a new
*constructor of the objective*, not a new data path.

## Migration — layered on the topology stages

The data layer is independent of the timeline; only step 3 waits on the topology
work's per-stream `ResetWindow`.

1. **(light, parallel with topology)** `LongRow` parse (long + wide sugar) over
   `caltime`; the NaN/finiteness guard at `pfilter.rs:669/699`. No behaviour change.
2. **(light)** `bind` + `BindReport` + `BoundObs`, reproducing today's
   homogeneous/dense semantics so goldens do not move; the report is additive. Route
   the five scattered load sites through it (the unification).
3. **(HEAVY — the correctness tier; gated on the topology `ResetWindow`)** relax the
   ≥5 shared-grid assertions to the union axis + `Option`-cell scoring at the single
   seam `log_likelihood_from_flows_and_counts` (`multi_stream_obs.rs`), **wired to**
   the per-stream `ResetWindow` for `Interval` streams. FD/likelihood parity must hold
   on the dense case; the sparse-interval reset gets its own window-correctness test.
4. **(small)** `ObsCell::Counted { value, denom }` through `bind` into the
   Binomial/BetaBinomial scoring path. Scipy-anchored value test; the fixed-`n: Expr`
   path unchanged when no `denom` supplied.
5. **(small)** the `observed_history` buffer + `observed(stream)` primitive (the
   reactive hook), and `check-data` + load-time report + `--allow-drop`.
6. **(cross-cutting)** the `gh#98` calendar equivalence test:
   `expander.parse_date_to_float == caltime::date_to_internal` over a date battery
   (tables convert dates in OCaml at compile, obs-data in Rust at load — one constant,
   two implementations; pin them per the `rata_die` rule).
7. **(separate, model-side)** the `gh#171` stratum-subset binder + effort covariate.

## Test obligations (the load-bearing ones)

Every malformed input has a named, located outcome. The non-negotiable correctness
tests:

- **hole ≠ zero**: a `None` cell and an absent row score *identically*; a `None` cell
  and an observed `0` score *differently*. The whole point of the `Option` axis — a
  dedicated likelihood test.
- **sparse `Interval` per-stream reset**: a sparse incidence stream's flow accumulated
  over `[t₁, t₃]` is not truncated by another stream's observation at `t₂`. The single
  most important correctness test, and it exercises the topology work's `ResetWindow`.
- **dense-parity regression**: the homogeneous dense case scores bit-identically
  before/after the union-axis refactor (goldens do not move).
- **NaN/inf value cell** → rejected before the log-pmf; **non-numeric value** →
  located `RejectedValue`.
- **`Counted` denom**: scipy-anchored per-cell Binomial/BetaBinomial value test.
- **`Instant` off-grid** (annual prevalence under a daily grid) → snap + warn, **not**
  reject; **`Interval` off-grid** (window can't tile) → error. (The snap/exact
  decision itself is the topology work's `StepPolicy`; this tests the *policy choice
  per kind*.)
- **gh#98** calendar equivalence over a date battery.

## Open questions

- `OffGridInterval` default Error vs a sanctioned, logged `--snap-observations`
  (threading the topology `StepPolicy::Snap` per-stream).
- `--aggregate=sum|mean` for the many:1 case — ship now, or leave to user
  pre-aggregation?
- Where `1:many` aggregate cells come from — does this wait on the deferred
  spatial-aggregation operator?
