# Generated quantities and counterfactual contrasts

Status: **Draft (v2)** — design converged; the decisions below are made here for
review, not open questions. One breaking lexer change (leading-dot floats) is
flagged for `language-changes.md`.

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
analog) and `compare {}` (counterfactual contrasts across scenarios) — built
from the machinery camdl already has. Quantities are evaluated **post-hoc over
the posterior** by `camdl fit predict`, which already replays each draw through
the forward simulator; nothing here touches the inference kernels, and `Expr`
stays closed.

## Design at a glance

- A quantity is the **non-scored twin of an observation**: it reuses the flat
  `Projection` enum (`observation.rs:9`) — `CurrentPop`, `CumulativeFlow`,
  `DerivedExpr`, … — minus the likelihood, plus an optional temporal reduction.
  No new `Expr` leaf.
- **Most "cumulative" quantities are a stock**, read directly — no flow
  accumulator, no reset hazard.
- A counterfactual is an **existing timed intervention toggled by
  `enable`/`disable` scenarios** (which are byte-identical under CRN until the
  intervention fires) plus a `compare {}` contrast.
- The contrast names a quantity under a scenario with a **dot** —
  `no_sia.deaths` — one general namespace operator (also the future home of
  grouped params, `immunity.gamma`).
- A counterfactual's **window is its fork**: `over [X, Y]` conditions on data
  through `X`, forks, and accumulates to `Y`. Free-forward is the open-start
  case.

## Stocks first: most cumulative quantities need no new machinery

A compartment count _is_ a lifetime running stock (`CurrentPop`, never reset).
So the common cumulative quantities are read directly off state:

- **Deaths**: `total_deaths = final(D)` — `D` is the absorbing stock, already
  the cumulative.
- **Total infections**: `(N0 - S)` — when `S` is monotone (no births/waning into
  it), its depletion _is_ cumulative incidence.

The lifetime **flow accumulator** is needed only when the flow is not captured
by any stock — SIRS waning (`R→S` erases the count) or reinfections. That is the
edge case (handled below), not the common one. This is the key simplification:
read the stock and the per-interval-reset problem of the observation
`CumulativeFlow` projection never arises.

## Types

### Quantity

```rust
/// A declared generated quantity: a named, derived function of latent state,
/// evaluated post-hoc over posterior draws. The non-scored twin of
/// `ObservationModel` — same `Projection`, no likelihood.
pub struct Quantity {
    pub name:       String,
    /// Empty = whole-population; non-empty = one per (dim, level), reusing the
    /// observation stratum machinery (StratumKey).
    pub strata:     Vec<StratumKey>,
    /// The pointwise value, reusing the observation projection enum.
    pub projection: Projection,
    /// None = a SERIES (one value per output time). Some = a SCALAR: the series
    /// reduced over time to one value per draw.
    pub reduce:     Option<Reduction>,
    /// Accumulation / conditioning window (also the counterfactual fork point).
    /// None = whole horizon.
    pub window:     Option<Window>,
}

pub enum Reduction { Final, Max, Min, ArgMax, Integral, Mean }
pub struct Window { pub from: Expr, pub to: Expr }   // times
```

A quantity is therefore `projection [reduced] [windowed]`. `projection` reuses
the existing enum, so `prevalence = I/N` is `DerivedExpr(I/N)` (state
arithmetic), `total_deaths = final(D)` is `CurrentPop(D)` + `Final`, and
`cumulative_cases = cumulative(infection)` is `CumulativeFlow("infection")` in
its quantity interpretation (below).

**`Reduction` vs the existing reducers.** Two reducer vocabularies already
exist: `ObsReducer { Latest, Sum, Mean, Max }` (`intervention.rs:90`, for
reactive triggers) and the n-ary `Reduce` _spatial_ sum in `Expr` (`expr.rs`).
Neither is a _temporal_ reduction over a trajectory — `ObsReducer` folds an
observed window for a trigger predicate, `Reduce` folds over strata at one
instant. `Reduction` is the third axis (over output/sim time) and is
intentionally distinct; the names are chosen not to collide.
(`Final`/`ArgMax`/`Integral` have no analog in either.)

### `cumulative` in the quantity context (the edge case)

For the flows that _aren't_ a stock, a quantity may read a flow's **lifetime
running total**:

```rust
// Reuses Projection::CumulativeFlow(name), but evaluated in the quantity
// context as a LIFETIME running total — un-reset, un-differenced.
```

This is the one genuine new _semantic_ (not a new type): the observation
`CumulativeFlow` projection accumulates over a reporting interval and **resets
on the cadence** (`observation.rs:32`), then the obs path _differences_
consecutive times to incidence (`main.rs:1986`). A quantity wants the opposite —
the running total that never resets. The runtime already computes this running
value internally (`main.rs:1962`); the quantity path exposes it instead of
differencing it. Decision: a **quantity-context evaluation of `CumulativeFlow`**
(the projection type is reused; the evaluator, selected by context = quantity vs
observation, does not reset/difference) — _not_ a new `Expr` leaf and _not_ a
new `Projection` variant, so `Expr` and the run-id hash are untouched.
Arithmetic over two flows (`cfr = cumulative(death)/cumulative(infection)`) is
**deferred** (follow-up): define the two cumulatives as separate quantities and
divide downstream.

### Contrast

```rust
/// A counterfactual contrast: scalar arithmetic over a quantity evaluated under
/// named scenarios. Kept a small AST (not `Expr`) so `Expr` stays closed; its
/// dimcheck is trivial (the referenced quantities carry known dimensions).
pub struct Contrast { pub name: String, pub expr: ContrastExpr, pub window: Option<Window> }

pub enum ContrastExpr {
    Ref { scenario: String, quantity: String },   // surface: `no_sia.deaths`
    BinOp(Box<ContrastExpr>, ArithOp, Box<ContrastExpr>),
    Const(f64),
}
```

### The dot namespace operator

`no_sia.deaths` introduces **one** dotted member-access operator, used
uniformly: `scenario.quantity` now, grouped parameters (`immunity.gamma`) later.
Robust because it is one rule, not a one-off sigil.

Lexer impact is a single line. The only conflict is the **leading-dot float**
`.5` (`lexer.mll:174`, `'.' digit+`). Remove it (require `0.5`), and
disambiguation is total: a `.` is part of a float _iff_ digits precede it
(`int_lit '.' digit*`, `lexer.mll:170`, matched greedily); a `.` adjacent to an
identifier is always member access (the current bare-`.` error at `:208` becomes
a `DOT` token). This is a **breaking change** (`.5` → `0.5`) and gets a
`language-changes.md` entry with the migration. (`@` was rejected — it is the
rate operator, `S --> E @ rate`, `lexer.mll:220`; `->` was rejected — one stroke
from the `-->` flow arrow.)

### Model

```rust
pub struct Model {
    // …existing…
    pub quantities: Vec<Quantity>,  // default empty, skip_serializing_if empty
    pub compare:    Vec<Contrast>,  // default empty, skip_serializing_if empty
}
```

Empty-by-default + `skip_serializing_if` ⇒ existing models do not re-key
(`runid::ir_hash`); a model that _adds_ quantities gets a new `run_id`, as it
should.

## DSL surface — worked examples

### Generated quantities (single scenario)

```
compartments { S, E, I, R, D }

quantities {
  prevalence       = I / N                  # series   (DerivedExpr, no reduce)
  attack_rate      = final((N0 - S) / N0)   # scalar   (stock-derived)
  total_deaths     = final(D)               # scalar   (absorbing stock)
  peak_prevalence  = max_t(I / N)           # scalar
  time_to_peak     = argmax_t(I)            # scalar (a time)
  person_days_inf  = integral_t(I)          # scalar (person·days)
}
```

Stratified, reusing the observation stratum form (`prevalence[p in patch]`):

```
quantities {
  prevalence[p in patch] = I[p] / N[p]              # series, one column-group per patch
  attack_rate[p in patch] = final((N0[p] - S[p]) / N0[p])   # scalar, one per patch
}
```

The flow-accumulator edge case (a flow with no stock — reinfections in SIRS):

```
quantities {
  cumulative_reinfections = cumulative(reinfection)   # lifetime running total
}
```

### Counterfactual: cases averted from an SIA

The intervention carries the timing; `enable`/`disable` scenarios toggle it (the
byte-identical-under-CRN case); the contrast differences a quantity with a
window.

```
compartments { S, E, I, R, D, V }

interventions {
  sia : transfer(fraction = 0.6, from = S, to = V) at [week 20]   # vaccinate 60% of S
}

scenarios {
  no_sia    { disable sia }       # counterfactual: campaign never happened
  with_sia  { enable  sia }       # factual policy
}

quantities {
  deaths = final(D)               # absorbing stock
}

compare {
  averted      = no_sia.deaths - with_sia.deaths        over [week 20, week 52]
  rel_averted  = 1 - with_sia.deaths / no_sia.deaths    over [week 20, week 52]
}
```

Reading: `no_sia.deaths` is `deaths` evaluated under scenario `no_sia`;
`over [week 20, week 52]` conditions on observed data through week 20, forks
both arms from the smoothed state there, runs them under CRN, and accumulates
the contrast to week 52.

### Free-forward vs conditioned vs historical — one window knob

```
compare averted = no_sia.deaths - with_sia.deaths over [week 20, week 52]
#   historical counterfactual: condition through week 20, fork, accumulate to 52

compare averted = no_sia.deaths - with_sia.deaths over [last_obs, week 80]
#   conditioned forward projection: condition on everything, project forward

compare averted = no_sia.deaths - with_sia.deaths to week 52
#   free-forward: no conditioning, fork at t_start (prior-predictive counterfactual)
```

`over [X, Y]` = condition through `X`, accumulate to `Y`. `to Y` = open start =
free-forward. "Cases averted since X to Y" is `over [X, Y]`, unchanged.

## Scoping: reductions and `cumulative` are quantity-only

`max_t`, `argmax_t`, `final`, `integral_t`, `mean_t`, and the lifetime
`cumulative` are **legal only inside `quantities {}` / `compare {}`** — a
temporal reduction in a transition _rate_ (a per-substep propensity) is
meaningless and unevaluable. Because these are not `Expr` leaves (reductions
live on `Quantity`; `cumulative` is a quantity-context projection evaluation),
they are _unrepresentable_ in a rate by construction — the same discipline as
gh#204's `TriggerQuantity`, which sits outside `Expr` so `observed(...)` cannot
appear in a rate (`intervention.rs:103`). A reduction name used in a rate fails
as today's undeclared-function `E100`; the diagnostic is upgraded to name the
quantity block.

## Dimensional checking

Quantities and contrasts run through `dimcheck` (they are not exempt from the
dimensional-safety guarantee every rate gets). Rules:

- `CumulativeFlow` (quantity context) ⇒ a **count** (`P`) — the time-integral of
  a rate.
- `final`/`max`/`min`/`mean` preserve the dimension of the series.
- `integral_t(series)` ⇒ `dim(series) · T` (e.g. `person_days_inf` is `P·T`).
- `argmax_t(series)` ⇒ `T` (discards the series dimension; returns a time).
- A `ContrastExpr` `BinOp`'s operands must have equal dimension for `+`/`-`
  (`no_sia.deaths - with_sia.deaths` is `P - P`;
  `peak_prevalence - total_deaths` is rejected) and `/` yields their quotient
  (`rel_averted` is dimensionless).

## Runtime and triggers

**Quantities are generated post-hoc, on the posterior — never in the fit loop.**
`fit run` produces posterior draws against the _factual_ observed data;
`fit predict` replays each draw and evaluates the quantities. Reductions fold
over the **integrator substep grid**, not the output cadence — so
`peak_prevalence`, `time_to_peak`, and `integral_t` are properties of the
dynamics, not of the serialization `every` (a peak between output times is _not_
missed). This is the correctness-honest choice; the output cadence only governs
how a _series_ quantity is sampled for serialization.

**Conditioning window.** The `over [X, Y]` start `X` is the fork/conditioning
point: condition on observed data through `X`, take the smoothed posterior
latent state at `X` (PGAS already produces the conditioned trajectory), and run
forward. No window start (`to Y`, or absent) = free-forward from `t_start` (the
model's declared initial state under each draw).

**Compare contrasts trigger only when a `compare {}` block is present**, in the
predictive replay only (not on every prediction — each contrast is an extra
forward replay per scenario per draw). Each draw is replayed under each
referenced scenario at the **same seed** — CRN. Honest coupling statement:
`enable`/`disable` scenarios (the intervention-toggle case) are **byte-identical
until the intervention fires**, giving a clean paired contrast; `set`/`scale`
scenarios that alter propensities from `t=0` are **correlated, not
byte-identical** (the chain-binomial backend consumes a rate-dependent number of
RNG words, so the streams diverge at the first differing substep) — usable, but
the variance reduction is uncontrolled. The recommended counterfactual surface
is therefore the intervention-toggle form above.

**Scope note (honest):** `fit predict` has **no multi-scenario plumbing today**
— it builds one inline baseline (`predict.rs:860`). Stage 2 builds the
paired-replay path; it is not "wire an existing hook." Cost is
`O(scenarios × draws × strata)` forward replays — at national scale (hundreds of
strata, thousands of draws) a two-arm contrast doubles the predict cost; the
`compare {}`-present gate is the only bound, and the cost is documented, not
hidden.

## Output format

- **TSV** (camdl convention), **one file per quantity / contrast**
  (`quantities/<name>.tsv`, `compare/<name>.tsv`), **long/tidy keyed by stratum
  level** — matching `fit predict` / gh#279 so a consumer joins by
  `(time, dims)`. One-per is required: a _series_ quantity has a `time` column
  and a _scalar_ does not, so a combined file would be a heterogeneous schema.
- Bands over draws reuse `fit predict`'s quantile levels
  (`q05 q25 q50 q75 q95`).

```
# series quantity, stratified         quantities/prevalence.tsv
time   patch    n_draws  q05 q25 q50 q75 q95

# scalar quantity, stratified         quantities/total_deaths.tsv   (no time axis)
patch  n_draws  q05 q25 q50 q75 q95

# contrast (scalar)                    compare/averted.tsv
patch  n_draws  q05 q25 q50 q75 q95     # single row when unstratified
```

- `n_draws` (the band's draw count) travels for provenance; a `quantities.json`
  / `compare.json` manifest lists each entry's name, shape (series|scalar),
  strata, reduction, window, and **unit** (so `attack_rate` (dimensionless) and
  `person_days_inf` (`P·T`) are machine-distinguishable). The predictive
  artifact's `horizon`/`treatment`/`rhat`/`ess` columns are _not_ carried — a
  quantity is its own object, not a predictive band; convergence is read from
  the fit's `fit.meta.json`.
- Resolved spec points (no implementer guess): `argmax_t` returns the **first**
  argmax on ties/plateaus; `fit predict` with an empty `quantities {}` writes no
  quantity files (and is not an error); quantities band over **all** posterior
  draws (the `--n-draws` cap is one-step-ahead only and does not apply); a
  `compare` reference to an undefined scenario, or a `compare {}` with no
  `scenarios {}` block, is a located `E`-error naming the missing scenario; on
  an ODE backend `cumulative`/stocks are real-valued (expected cumulative
  incidence) vs integer counts on chain-binomial — the manifest unit is the
  same, the realization differs, and the manifest records the backend.

## Staging

- **Stage 1 — `quantities {}` (single scenario).** The block, the `Reduction`
  set, the lifetime-`cumulative` quantity-context evaluation, dimcheck, the
  substep-grid fold, and `fit predict` banding into `quantities/<name>.tsv`.
  Delivers attack rate, peak, cumulative incidence, time-to-peak. Reuses the
  `Projection` enum and the #298 replay loop; **no `Expr`/run-id change**.
- **Stage 2 — `compare {}` (counterfactual).** The dot namespace operator (+ the
  `.5`→`0.5` lexer change), the `ContrastExpr` AST + its dimcheck, the
  conditioning-window fork, and the **multi-scenario paired replay in
  `fit
  predict`** (the real build). Delivers cases averted.

## Decisions recorded

- **Quantities reuse the flat `Projection` enum**, not a new `Expr` leaf — keeps
  `Expr` (autodiff/dimcheck/flat-eval/run-id) closed; quantities are never
  differentiated, so they never belong in the kernel's expression type.
- **`cumulative` is a lifetime running total in the quantity context**
  (un-reset, un-differenced) — distinct from the observation per-interval
  incidence.
- **Reductions fold over the substep grid**, not the output cadence — no silent
  dependence of a headline number on `every`.
- **Dot namespace operator**, with `.5`→`0.5` as the (documented, breaking)
  cost; `@`/`-`/`->`/`:` rejected for collision/ambiguity.
- **Window-is-fork** (`over [X, Y]`) unifies free-forward, conditioned,
  historical, and "averted since X to Y".
- **Counterfactual = timed intervention + `enable`/`disable`** (clean CRN), not
  a `set`/`scale` from `t=0` (correlated only).
- **Quantities/contrasts run through `dimcheck`.**
- **Separate `quantities {}` and `compare {}` blocks**, not folded into
  `output {}` — three independent reviews converged on the define/serialize
  seam.

## Follow-ups (named, not blocking)

- Flow arithmetic in a quantity
  (`cfr = cumulative(death)/cumulative(infection)`) and reduction arithmetic
  within one quantity (`final(A) - final(B)`).
- `at_time(series, t)` (value at an instant) and decoupling the conditioning
  point from the accumulation window (two windows).
- `output { trajectories { quantities = [...] } }` co-location (serialize a
  series quantity as a trajectory column) — dropped from v1; everything goes to
  `quantities/<name>.tsv`.
- Geometry-aware scalar quantities once gh#306 lands (a per-patch scalar → a
  choropleth).
