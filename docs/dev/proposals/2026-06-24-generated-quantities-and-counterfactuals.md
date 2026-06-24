# Generated quantities and counterfactual contrasts

Status: **Draft** — design agreed in principle; the function set, output format,
and trigger policy below are decisions made here for review, not open questions.

Supersedes: `2026-06-04-output-trajectory-customization.md` (Phase 2 only —
Phase 1 cadence/format is shipped and stays) and
`2026-06-04-experiment-compare.md`.

## Summary

A camdl user can fit a model and recover a posterior, but cannot yet ask the
model to _report a derived quantity_ — cumulative incidence, attack rate, peak
prevalence, time to peak, or the headline policy number, **cases averted**.
Today the only way to compute a function of latent state is to smuggle it in as
a _scored_ observation stream (the expander requires a likelihood, `E266`),
which forces the author to pretend a reported quantity is data.

This proposal adds two blocks — `quantities {}` (the Stan `generated quantities`
analog) and `compare {}` (counterfactual contrasts across scenarios) — and the
expression-language pieces they need. Quantities are evaluated **post-hoc over
the posterior** by `camdl fit predict`, which already replays each draw through
the forward simulator; nothing here touches the inference kernels.

## The seam: define vs serialize

camdl already separates _defining_ a computed-from-state quantity from
_serializing_ it, and this proposal follows that boundary rather than the file
destination. An `ObservationModel` is computed from state and written to a file,
yet it is **defined** in `observations {}` (`rust/crates/ir/src/model.rs:180`)
while `output {}` only carries the serialization switch (`OutputConfig`,
`model.rs:196`; `output { observations = true }` is a bool toggle). A generated
quantity is the **non-scored twin of an `ObservationModel`** — same
`Projection::DerivedExpr` machinery (`observation.rs:13`), minus the likelihood.
So it belongs in its own definition block beside `observations {}`, and
`output {}` stays the place that schedules and selects what is written.

Three blocks, one concern each:

- `scenarios {}` — _define_ baseline / intervention (exists today).
- `quantities {}` — _define_ derived functions of state. **New.**
- `compare {}` — _contrast_ a named quantity across a scenario pair. **New.**

`output {}` is unchanged except for an optional reference to series quantities
it should co-locate as trajectory columns (below).

## Types first

### Quantities

```rust
/// A declared generated quantity: a named, derived function of latent state,
/// evaluated post-hoc over posterior draws. The non-scored twin of
/// `ObservationModel`.
pub struct Quantity {
    pub name:   String,
    /// Empty = whole-population scalar/series; non-empty = one per (dim, level),
    /// reusing the observation stratum machinery (StratumKey).
    pub strata: Vec<StratumKey>,
    pub body:   QuantityBody,
}

pub enum QuantityBody {
    /// Pointwise expression evaluated at each output time → a time series.
    /// (prevalence = I/N; cumulative_cases = cumulative(infection); R_eff(t))
    Series(Expr),
    /// A temporal reduction of a series → one value per draw, no time axis.
    /// (total_cases = final(cumulative(infection)); peak = max_t(I/N))
    Scalar { reduce: Reduction, over: Expr },
}

pub enum Reduction {
    Final,    // value at the last output time
    Max,      // peak value over t
    Min,      // trough over t
    ArgMax,   // the *time* at which the max occurs (time-to-peak) — returns a time
    Integral, // ∫ over output times (trapezoidal) — person-time / area under curve
    Mean,     // time-average over the output window
}
```

A quantity is therefore either a **series** (pointwise expr → has a time axis)
or a **scalar** (a single top-level reduction of a series → no time axis).
Reductions are top-level only in v1 — `max_t(I/N)` is allowed; `I / max_t(I)` (a
series divided by a scalar, mid-expression) is deferred, because it requires a
series/scalar broadcasting language the pointwise `Expr` does not have. The v1
restriction covers every named use case below and keeps the grammar a flat
`name = expr` or `name = reduction(expr)`.

### Expression extension: `cumulative`

```rust
pub enum Expr {
    // …existing: Const | Param | Pop | PopSum | Time | Dt | BinOp | UnOp |
    //   Cond | TimeFunc | TableLookup | Projected | UncheckedDim | Reduce |
    //   BindingRef | PerEvalRef | ObsColumnRef…
    Cumulative(CumulativeExpr),   // running total of a named flow's accumulator
}
pub struct CumulativeExpr { pub flow: String }
```

`cumulative(infection)` reads the same per-flow accumulator the observation
`Projection::CumulativeFlow` reads (`observation.rs:9`), but _inside_ `Expr`, so
a quantity can do arithmetic on cumulative flows — e.g. a case-fatality ratio
`cfr = cumulative(death) / cumulative(infection)`. This is the one genuinely
missing leaf: today flows are reachable only through the `Projection` enum,
never in an expression (`expr.rs` has no flow accessor — verified, no
`cumulative` in `expr.rs`/`parser.mly`).

### Compare

```rust
/// A counterfactual contrast: arithmetic over a quantity evaluated under two
/// (or more) scenarios, with the SAME quantity defined once in `quantities {}`.
pub struct Contrast {
    pub name: String,            // "cases_averted"
    pub expr: ContrastExpr,      // baseline.total_cases - with_sia.total_cases
}
/// Expr-shaped, but its leaves are scenario-qualified quantity references.
/// The referenced scenarios define the paired-replay set; CRN is implicit.
pub enum ContrastExpr {
    Ref { scenario: String, quantity: String },
    BinOp(Box<ContrastExpr>, ArithOp, Box<ContrastExpr>),
    Const(f64),
}
```

### Model

```rust
pub struct Model {
    // …existing…
    pub quantities: Vec<Quantity>,  // default empty
    pub compare:    Vec<Contrast>,  // default empty
}
// OutputConfig gains:  pub quantities: Vec<String>  (series quantities to also
// emit as trajectory columns at the output cadence — optional convenience)
```

## DSL surface

```
quantities {
  prevalence[p in patch] = I[p] / N[p]            // series, stratified
  cumulative_cases       = cumulative(infection)  // series
  total_cases            = final(cumulative(infection))   // scalar
  attack_rate            = final((N0 - S) / N0)           // scalar
  peak_prevalence        = max_t(I / N)                   // scalar
  time_to_peak           = argmax_t(I)                    // scalar (a time)
  person_days_infectious = integral_t(I)                  // scalar
}

compare {
  cases_averted    = baseline.total_cases - with_sia.total_cases
  relative_averted = 1 - with_sia.total_cases / baseline.total_cases
}

output {                                  // serialization axis, unchanged
  trajectories { every = 1 'week, quantities = [prevalence, cumulative_cases] }
}
```

`output { trajectories { quantities = [...] } }` is the optional co-location:
list **series** quantities to also write as columns on `trajectories.tsv` at the
output cadence (the cadence lives here, the _definition_ lives in
`quantities {}` — exactly the `observations {}` define ↔
`output { observations = true }` serialize split). Scalar quantities and
contrasts have no trajectory cadence and are never trajectory columns; they go
only to their own artifacts.

## The function set

`cumulative(flow)` is required (above). Beyond it, the reduction set is chosen
to cover the standard outbreak quantities with the smallest surface:

| function             | shape         | covers                                 | tier          |
| -------------------- | ------------- | -------------------------------------- | ------------- |
| `cumulative(flow)`   | series        | cumulative incidence, total cases, CFR | **essential** |
| `final(series)`      | scalar        | attack rate, final size, total cases   | **essential** |
| `max_t(series)`      | scalar        | peak prevalence / peak incidence       | **essential** |
| `argmax_t(series)`   | scalar (time) | time to peak                           | **essential** |
| `integral_t(series)` | scalar        | person-days infectious, exposure-time  | useful        |
| `min_t(series)`      | scalar        | epidemic trough                        | optional      |
| `mean_t(series)`     | scalar        | average prevalence over window         | optional      |

Deliberately **out** of v1: `at_time(series, t)` (value at an arbitrary instant
— useful, but adds an interpolation question), and mid-expression reductions
(normalized series). Both are clean follow-ups once the v1 shape is exercised.
The existing pointwise `Expr` already provides arithmetic, `exp/log/sqrt/abs`,
binary `min/max`, and `Cond`, so ratios, logs, and piecewise quantities need no
new functions.

## Runtime and trigger policy

**Quantities are generated post-hoc, on the posterior — never during the fit.**
The fit (`fit run`) produces posterior draws against the _factual_ observed
data. `fit predict` then replays each draw through the forward simulator and
evaluates the quantities — the same loop #298 already runs for predictive bands,
with the quantity expressions evaluated over the replayed trajectory state and
flow accumulators (`eval_expr` exists; reductions fold over the output-time
series).

- `camdl fit predict <fit>` evaluates every declared `quantities {}` entry on
  the terminal Bayesian stage's posterior draws and writes
  `quantities/<name>.tsv`. It does **not** auto-run on `fit run` — predict stays
  a separate, explicit verb (keeps the fit loop lean; mirrors #298).
- `camdl simulate --draws <file>` evaluates the same quantities forward at fixed
  params or prior draws — a fit-free "what-if," same runtime, sourced from the
  draws file instead of the posterior.

**Compare contrasts trigger only when a `compare {}` block is present, and only
in the predictive replay** — they are not run for every prediction (each
contrast costs one extra forward replay per scenario per draw). When present,
`fit predict` (or `simulate --draws`) computes each contrast by replaying every
draw under each referenced scenario at the **same seed** (CRN — the `compare`
block owns the pairing; the author never manages seeds), evaluating the named
quantity under each, applying the contrast arithmetic, and banding over draws
into `compare/<name>.tsv`. In the with-fit case the draws _are_ the seeds; in
the fit-free forward case the seed range comes from `simulate`.

`enable`/`disable` scenarios are byte-identical pre-intervention under CRN;
`set`/`scale` are correlated-from-t0 — both already tested. The contrast is
therefore paired and low-variance: it isolates the intervention, not RNG noise.

## Output format

Decisions (answering the open UX questions):

- **TSV, not CSV.** Consistent with `fit predict` and every other camdl
  artifact. A `format` override can follow `output {}`'s existing `format = …`
  if ever needed; default TSV.
- **One file per quantity** — `quantities/<name>.tsv`, `compare/<name>.tsv` —
  mirroring `predictive/<stream>.tsv`. One-per is required, not stylistic: a
  _series_ quantity has a `time` column and a _scalar_ does not, so a single
  combined file would be a heterogeneous schema. Per-file keeps each schema
  clean and lets a consumer load exactly what it wants.
- **Long (tidy), keyed by stratum level** — yes, long, matching #298 / gh#279 so
  the same join-by-level a consumer already uses for predictive bands works
  here. Wide-by-stratum would break generic faceting.

Column schemas (bands reuse `fit predict`'s `QUANTILE_LEVELS` —
`q05 q25 q50 q75
q95` — over draws):

```
# series quantity, stratified            quantities/prevalence.tsv
time   patch     q05 q25 q50 q75 q95

# scalar quantity, stratified            quantities/total_cases.tsv   (no time axis)
patch  q05 q25 q50 q75 q95

# contrast (scalar)                       compare/cases_averted.tsv
patch  q05 q25 q50 q75 q95     # or a single row when unstratified
```

A `quantities.json` / `compare.json` manifest (paralleling `fit predict`'s
existing schema descriptor) lists each quantity's name, shape (series|scalar),
strata, and reduction, so a consumer renders generically.

## Staging

- **Stage 1 — `quantities {}` (single scenario).** The `quantities` block, the
  `cumulative` leaf + the reduction set, the IR additions, and the `fit predict`
  evaluation/banding into `quantities/<name>.tsv`. Delivers attack rate, peak,
  cumulative incidence, time-to-peak, CFR. This is the Stan
  `generated quantities` analog and reuses #298 wholesale.
- **Stage 2 — `compare {}` (counterfactual).** Scenario-qualified contrast
  references, the paired-CRN replay in predict, `compare/<name>.tsv`. Delivers
  cases averted. Builds on Stage 1 + the existing `scenarios {}` machinery and
  the predict scenario hook (`predict.rs:857`).

## Decisions recorded (vs. alternatives considered)

- **Separate `quantities {}`, not folded into `output {}`.** Three independent
  reviews converged: the IR already factors define (`observations`) from
  serialize (`OutputConfig`); the removed phantom `output { summary {} }` was
  computation-inside-output and was the part that never worked; Stan/pomp both
  keep the computation block separate from output config. The one pull toward
  folding — a pointwise series _is_ a trajectory column — is resolved by the
  define/serialize split: definition in `quantities {}`, cadence in `output {}`.
- **`compare {}` is a sibling of `quantities {}`, not nested in it.** A contrast
  needs a by-name quantity referent and a scenario set; it is its own concern.
- **Reductions top-level only in v1** (no mid-expression reductions) — covers
  every named quantity without a series/scalar broadcasting language.
- **Post-hoc on the posterior via `fit predict`, never in the fit loop** — a
  counterfactual has no data to score against; it is a projection.

## Follow-ups (named, not blocking)

- `at_time(series, t)` and mid-expression reductions (normalized series).
- `format = csv` override if a consumer needs it.
- Promoting geometry-aware quantity output once gh#306 (per-dimension geometry)
  lands, so a scalar quantity per patch can drive a choropleth directly.
