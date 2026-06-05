---
date: 2026-06-05
status: proposal
related: gh#171, gh#172, gh#98
area: observation data loading / inference
note: re-audit gh#134 / the 2026-05-30-unified-observation-data umbrella against
  HEAD before relying on it — FromData and data_stream were deleted (e845282,
  77bfe4e); fitting already bypasses the schedule enum, reading obs times
  straight into Vec<Observation>, so the union axis is a pure loader concern.
---

# Observation-data binding

## Framing: bind, not join

A **join** is _symmetric_: two co-equal relations, `A ⋈ B = B ⋈ A`, the result
schema is the union of both attribute sets, and the operation _discovers_ which
keys coincide — neither side is privileged. Loading observation data is not
that. A **bind** is _asymmetric and directional_: the model **defines** a fixed
lattice of named cells (which streams exist; which strata; which grid times
`t_start + k·dt`), and data values are bound _into_ those named slots — like
binding arguments to parameters, or filling a template's holes. You still match
keys mechanically, but the framing privileges the model's lattice as the
authority, and that changes the semantics in exactly the way we need:

- a **leftover** (a data row with no cell — typo'd stream, stratum the model
  lacks, off-grid time) is "data I have nowhere to put" → usually a mistake;
- a **hole** (a cell with no data) is "this slot got nothing" → often _expected_
  (sparse surveillance).

A symmetric join collapses both into one "unmatched" bucket; a bind keeps the
two directions distinct, with distinct severities. And the _result type_
differs: a join yields a union of two tables; a bind yields a **model-shaped
object with typed holes** (`Option` cells) — precisely the sparse-geometry
representation gh#171 needs.

## Why this is a correctness surface

A data point that fails to reach scoring — silently — is a wrong likelihood,
hence a wrong posterior. Genuine residual gaps today (verified against HEAD):

- **NaN/inf are unguarded on the obs path.** `pfilter.rs:669,699` parse values
  with bare `parse::<f64>()`, which accepts `"NaN"`/`"inf"`; nothing checks
  `is_finite` before the value reaches the log-pmf. (Note: gh#100 is a
  _different_ loader — `batch.rs` parameter draws — not this path.)
- **Silent overwrite in the substep map.** `build_obs_at_substep`
  (`pgas.rs:269`) fills a `HashMap<substep, obs_idx>` with `insert` and no
  collision check — two obs rounding to one substep silently drop one from the
  PGAS likelihood. (The CLI load path _does_ hard-error this collision first,
  `caltime_load.rs:254-263`; this is a defense-in-depth latent bug, worth a
  pinning test.)
- **Heterogeneous schedules are a hard wall**, replicated across
  `multi_stream_obs.rs:312-318`, `pfilter.rs:163-179`, `profile.rs:516`,
  `survey.rs:755,890`. Sparse/multi-cadence surveillance (gh#171) can't be
  expressed; the homogeneity is enforced by rejecting, not by reporting.

(Not residual: the `--data` dt-collision is already a clean error
`caltime_load.rs:254-263`; gh#108 is the unrelated `--dates` _output_ render at
`caltime.rs:178`.)

## Unification: one module, consumed as a type

The observation surface is half-unified today. **Scoring** is a single seam —
`MultiStreamObsModel` (`multi_stream_obs.rs`), consumed by PGAS and its
gradient, and since gh#139 inherited by all four methods (PF/IF2/PMMH/PGAS). But
**loading and construction** is scattered and duplicated: the
`--data → per-stream series → MultiStreamObsModel` pipeline is re-implemented in
`pfilter.rs`, `profile.rs`, `fit/runner.rs`, and `survey.rs` — each resolves
`--data` itself, builds its own `per_stream_obs`, runs the shared-grid check
(replicated across ≥5 sites), and canonicalizes
`observations = per_stream_obs[0]`. That duplication is where the silent drops
and the homogeneity asserts live.

So `obsdata::bind` is not only an audit — it is the **unification**: it owns the
load + validate + construct, emits one `BoundObs`, and every algorithm _consumes
that type_ instead of re-deriving it. One module, one type, defined once — so
the on-grid policy, the temporal-kind handling, and the accumulator-reset
semantics live in a single place, not per caller. The data _flow_ becomes
`load → bind → BoundObs → {PF, IF2, PMMH, PGAS}`, rather than each algorithm
re-loading and re-checking.

## Temporal kind is first-class

Each stream has a `TemporalKind`, derived from its projection, and it governs
on-grid policy, accumulator semantics, and off-grid handling:

- **`Interval`** (`CumulativeFlow`/`CumulativeFlowSum`, i.e. incidence): the
  value is flow accumulated over `[prev_obs, this_obs]`; the cell is a window.
  - On-grid is a **correctness** requirement: `dt` must tile the window, which
    holds iff both endpoints are on the grid → off-grid is an **error**.
  - Needs **per-stream accumulator reset** at that stream's obs times. The
    current reset is _global_ (`particle_filter.rs:401`), which over a sparse
    stream would inflate other streams' windows — this is the umbrella's §5.2.1
    CRITICAL finding and must land _with_ `None`-skip, not after.
- **`Instant`** (`CurrentPop`/`CurrentPopSum`/`DerivedExpr`, i.e. prevalence):
  the value is an instantaneous snapshot read at the obs time
  (`resets_after_observation` is false, `multi_stream_obs.rs:84-89`). There is
  no window to tile.
  - Off-grid is a mild **snapshot-time** error → read at the nearest grid point
    and **warn** past a tolerance, do _not_ reject.
  - No accumulator, no reset.

A single unified "off-grid = error" rule (the prior draft's mistake) would
hard-reject a valid annual prevalence survey under a daily grid. The kind split
fixes that.

## The bind, as a cardinality map

`bind` is a partial map `φ : DataRow → ModelCell`, cell =
`(stream, stratum, k)`. Every failure is a departure from "injective and total,"
with a defined resolution:

| cardinality            | cause                                        | resolution                                                                                     |
| ---------------------- | -------------------------------------------- | ---------------------------------------------------------------------------------------------- |
| 1:1                    | —                                            | clean                                                                                          |
| many:1 (non-injective) | `dt` coarser than data, or duplicate row     | `Collision`/`Duplicate` → **Error**; `--aggregate=sum\|mean` opt-in (loud, changes likelihood) |
| 1:many                 | data coarser than model (region vs district) | needs a model aggregate cell (`CumulativeFlowSum`); else `CoarserThanModel` → **Error**        |
| 0:1                    | cell with no data — a hole                   | `None` cell; `Hole` → **Info** (sparse) / **Warn** (stream declared dense)                     |
| 1:0                    | data with no cell — a leftover               | `LeftoverColumn`/`LeftoverStratum` → **Error**; benign extra metadata column → **Info**        |

Severities split by **direction**: data-has-extra (`LeftoverColumn`) defaults to
Info (real files carry `population`, `notes` columns); model-cell-unfilled-
when-dense and stratum-mismatch default to Error.

## Types and how they throw

```rust
mod obsdata {
    pub enum TemporalKind { Interval, Instant }

    struct LongRow { stream: String, stratum: Option<String>, when: RawTime, value: RawValue }
    enum RawTime  { Offset(f64), Date(String) }     // via ir::caltime + model origin
    enum RawValue { Num(f64), Missing, Unparseable(String) }

    /// model-shaped result; PRIVATE ctor — only `bind` makes it.
    pub struct BoundObs { times: Vec<f64>, streams: Vec<StreamCells> }
    struct StreamCells { name: String, kind: TemporalKind, cells: Vec<Option<f64>> }  // None = hole

    pub enum Severity { Error, Warn, Info }
    pub struct Finding { kind: BindIssue, stream: String, detail: String, count: usize, severity: Severity }
    pub enum BindIssue {
        LeftoverColumn, LeftoverStratum, OffGridInterval, OffGridInstant,
        Collision, Duplicate, CoarserThanModel, Hole, RejectedValue,
    }
    pub struct BindReport { findings: Vec<Finding>, verdict: Severity }

    pub fn bind(model: &Model, rows: Vec<LongRow>, dt: f64, cal: &CalendarCtx, policy: &BindPolicy)
        -> (BoundObs, BindReport);   // never panics, never exits — errors are VALUES (gh#181)
}
```

Throw discipline:

- `bind` returns the pair; the **caller** gates on `report.verdict`.
- `fit`/`pfilter` at load: `Error` → refuse with the rendered findings (a
  `SimError::Validation` value), unless `--allow-drop[=kind]` downgrades the
  acknowledged kinds.
- **`camdl check-data <model> --data …`** — a _new Rust subcommand_ that runs
  `bind` to render the report + set the exit code. (Not the OCaml `check`, which
  is a `camdlc` passthrough — `main.rs:206,414` — and never reads obs data.)
  Findings render as structured diagnostics so `--json-errors`/CI/book consume
  them, per gh#181.
- Invariant: `BoundObs` has no public constructor → no un-bound data reaches the
  likelihood; every leftover/collision/hole is in some `BindReport`.

## Input format: long-canonical

One row per _present_ observation: `time, stream, stratum, value`. Sparsity is
absent rows — no NaN-vs-zero ambiguity. A long-indices/wide-streams sugar
(`time, patch, afp, es`) is accepted and normalized in, with empty = typed
`None`, never zero. `BoundObs` is itself "long indices × wide `Option` cells".

## Scope vs #171 (honest)

This is the **data-sparsity substrate** for gh#171: it lets the _data_ be sparse
and time-varying, and the `Option` cells carry it through scoring. It does
**not** by itself satisfy gh#171's two model-side asks — restricting a stream's
`projected` to a subset of strata, and time-varying observation effort (a
forcing). Those are separate (a subset binder in the DSL; an effort covariate).
Necessary, not sufficient — the Sokoto ES case needs all three. gh#172
(summary-statistic targets) is orthogonal (it changes _what_ is scored).

## The shared calendar (gh#98)

`bind` converts dates via `ir::caltime` (the model's time basis). But tables
convert dates in **OCaml** (`expander.ml:130` `parse_date_to_float`) at compile
time, while obs-data converts in **Rust** (`caltime`) at load — two
implementations of one constant. Pin them with an equivalence test
(`expander.parse_date_to_float == caltime::date_to_internal` over a date
battery), per the `rata_die` cross-language rule.

## Migration (honest about the heavy tier)

1. **(light)** `LongRow` parse (long + wide sugar) over `caltime`; the NaN guard
   at `pfilter.rs:669/699`; the `build_obs_at_substep` collision pin. No
   behavior change.
2. **(light)** `bind` + `BindReport` + `BoundObs`, reproducing today's
   homogeneous/dense semantics so goldens don't move; the report is additive.
3. **(HEAVY — the real correctness tier, not "incremental")** Relax the ≥5
   shared-grid assertions to the union axis + `Option`-cell scoring at the
   single likelihood seam `log_likelihood_from_flows_and_counts`
   (`multi_stream_obs.rs:366-391`, the ~100×-divergence code), **together with**
   per-stream accumulator reset for `Interval` streams. FD/likelihood parity
   tests must hold on the dense case; the sparse-interval reset needs its own
   window-correctness test (the umbrella's §5.2.1 trap).
4. `check-data` + load-time report + `--allow-drop`.
5. The gh#98 date-equivalence test.
6. (separate) the gh#171 subset binder + effort covariate — model-side, not this
   proposal.

## Open questions

- `OffGridInterval` default Error vs a sanctioned, logged `--snap-observations`.
- `--aggregate=sum|mean` for the many:1 case — ship now or leave to user
  pre-aggregation?
- Where `1:many` aggregate cells come from — does this wait on the deferred
  spatial-aggregation operator (umbrella §7.1)?
