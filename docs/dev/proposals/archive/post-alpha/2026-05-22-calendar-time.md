# Proposal: Dated I/O as a Boundary Translator — calendar time, `time_unit`, and the integrator step

**Status:** draft for discussion (supersedes the earlier "Calendar Time" draft).
**Scope:** let users feed camdl **dated** observation data (ISO dates) and read
results back as dates, via a thin translator at the I/O boundary, with a single
precisely specified relationship to the internal continuous time axis. Dates
never become a type the dynamical or inference core has to reason about.

**One-line thesis:** the date↔number conversion already exists — it lives in
every user's fetch script, untested, where camdl cannot see it. Centralizing it
as a small, golden-tested boundary layer _shrinks_ the real bug surface. The job
is to centralize it without letting it leak inward, and **without touching the
integrator step `dt`**, which is a separate axis (§4).

---

## 1. Motivation

camdl's internal time is a continuous float in units of `time_unit`, measured
from an origin. Real epidemic data is dated. Today the bridge is built **outside
the tool, by hand**: the seed-timing chapter's real datasets (NYT COVID-WA,
Hagelloch measles) are loaded by Python/R fetch scripts that diff cumulative
counts and anchor `t=0` to a chosen calendar date (`t=0 = 2020-01-21`,
`t=0 = 1861-10-01`). Every outside epidemiologist with a dated line list does
the same anchoring by hand.

That hand-conversion is the bug surface that matters. A wrong anchor, an
off-by-one, or a leap-year slip in a notebook produces a _confidently wrong fit_
that camdl has no way to detect. For a tool whose value is reproducible
inference, pushing error-prone date arithmetic into user-land is an epistemic
hole, not a neutral simplification. The goal: an outside epidemiologist points
camdl at a dated TSV and gets a fit, with the seed time τ (the estimated time at
which infection is introduced) reported back **as a date**.

---

## 2. Current system (corrected)

- **Internal time is a continuous float**, in units of `time_unit`, measured
  from `origin`. The `chain_binomial` backend advances it in **fixed sub-unit
  steps of length `dt`** (Euler-multinomial); the dynamics are a continuous-time
  process, exact only as `dt → 0`. So the solver axis is _not_ integer (§4).
- **The observation grid is commonly integer** for daily surveillance (`time`
  column = day-numbers), but that is a property of the _data_, not an invariant
  of the engine. `--data` time columns are parsed as `f64`
  (`cli/src/data.rs:45`); fractional times are legal.
- **`time_unit`** ∈ `Days | Weeks | Months | Years` (+ `per_*`). Canonical
  duration `D(unit)` in **days**:
  `Days=1, Weeks=7, Months=365.2425/12,
  Years=365.2425`. Days and Weeks are
  integer-day units; Months and Years are not (average lengths).
- **Compile side (OCaml) does dates; runtime (Rust) is date-blind.**
  `origin = date("YYYY-MM-DD")` declares the epoch. `date("YYYY-MM-DD")` in any
  constant position is converted by `expander.ml:parse_date_to_float` to
  `(proleptic-Gregorian day delta) / D(time_unit)` and emitted as `Ir.Const`.
  Date only, no time-of-day. `date()` without `origin` → **E220**.
- **`Model.origin: Option<String>`** round-trips in the IR but **nothing in Rust
  reads it**.
- **The compile-time `date()` path is unexercised:** no golden model uses
  `date()`/`origin`. There is therefore _untested conversion code shipping
  today._

So dates are write-side literal sugar in `.camdl` files and nowhere else; the
data-input and output halves — the ones outside users need — are missing, and
the half that exists is untested.

### 2.1 Verified against the implementation (the seam is already all-floats)

The boundary-translator bet was checked against the code, and it holds — the
internal surface below the I/O edge is **already nothing but `f64` time**:

- **One funnel for time→step.** Every continuous-time → integer-step conversion
  goes through `sim/src/time.rs`: `time_to_step(t, dt) = (t/dt).round()` and
  `interval_steps(t0, t1, dt) = ((t1−t0)/dt).round()`. Interventions, the
  inference obs→substep map (`build_obs_at_substep`), and fire-step resolution
  all call it. There is exactly one place the axis meets the grid.
- **No backend requires obs on dt boundaries; none errors on off-grid times.**
  - `chain_binomial` / `tau_leap` (fixed-step): an off-grid obs **snaps** to a
    step boundary (sim records at the first boundary ≥ t; inference rounds to
    nearest). Snap error ≤ `dt`.
  - `gillespie` (continuous): advances `t` **exactly to the obs/output
    boundary** (`boundary = min(t_end, next_iv, next_out)`), so arbitrary
    fractional/dated times are **exact**, no snap.
  - So a dated obs and the same value typed numerically are byte-identical
    inputs. **§6.5's orthogonality claim is verified for every backend**, and
    the "works with gillespie/tau" requirement is met (Gillespie exact;
    fixed-step snap ≤ dt, exactly as for numeric fractional times today).
- **One time column per data file.** `cli/src/data.rs` resolves a single
  `time_idx`; multiple observation _streams_ share that axis (or live in
  separate `--obs-dir` files under one model origin/`time_unit`). There is no
  "two time-typed columns in one file" case — so a **model-level** time-format
  policy is sufficient; no per-column control is needed.
- **A real dimensional layer already exists.** `ocaml/lib/ir/dimcheck.ml` tracks
  base dimensions **P (population) and T (time)** as exponent vectors, with
  unification and `E302`/`E303` errors, and there is a `param_kind` enum
  (`PRate | PProbability | PPositive | PCount | PReal`). A `time` kind that
  auto-renders dates and catches "time added to a rate" is therefore a _moderate
  extension of existing machinery_, not greenfield (§6.7, §8).

**The seam, stated precisely.** Calendar dates/times exist in exactly two
functions: `parse_time_cell` (in) and the date-renderer (out). Everything below
them — the IR's numeric fields, every backend, every inference kernel — sees
only `f64`. `Model.origin` is the one date-shaped value that crosses inward, and
it is inert at runtime (a string we will pre-resolve to an integer cache). This
seam is the surface to engineer ruthlessly (§6.4, §9).

### 2.2 The core engine is origin-invariant (measured)

A reviewer raised the load-bearing worry that the _engine_ might not be
shift-invariant — that the same data+model fit at a different absolute origin
gives a different likelihood — which would make the dated loader "faithfully
feed a broken engine." **Measured, it is invariant.** Shifting `t_start`, the
data times, and the time-typed params (`tau`) together by `c`, the
particle-filter log-likelihood is _identical_:

| shift c | −20       | −11       | 0         | +11       | +20       |
| ------- | --------- | --------- | --------- | --------- | --------- |
| loglik  | −178.5525 | −178.5525 | −178.5525 | −178.5525 | −178.5525 |

— including **negative** origins (`c=−20`, `t_start=−20`), so the §5.5
negative-time path works when `t_start` is set below the data. Two facts make
this robust and explain the reviewer's _apparent_ non-invariance:

- **The integration window is data-driven, not `simulation.t_end`-driven.** Both
  `pfilter` and the `fit` path (`if2.rs` iterates `for obs_idx in 0..n_obs`,
  propagating `t_start → each obs_time`) run from `t_start` to the **last
  observation**; `simulation.t_end` does not bound the fit. So holding `to`
  fixed while shifting `from`+data (the reviewer's protocol) does **not** change
  the fit — ruling out the `to`-confound for both paths.
- **The remaining failure mode is a model anchor not shifted with the origin.**
  The only way to break invariance is an absolute time value that does _not_
  move when the origin moves — e.g. the seed-timing report model's
  `[fixed]
  t_rep` (the WA testing-onset date) used in
  `rho(t) = …/(1+exp(−(t−t_rep)/w))`. Shift `from`/`tau`/data but leave `t_rep`
  at its absolute value and `(t−t_rep)` changes → the likelihood shifts. This is
  a _modeling_ issue, **not** an engine bug, and it is exactly what the
  `instant` parameter-kind (§6.7) fixes: an `instant` is origin-relative and
  re-anchors with the origin; a bare-`real` absolute anchor is the footgun.

**Consequence for the proposal:** no engine-fix is a prerequisite (the core is
invariant). What _is_ load-bearing is (a) the integration-window spec — derive
it from the data, never floor at `0` (§6.6, now a hard spec) — and (b) that all
time-typed quantities are origin-relative, which the `instant` kind enforces. A
shift-invariance golden (§9.0) locks the property as a regression guard.

---

## 3. The two sides, and what each one needs to know about time

The conversion can run in two places; confusing which side owns what is how this
design goes wrong.

- **Compile side (OCaml, compile-time).** Sees `date()` literals and `origin` in
  the source. Already converts literals to numeric `Ir.Const`. Runs once per
  compile, on values written by the _model author_.
- **Runtime side (Rust, per-run).** Sees the `--data` files. Today it sees only
  numbers. Runs every fit/filter, on values supplied by the _data_, which the
  model author may not control.

The key observation: these two sides do not need to share a floating-point
_pipeline_. They need to agree on one integer fact — the proleptic-Gregorian day
number of a date — and on one constant, `D(unit)`. Everything else stays on its
own side (§5.4).

---

## 4. `dt`, `time_unit`, and observation cadence — three different resolutions

This section exists because the most common design mistake here is to conflate
the integrator step with the time unit, and to reach for sub-day units when the
real need is a smaller step. They are independent axes:

- **`dt` — the integrator step.** How finely the solver discretizes the
  continuous-time dynamics. A **fast/hot epidemic needs small `dt`**: the leap
  condition `dt ≤ 1/(5·r_max)` (`r_max` = the largest per-capita event rate at
  the operating point, in `1/time_unit`) forces it, and `dt` is _part of the
  model_, not a tuning knob. **This is already supported** via `--dt` and is
  fully orthogonal to dates.
- **`time_unit` — the unit of the axis.** `Days`, `Weeks`, … `dt` is measured in
  these units (`dt = 0.1` under `'days` is a tenth of a day). Sub-day units
  (`'hours/'minutes/'seconds`) are a _different_ axis from `dt` resolution.
- **Observation cadence — how often the data is sampled.** What dated columns
  encode. Daily surveillance → daily cadence regardless of how fine `dt` is.

The consequence that drives the design: **a hot epidemic is handled entirely by
`dt`, in whatever `time_unit` the model already uses.** The boarding-school flu
runs in `'days` at `dt = 0.1`; pomp's treatment of the same data uses
`delta.t ≈ 0.083`. You get arbitrary sub-day _integration_ resolution without
any sub-day _unit_. So the date layer neither needs nor touches `dt`:

- **The date layer does not read or write `dt`.** It converts observation times
  to internal-time floats; `dt` governs integration _between_ those times.
- **The date layer introduces no new alignment constraint.** Whatever
  obs-time/`dt` alignment the solver requires today (e.g. observations landing
  on step boundaries) applies unchanged to converted values — a dated obs at
  `t=23` is identical to a numeric obs at `23`. Dates cannot make alignment
  worse than the same value typed numerically.

---

## 5. Risks and bug-surface analysis

### 5.1 The status quo is not zero-risk

The honest baseline is not "robust core, no date code." It is "the core **plus**
an unbounded amount of untested date arithmetic scattered across downstream
notebooks, invisible to camdl," **plus** the existing untested OCaml `date()`
path. The comparison is between _one tested place_ and _many untested places_,
not between _some surface_ and _none_.

### 5.2 Where the new surface lives, and how to bound it

If dates are a boundary translator, all new code is at the I/O edge:

1. **Date string → day number** (parse + proleptic-Gregorian rata die).
2. **Day delta → internal-time float** (subtract origin's day number, divide by
   `D(unit)`).
3. **Internal time → date** (the inverse, for output rendering only).

The dynamical core, the integrator (`dt`), and the inference kernels (IF2, PMMH,
PGAS/CSMC-AS, NUTS) are **not touched** — they receive the same continuous time
axis they receive today. That containment is the entire robustness argument; if
it holds, the core's correctness is unaffected.

### 5.3 The cross-language float surface is small and isolated

The conversion's only floating-point operation is step 2's single division. The
day-number step is exact integer arithmetic — a valid proleptic-Gregorian date
maps to one rata die in any correct implementation — so the **numerator agrees
exactly across languages**. The remaining risk is one division
`(Δdays as f64) /
D(unit)`:

- Under `'days`, `D = 1.0`, so the result is exact (integer-valued f64) for any
  realistic magnitude — _the dominant case is effectively float-free._
- Under `'weeks`, it is `Δdays / 7.0`; under `'months`/`'years`, division by an
  average length.

This is far smaller than the prior draft's "two languages must reproduce
identical IEEE floats across a pipeline." It collapses to: **integer day-delta
exact + one pinned division** (§5.4). No FMA, no accumulated rounding.

### 5.4 The months/years convention is a documented surprise, not a bug

`D(Months) = 30.436875` days is an _average_ month. Under `'months`, monthly
calendar data lands at `0, 1.0185, 1.971, …` (Jan 1 → Feb 1 is 31 days →
`31/30.4369 = 1.0185`), not on integers. With a **continuous** axis this is not
a correctness bug — they are valid float observation times the solver handles
fine.

**This is not really a months problem — it is a cadence-vs-`dt` problem, and the
code settles the policy.** Off-grid is generic: arbitrary dates under `'weeks`
are just as fractional (a date 10 days out is `10/7 = 1.4286`). The integrator
**snaps** any off-grid obs to the nearest step (`time_to_step` rounds; Gillespie
is exact — §2.1), with error ≤ `dt`. Because the leap condition already forces
`dt ≪` dynamics (hence ≪ obs spacing), the snap is negligible. So **warn and
proceed** is correct, and the right check is _unit-agnostic_, not a months ban:

- **Distinct-substep check (the real footgun).** After conversion, assert that
  distinct observation times map to **distinct** substeps:
  `interval_steps(t_start, obs_i, dt)` must be injective over the obs set. The
  only genuine hazard is obs spacing `< dt` (two obs collapsing onto one step) —
  independent of dates, already possible with numeric data, and a true data/`dt`
  mismatch worth a hard error. This subsumes the months worry.
- **Off-grid warning.** When converted times do not land on the `dt` grid, warn
  (`"observations don't align to dt=…; snapped within dt"`) — most useful under
  `'months`/`'years` (never integer-aligned), but emitted by the grid check, not
  by the unit.

So months/years are a _documented surprise_ (average-vs-calendar length), not a
hard error; the load-bearing safety is the distinct-substep check, which
protects every unit. (Constant-day units `'days`/`'weeks` with daily/weekly data
land on integers and trip neither.)

### 5.5 Negative time and arbitrary origin — safe, with two conditions

Arbitrary origin and negative internal time are the _right_ design for seed
estimation (τ can fall before the first observation and before the anchor date),
and a continuous float axis handles negatives natively — a negative day delta is
just a signed value. The conversion layer carries no risk here. Two conditions
make it safe end to end:

1. **Decouple `origin` from the solver's start time.** `origin` is purely the
   I/O calendar anchor (the date mapped to `t = 0`); it carries _zero_ dynamical
   meaning. The time at which the model imposes initial conditions and begins
   integrating — call it `t_init` — is a separate quantity that may be negative,
   fixed, or estimated. The failure mode is any code that treats `t=0` as "where
   the model starts" or "the earliest valid time," which silently truncates a
   seed estimated before the anchor.
2. **Audit `t`-consumers for hidden `t ≥ 0` assumptions.** Closed-form forcing
   (`cos(2π t / 365.25)`, piecewise `t > t_lockdown`) is fine for negative `t`.
   The landmines are: array/time-series **indexing** by `t` (`covariate[t]`
   underflows), **defaults** that floor a missing start at `0` (clips early
   growth → corrupt fit), and **binning/plotting** that assumes non-negativity.
   The integration interval must be _derived_ —
   `[min(t_init, min(obs_t)),
   max(obs_t)]` — never floored at `0`.

   **Concrete instance found in the code (the audit is not hypothetical):**
   `sim/src/time.rs::interval_steps(t0, t1, dt)` computes
   `((t1 − t0)/dt).round() as usize` with `debug_assert!(t1 >= t0)`. An
   observation earlier than the integration start `t0` (= `t_start`)
   **underflows the `usize`** in release and panics in debug. So negative
   _internal_ time is safe **iff** `t_start ≤ min(obs_time)` always holds — i.e.
   condition (1)'s derived interval is mandatory, and `t_start` must never be
   silently floored to `0`/`origin`. This single function is the
   highest-priority audit target before any negative-time data is fed in.

### 5.6 The failure mode that makes this _not_ worth doing

This is worth it **only if the boundary holds**. The design becomes a net
negative if either: (a) date handling leaks into the core (e.g. dates start
constraining `dt` or the integration grid), or (b) two un-pinned date parsers —
an OCaml hand-rolled one and Rust `chrono`/`time` — accept slightly different
fringe forms, so a date that compiles fails to load or vice versa. The
mitigations (§6.4) are non-optional: one pinned grammar, one shared integer
algorithm, one golden table.

---

## 6. Proposed architecture

**Principle: dates are a boundary translator over an unchanged continuous core;
the integrator step `dt` is untouched.**

### 6.1 Semantic model

- An **instant** is a **naive** proleptic-Gregorian **date** `YYYY-MM-DD` (ISO
  8601). "Naive" means _no timezone at all_ — **not** "assume UTC." The date is
  a **civil-calendar label**, and its `rata_die` is computed straight from
  `(Y, M, D)`; no zone, offset, or instant interpretation ever enters. This is
  precisely what makes the same date string behave identically for a modeler in
  any timezone (§6.8). No time-of-day in v1.
- **`origin`** is the instant mapped to internal time `t = 0`.
- **Conversion (observation times):**
  ```
  Δdays      = rata_die(instant) − rata_die(origin)      # exact integer
  t_internal = Δdays / D_days(time_unit)                 # f64; exact under 'days
  ```
  where `rata_die(·)` is the proleptic-Gregorian day number and `D_days(unit)`
  the days-per-unit. `t_internal` is a float and may be negative. No integrality
  is required (the core is continuous); `'months`/`'years` get a warning (§5.4),
  not an error.
- A **bare number** in a time position is already internal time. A **date** is
  an instant, converted as above. The two are never mixed implicitly; the data
  loader decides per column (§6.3).

This subsumes the existing `date()`-as-midnight behavior and adds nothing the
core must learn.

### 6.2 Compile side (OCaml) — mostly hardening

- `date()` literal semantics are **unchanged**: a date in a constant position is
  converted to `Ir.Const` via the day-number formula.
- **New:** parse `origin` once, at compile time, to its canonical **numeric**
  form (its `rata_die` integer) and serialize _that_ into the IR alongside the
  string. The runtime then never re-parses the origin string — removing one of
  the two date-string parsers from the shared contract.
- **New:** the day-number algorithm and `D` table move behind a spec'd interface
  and gain the golden table (§6.4). This hardens code that ships today untested.

### 6.3 Runtime side (Rust) — the deliverable

- A single `parse_time_cell(cell, origin_rata_die, time_unit)` shared by every
  consumer (`simulate --obs`, `fit`, `pfilter`, `profile`, `compare`), so they
  all gain dated input together.
- **Column-type detection over the whole column** (not the first cell): if every
  non-empty cell parses as `f64` → numeric column (today's behavior); if every
  cell parses as an ISO date → dated column; mixed → hard error. Genuine
  ambiguity → error, not a guess. `--time-format numeric|date` is the explicit
  override, honored _before_ detection.
- **Conversion** (§6.1): `rata_die(cell) − origin_rata_die`, divided by
  `D_days(unit)`. A dated column with no `origin` in the IR → clear error. The
  resulting float flows into exactly the same path numeric data uses today; it
  does not interact with `dt` (§4).

### 6.4 The shared contract (the only thing both sides must agree on)

- **One algorithm:** the proleptic-Gregorian `rata_die` function (e.g.
  days-from-civil). Integer-valued, total on valid dates, implementation-
  independent — so OCaml's hand-rolled version and Rust's produce identical
  integers by construction.
- **One grammar:** a small EBNF for the accepted date string, specified once.
  Both parsers accept exactly this set, _including_ a date bearing a trailing
  zone designator (`Z` / `±HH:MM`), which is accepted and reduced to its civil
  date (§6.8). The **rejection** set (malformed cell, datetime forms in v1) is
  tested too.
- **One golden table:** committed `(origin, date, time_unit) → expected_value`
  fixtures, checked by _both_ an OCaml and a Rust test. The day-delta is
  compared as an **exact integer**; the final float is compared to ≤1 ULP, with
  the division literal and operation order pinned in the spec. (Under `'days`
  this is exact.)

### 6.5 `dt` is orthogonal (made explicit)

The date layer reads and writes no `dt`. Converted observation times are floats
identical to the numeric values they replace, and inherit — never alter — the
existing obs-time/`dt` alignment behavior. A fast/hot epidemic is served by
choosing small `dt` (§4), in whatever `time_unit` the model uses; it requires no
date-layer change and no sub-day unit.

### 6.6 The integration window + origin-relativity (hard spec, not an audit note)

This is a spec, because §2.2 shows it is where a _non_-invariance would come
from if it were left implicit:

- **`t_init` is `simulate.from`.** The model already has it; it is the
  initial-condition time and the integration start. It **may be negative** and
  may be estimated. `origin` (the I/O calendar anchor) is _separate_ and carries
  no dynamical meaning.
- **The integration window is derived from the data, never floored.** The fit
  runs `t_start → last observation` (data-driven; not `simulation.t_end` —
  §2.2). The loader sets `t_start = min(t_init, min(obs_t))` and the end at
  `max(t_init-window-end, max(obs_t))`. An observation outside the window
  **auto-extends** the window (or hard-errors) — it is **never silently
  truncated**, and `t_start` is **never** floored to `0`/`origin`.
- **Guard the `interval_steps` underflow (§5.5).** With the derived window
  `t_start ≤ min(obs_t)` always holds, so `interval_steps(t_start, obs, dt)` is
  non-negative by construction; add an explicit check/test so a future
  regression surfaces as an error, not a `usize` wrap.
- **All time-typed quantities are origin-relative.** `instant`-kind params
  (`tau`, `t_init`, and any reporting-onset `t_rep`) are anchored to `origin`
  and shift _with_ it; re-anchoring the origin moves them together (§2.2's
  failure mode is a `[fixed]` absolute `t_rep` that does not). This is the
  model-level invariant the `instant` kind (§6.7) enforces.

### 6.7 Output rendering (report results as dates)

- When `origin` is set, time-typed estimands (seed time τ, `t_init`) render as
  dates in summaries/reports via the inverse map
  `instant = origin + t ·
  D_days(unit)` days. The chapter's "τ ≈ day 23"
  becomes "τ ≈ 2020-02-13".
- **v1 decision (taken): a first-class `time` parameter-kind, with dimensional
  checking.** "Which estimands are time-typed" is declared _in the model_ (where
  it belongs), not per report. Two kinds, both carrying dimension `[T]` (the
  `dimcheck` layer already tracks `T`, §2.1):
  - **`instant`** — an absolute point (τ, `t_init`); renders as a **date**
    against `origin`. Scales to a `τ[location]` vector for free.
  - **`duration`** — a relative span (a generation interval, a reporting delay);
    renders as a **span**, never a date, needs no `origin`. Because both are
    `[T]`, the dimensional checker _gains time coverage as a feature_:
    `rate * duration` is dimensionless (OK), `rate + instant` is an `E302`
    dimension mismatch (a real modeling bug caught at compile time). The
    rendering uses the instant/duration tag; the checking uses the `[T]`
    dimension. _(Surface detail to settle in implementation: two kinds
    `instant`/`duration` vs. one `time` kind with an instant/duration modifier —
    both express the same thing; recommend two kinds for readability.)_
- An optional calendar **column** in trajectory/obs output stays behind
  `--dates`; numeric `t` remains the canonical, diff-stable default.

### 6.8 Timezones and international / multi-source data (v1 policy)

This is the section to read if you (or an agent) wonder whether naive dates
break for international data spanning many timezones. They don't — this is the
case naive handles _best_ — but the reasoning is worth stating.

**Naive is timezone-_independent_, not UTC-assuming.** Epidemic surveillance is
labeled by **local civil day**: "47 cases on 2020-03-15 in Kano" denotes that
local calendar day, with no absolute instant attached. The date is a label, not
a moment. Because `rata_die` is computed from `(Y, M, D)` directly (§6.1), the
zone never participates, so two modelers in any two timezones who write
`2020-03-15` get the _same_ day-number against the same origin. Zone-awareness
would be the surprising thing: it would require an instant interpretation that
shifts day boundaries (Bangladesh's local midnight is a different instant from
Nigeria's), reintroduce DST and so break the constant `D(Days)=1` the arithmetic
relies on, and put civil days on fractional `t` for offsets like Nepal's
`+05:45`.

**Pooled multi-country example.** Suppose you aggregate three countries into one
file, and the exports stamped offsets:

```
date,             country,     cases
2020-03-15+01:00, Nigeria,     47
2020-03-15+06:00, Bangladesh,  30
2020-03-15-03:00, Brazil,      22
```

For a bare date, the offset cannot change which calendar day it is, so all three
cells reduce to `rata_die(2020, 3, 15)` → the **same** `t`. The offsets are
discarded; every row aligns by civil date onto the one shared internal axis.
That is the correct, unsurprising result for daily/weekly surveillance.

**Two alignment semantics — the right one is civil-date.** "March 15 across
three countries" can mean _civil-date alignment_ (all the same model day; naive
gives this) or _absolute-instant alignment_ (each midnight a different instant,
the same date string splitting into three fractional `t`). For daily/weekly
counts, civil-date alignment is correct: each country's incidence accumulates
over its own local civil day, generated by that country's day-15 dynamics. The
sub-day offset is below the data's resolution — noise, not signal — and instant
alignment would inject spurious sub-day precision and shift whole series by
non-integer amounts.

**Separate calendar alignment from epidemic-phase alignment.** "Are these the
same model day?" is the _calendar_ question (naive civil dates, one shared
`origin`). "Did the outbreak start at the same time in each country?" is a
_modeling_ question (per-location seed time τ_i, which you estimate anyway). The
calendar layer needs no zone cleverness: countries share the axis; their
epidemics sit at different τ_i on it.

**The one case naive does not serve** — sub-day events collected in _different_
zones that must be aligned to a common absolute instant — is datetime territory
(§6.9), rare in compartmental modeling (you bin to civil days regardless), and
best normalized upstream.

**Honesty note (no policy can fix this):** if an upstream export computed the
date field in UTC rather than local time, a late-night local event can already
be mislabeled to the adjacent civil day in the file. Once the original timestamp
and zone are gone, camdl cannot recover the intended day, and a zone-"clever"
parser would risk making it worse. camdl's contract is to **trust the civil date
as given**; correct date-labeling is an upstream responsibility.

### 6.9 Sub-day resolution: two separable features, both deferred from v1

A fast/hot epidemic needs **neither** of these — it needs small `dt` (§4), which
exists. These are needed only for sub-day _observation cadence_ or sub-day
reporting/units:

- **(F1) Sub-day `time_unit`s** (`'hours/'minutes/'seconds` — extra rows in
  `D(unit)` and the enum). **Cheap**, precisely because the core is continuous:
  no integral check to satisfy. Useful for hour-denominated rates/reporting and
  within-host models. But only _useful_ in combination with sub-day input or
  numeric times. Can be added in isolation when a real need appears.
- **(F2) Datetime input** (parsing `YYYY-MM-DDTHH:MM:SS` in data columns and
  `origin`). **The expensive part**: a time-of-day grammar, a sub-day day-number
  representation, and — critically — a trailing offset becomes **load-bearing**
  here (on a datetime it shifts the instant, unlike on a date, §6.8), so this is
  where real timezone semantics enter. Defer until a genuinely
  sub-day-_cadenced_ dataset exists.

Also deferred: an **always-on** calendar output column (kept behind `--dates`).
_(The `time` parameter-kind with instant/duration + dimensional checking is **in
v1** — §6.7 — not deferred.)_

---

## 7. Phasing (commits)

1. **Shared `rata_die` + golden table; numeric origin in IR.** Extract the
   day-number algorithm and `D` table behind a spec'd interface; serialize the
   numeric origin; commit the golden table tested both sides. _Hardens
   already-shipping untested code; no user-visible behavior change._
2. **Dated data loader.** `parse_time_cell` (numeric-or-date), whole-column
   detection, `--time-format` override, origin-missing error, `'months`/`'years`
   warning. _The deliverable outside users need._
3. **`time` parameter-kind + dimensional checking.** Add `instant`/`duration`
   param kinds (dimension `[T]`) to OCaml + the IR + Rust; `dimcheck` gains time
   coverage (`rate + instant` → `E302`; `rate * duration` → dimensionless OK).
   Independent of the loader; hardens the model layer.
4. **Date-rendered results.** `instant`-kind estimands render as dates when
   `origin` is set (inverse map); `duration` as spans; optional `--dates` output
   column.
5. **(Deferred)** F1 sub-day units when a sub-day need appears; F2 datetimes +
   timezones when a sub-day-cadenced dataset appears.

Each commit: `cargo test` + `dune runtest` green; the golden table is the
correctness anchor. None of these phases touches `dt` or the integrator. Phase 1
is independent of the `time`-kind work (phase 3), so they can proceed in either
order; the loader (phase 2) depends only on phase 1.

---

## 8. Open questions / decisions

**Settled by the code (no input needed):**

- **Off-grid / `'months`/`'years`** → warn and proceed + the distinct-substep
  check (§5.4). The integrator snaps within `dt`; refusing non-day units is
  unnecessary.
- **`--time-format` surface** → `numeric|date` in v1; a single model-level
  policy suffices because a data file has one time column (§2.1). `datetime` is
  added with F2; until then an unknown value is rejected (clearer than a
  known-but-broken one).
- **Multi-column time** → not a case; no per-column control (§2.1).

**Decided:**

1. **`τ`-as-a-date → the principled `time` parameter-kind, with dimensional
   checking** (instant/duration; §6.7). Date-rendering _and_ `rate + time`
   compile-error coverage, declared in the model, scaling to `τ[location]`.
   Built in v1 (it's a moderate extension of the existing
   `dimcheck`/`param_kind` machinery, not greenfield).

**Genuinely your call (input still wanted):**

2. **Do you actually fit `'months`/`'years` models with dated data**
   (endemic/demographic turnover), or is that hypothetical? This only sets _how
   much to invest in the warning copy_ — warn-and-proceed stands either way. If
   you have a real years-unit dated model, the warning matters more and deserves
   a worked test; if hypothetical, a one-line warning suffices.

**Confirm (recommendation stands unless you object):**

3. **Origin in IR** — carry both: keep `Model.origin: Option<String>` for
   display/debuggability + F2 future-proofing, add a compiler-derived numeric
   `origin_rata_die` cache the runtime reads. The integer is _derived_
   (generated by OCaml alongside the string), never hand-edited, so they cannot
   drift — "authoritative" is the wrong worry. _(Nothing currently reads
   `origin` back from the IR, so numeric-only is also defensible; carrying both
   costs ~nothing and helps. This is a small IR schema change — both
   serializers + golden regen, atomically, per CLAUDE.md.)_

4. **F1 sub-day units** — defer (recommendation): covered by `dt` for fast
   dynamics; only useful with F2 datetimes. Flip to "now" only if you have an
   imminent genuinely sub-day-_denominated_ model (within-host kinetics, hourly
   nosocomial counts) wanting `per_hour` rates.

---

## 9. Testing — the seam is small, so it must be exhaustively pinned

The whole robustness argument is "a tiny boundary, ruthlessly tested." A small
surface earns a _dense_ test suite. Organized by layer, smallest first; every
item is a concrete test, not a property to assume.

### 9.0 Backward compatibility — numeric/indexed times are untouched

The numeric path is the default and must not move. Dates are purely additive.

- **Indexed integers `0, 1, 2, …` load byte-identically to today**, with **no
  `origin` required** — an all-numeric time column takes the existing `f64`
  path; the date layer never engages (whole-column detection sees numbers).
- **Fractional numeric times** (`0.0, 0.5, 1.0`) behave exactly as before.
- **Every current golden / fixture / fit re-runs unchanged** (no model declares
  `origin`, so nothing converts). This is the regression wall: if any existing
  numeric-time test output shifts, the date layer has leaked where it must not.
- A model **with** `origin` but a **numeric** data file still uses the numeric
  values directly (origin affects only _date_ cells and output rendering) — so
  adding `origin` for date-rendering never silently reinterprets numeric data.

### 9.0.1 Shift-invariance golden — run on the numeric engine, before any date code

The engine must give the same likelihood under a consistent change of origin;
this is the property the dated loader _relies on_, so it is pinned independently
of dates (it currently **passes** — §2.2):

- **Numeric-engine shift-invariance:** for a fixed θ, shift
  `(t_start, the data
  times, every time-typed param)` by
  `c ∈ {−20, −11, +11, +20}` and assert the log-likelihood is **bit-identical**
  to `c=0`. Run on `pfilter` and on a `fit` iteration. Negative `c` (origin
  after the first obs → negative internal times) is included.
- **Anti-test (catches the real footgun):** shifting `t_start`+data+`tau` but
  **leaving an absolute anchor** (`t_rep` as a bare `real`) unshifted **must**
  change the loglik — and the same model with `t_rep` declared `instant` (so it
  re-anchors) **must** stay invariant. This locks the §6.7 origin-relativity
  contract.

### 9.1 The conversion core (`rata_die` + `D` table), per language

- **Leap-year correctness:** `2000-03-01` vs `2000-02-28` (leap), `1900-03-01`
  vs `1900-02-28` (century non-leap), `2020-02-29` (valid), `2021-02-29`
  (rejected). Feb-29 around all four century rules.
- **Month-boundary deltas:** Jan→Feb (31), Feb→Mar (28/29), Dec→Jan (year roll),
  31-day vs 30-day months.
- **Sign + zero:** date == origin → 0; date before origin → exact negative
  integer delta; symmetry `delta(a,b) == −delta(b,a)`.
- **`D(unit)` division:** under `'days` the result is integer-valued f64 (assert
  exactly, no tolerance); `'weeks` = Δ/7; `'months`/`'years` against the pinned
  `365.2425` constants.
- **Round-trip:** `instant → t → instant` recovers the input date for a sweep of
  dates × units (the inverse map, used by output rendering).
- **Range:** dates across the supported span (≥ 1583 CE) without overflow.

### 9.2 Cross-language golden (the contract)

- A committed `(origin, date, time_unit) → {Δdays, t_internal}` table, run by
  **both** a `dune` test and a `cargo` test. Δdays compared as **exact
  integers**; `t_internal` to ≤1 ULP (exact under `'days`). The division literal
  and operation order are pinned in the spec so the ULP claim is meaningful.
- **Compile-vs-runtime agreement:** a model with a `date()` literal and a data
  file with the _same_ date produce the **same** internal float — the property
  that would silently break if the two parsers diverged.

### 9.3 The date grammar (accept/reject), shared by both parsers

- **Accept:** `YYYY-MM-DD`; with trailing zone designator (`Z`, `+HH:MM`,
  `−HH:MM`) → reduced to civil date, zone discarded (§6.8).
- **Reject (with a clear message, both languages identically):** datetime forms
  (`…T12:00`, v1), `YYYY/MM/DD`, `DD-MM-YYYY`, 2-digit years, out-of-range month
  /day (`2020-13-01`, `2020-02-30`), empty, non-numeric components, whitespace
  variants. The reject set is a test fixture, not folklore.

### 9.4 The data loader (`parse_time_cell` + column detection)

- **Whole-column detection:** all-numeric → numeric; all-date → dated; **mixed
  numeric+date → hard error** (named offending row).
- **`--time-format` override** honored before detection; `date` forces parsing,
  `numeric` forbids it.
- **Origin-missing:** dated column with no `origin` in the IR → clear error.
- **Distinct-substep check (§5.4):** two obs that map to the same substep under
  the run's `dt` → hard error naming both rows; obs spacing ≥ `dt` passes.
- **Off-grid warning:** converted times not on the `dt` grid → warning emitted
  (test it fires under `'months`, and does _not_ under daily-dates/`'days`).
- **Byte-identity:** a dated TSV yields **byte-identical observation vectors**
  to the same data hand-converted to day-numbers (the user's old fetch-script
  output) — the core promise.

### 9.5 Negative time / the `interval_steps` landmine (§5.5)

- **Regression for the underflow:** an obs earlier than `t_start` must be a
  caught error (or `t_start` correctly derived below it), never a `usize`
  underflow. Add a direct `interval_steps` unit test for `t1 < t0`.
- **Derived interval:** with a seed before `origin`, the integration interval is
  `[min(t_init, min(obs_t)), max(obs_t)]`, never floored at `0`/`origin`.
- **End-to-end:** a seed before the anchor yields negative `t`, integrates,
  fits, and renders to a date _before_ `origin`.

### 9.6 Backend equivalence (gillespie / tau-leap / chain-binomial)

- **Orthogonality:** for each backend, a dated fit and the identical
  numeric-time fit produce the same result (Gillespie exact; fixed-step within
  the backend's own `dt` snap) — dates change values, never integration.
- **`dt` independence:** the same dated fit at `dt ∈ {1.0, 0.5, 0.1}` matches
  the numeric-time fit at those steps; the Richardson dt-convergence verdict is
  unchanged by whether time arrived as dates or numbers.

### 9.7 International / multi-source — a committed, loaded-and-verified fixture (§6.8)

Not just an assertion: a real multi-timezone dataset is committed and run end to
end, so the civil-date policy is exercised, not assumed.

- **`tests/fixtures/dated_multitz.tsv`** — pooled rows whose date cells carry
  mixed trailing offsets (`2020-03-15+01:00` Nigeria, `2020-03-15+06:00`
  Bangladesh, `2020-03-15−03:00` Brazil, plus a `Z` row and a Nepal `+05:45`
  row), with case counts.
- **Verification:** loading it produces the **identical** internal-time vector
  as a sibling `dated_multitz_naive.tsv` with every offset stripped — every row
  maps to `rata_die(2020,3,15)` → the same `t`. The offset is discarded; no row
  lands on a fractional `t` (the `+05:45` row would, if a zone leaked in — so
  this catches a regression that re-introduces zone-awareness).
- **Civil-date alignment (distinct dates):** a second fixture with genuinely
  different civil dates per row (`2020-03-15`, `2020-03-16`, `2020-03-17`) maps
  to consecutive integer `t` under `'days` — confirming dates that _should_
  differ do, while same-civil-date-different-offset rows collapse.
- **Datetime-with-offset is rejected** in v1 (a row like
  `2020-03-15T23:30+06:00` errors clearly — offsets are load-bearing only on
  datetimes, which are F2).

### 9.8 Output rendering

- Time-typed estimands render as dates when `origin` is set (`τ ≈ day 23` →
  `2020-02-13`); the `--dates` output column matches the inverse map; numeric
  `t` remains the canonical default.

### 9.9 The `time` parameter-kind + dimensional checking

- **Dimension assigned:** an `instant`/`duration` parameter is `[T]`; the
  checker sees it as time (a small `dimcheck` golden).
- **Catches real bugs:** `beta + tau` (rate `[1/T]` + instant `[T]`) → `E302`;
  `beta * gen_interval` (rate × duration) → dimensionless, accepted;
  `S * tau / N` and other `[T]`-using rates check out correctly.
- **Rendering split:** an `instant` renders as a date against `origin`; a
  `duration` renders as a span and needs no `origin` (rendering a `duration`
  with no origin is fine; rendering an `instant` with no origin falls back to
  numeric with a note).
- **Vector:** a `τ[location]` (instant-kind, stratified) renders each component
  as a date — the case a per-report fixed set could not handle.

### 9.10 Integration regression

- **Seed-timing:** the chapter's COVID-WA fit on the _dated_ source with
  `origin = date("2020-02-28")` reproduces the current day-number fit, with τ
  reported as a date — and the fetch script's hand-anchoring step is deleted.
