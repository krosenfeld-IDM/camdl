# camdl DSL cheatsheet

Orientation doc — what the DSL supports, in one place, with pointers to the
normative sources. **Not the spec.** When this disagrees with
[`docs/camdl-language-spec.md`](camdl-language-spec.md), the spec wins.

This file exists because the DSL surface is large enough that an agent or new
contributor working from memory or from a single proposal often misses
features the language already provides, then reinvents them poorly. Read this
first when proposing DSL changes.

> **Status conventions.** Things in the language *today* are unmarked. Things
> coming via the in-flight typed-time proposal
> (`docs/dev/proposals/2026-05-22-typed-time-and-dsl-ergonomics.md`) are
> tagged **(proposed: typed-time)** — described here so this doc orients you
> to where the language is going, but flagged so you don't ship code that
> assumes them before they land. When the proposal lands, drop the tags.

## Time and units

**Time unit declaration** sets the model's internal axis:

```camdl
time_unit = 'days        # also: 'weeks, 'months, 'years
```

All durations and rates normalise to this unit at compile time. The runtime
only sees `f64` values in this unit — `time_unit` itself isn't read in the
dynamics path (only at I/O boundaries for date conversion).

**Numeric literals carry units** — prefix-apostrophe syntax:

```camdl
5 'days       # duration, dimension [T]
14 'days
2 'weeks
0.5 'years
0.1 'per_day  # rate, dimension [T⁻¹]
0.02 'per_year
1.0 'ratio    # dimensionless multiplier (sinusoidal forcings, etc.)
100 'count    # raw population count
```

Available unit literals (defined in `ocaml/lib/compiler/lexer.mll`):

| Class      | Literals                                              | Dimension  |
|------------|-------------------------------------------------------|------------|
| Duration   | `'days`, `'weeks`, `'months`, `'years`                | `[T]`      |
| Rate       | `'per_day`, `'per_week`, `'per_month`, `'per_year`    | `[T⁻¹]`    |
| Count      | `'count`                                              | `[P]`      |
| Multiplier | `'ratio`                                              | `[1]`      |

**Conversion table** (`days_per_unit` in
`rust/crates/ir/src/caltime.rs` and the mirror in `expander.ml`,
proleptic-Gregorian throughout):

```
1 'day   = 1                 day
1 'week  = 7                 days
1 'month = 365.2425 / 12     days  ≈ 30.4369
1 'year  = 365.2425          days
```

**Mixed-unit arithmetic** works through the dimensional checker:

```camdl
5 'days + 3 'days        # = 8 'days
1 / (14 'days)           # = rate (1/time)
0.1 'per_day * 5 'days   # = 0.5 (dimensionless)
5 'days + 0.1 'per_day   # ERROR E302: cannot add time and rate
```

## Dimensional information — three tiers

For full background see `docs/camdl-language-spec.md` §2.3.

| Tier | Syntax | Carries | Use when |
|------|--------|---------|----------|
| 1. Kind keyword | `rate`, `probability`, `count`, `positive`, `real`, `instant`, `duration` | dimension (inferred from kind) | parameter declarations — the 99% case |
| 2. Bracket annotation | `[T]`, `[T^-1]`, `[P]`, `[P/T]`, `[1]` | dimension only | kind is under-determined (`real`/`positive`) |
| 3. Unit literal | `'days`, `'per_day`, `'count`, `'ratio`, … | dimension *and* scale | concrete numeric values with a real-world scale |

Tiers are complementary, not redundant — tier 3 carries *scale*, the others
don't. A parameter from a prior or `--params` file lives at tier 1 or 2
(scale is implicit in the model's `time_unit`); a literal like `5 'years`
lives at tier 3.

## Parameter kinds

```camdl
parameters {
  beta     : rate                          # [T⁻¹], log transform for inference
  rho      : probability                   # [1] bounded [0,1], logit transform
  R0       : positive in [1.0, 20.0]       # >0, log transform, with bounds
  N0       : count                         # [P], integer ≥ 0
  alpha    : real                          # unconstrained
  tau      : instant in [date("2020-01-01"), date("2020-04-30")]
                                            # [T] absolute time, renders as date
  delta    : duration in [1 'days, 60 'days]
                                            # [T] span, renders as span
}
```

`instant` and `duration` are the time-typed kinds; see
[`docs/dates.md`](dates.md) for full date semantics.

## Dates and calendar arithmetic

**`date("YYYY-MM-DD")`** in DSL constant positions converts to internal time
via `origin`:

```camdl
origin = date("2020-02-24")

simulate {
  from = date("2020-01-21")    # = t − 34 days from origin (in time_unit)
  to   = date("2020-06-27")
}
```

Without a top-level `origin`, `date(...)` is **E220**.

**Anchored vs unanchored** models (vocabulary introduced by the typed-time
proposal — see `docs/dev/proposals/2026-05-22-typed-time-and-dsl-ergonomics.md`):

- **Anchored**: declares `origin`. Internal axis maps to real calendar
  dates. Must use `time_unit = 'days` or `'weeks` (constant-day rule).
- **Unanchored**: no `origin`. Internal axis is abstract; bare numbers.
  Any `time_unit` is fine including `'months`/`'years`. SBC, synthetic,
  textbook SIR live here; so do the dacca SIRS models.

**Anchor-only primitives** — these require `origin` to be declared:

| Construct                       | Anchor-only? | If used unanchored                |
|---------------------------------|--------------|-----------------------------------|
| `date("YYYY-MM-DD")`            | yes          | E220 (existing)                   |
| `add_calendar_months(d, n)`     | yes          | E3xx targeted error (new)         |
| `add_calendar_years(d, n)`      | yes          | E3xx targeted error (new)         |
| `instant`-kind param (rendering)| yes          | works as `[T]`; no date rendering |
| `5 'months`, `5 'years`         | **no**       | legal — affine span               |
| `0.087 'per_month`              | **no**       | legal — affine rate               |
| `time_unit = 'months`/`'years`  | **no**       | legal (Rule 2 only fires anchored)|

The bottom rows are *calendar-named affine constructs*, not anchor-only —
the dacca SIRS configuration (unanchored, monthly axis, per-month rates,
month-span durations) is all of those, and it remains fully legal.

**Calendar arithmetic** (proposed; not yet in the language as of this
writing):
- `add_calendar_months(d, n)`, `add_calendar_years(d, n)` for stepping
  dates by calendar (non-affine, with month-end clamping).
- `Instant + 'months` and `Instant + 'years` are *hard errors* in
  anchored mode — calendar months/years aren't invertible spans. Use
  `add_calendar_*` instead.

## Periodic forcings — already calendar-friendly

```camdl
forcing {
  school : periodic 'ratio {
    period = 365.25 'days
    step   = 1 'days
    on     = [7:100, 115:199, 252:300, 308:356]
  }

  reporting_dow : periodic 'ratio {
    period = 7 'days
    values = [1.2, 1.1, 1.0, 1.0, 0.9, 0.8, 0.7]
  }
}
```

Every forcing declaration carries a **tier-3 unit literal** between the
kind keyword and the block (`sinusoidal 'ratio`, `interpolated 'count`,
etc.). This is required per GH #8.

## Common diagnostics

The compiler issues E-codes with source locations and (per the
typed-time proposal's discipline) fix-hints.

| Code | Class | Typical trigger |
|------|-------|----------------|
| E100 | naming | parameter name shadows reserved (`t`, etc.) |
| E203 | indexing | named-index references wrong dimension |
| E220 | date | `date(...)` without `origin` declared |
| E300 | dim | transition rate not P·T⁻¹ |
| E301 | dim | non-dimensionless argument to `exp`/`log` |
| E302 | dim | addition/subtraction of mismatched dimensions |
| E303 | dim | parameter used with inconsistent dimensions |
| E304 | dim | `sqrt` of odd-exponent dimension |
| E305 | dim | balance expression must have dimension P |
| E306 | dim | ODE derivative must have dimension P·T⁻¹ |
| E308 | dim | overdispersion σ² must be dimensionless |
| W301 | forcing | periodic range not aligned to step size |

## Where things live

- **Lexer (tokens, unit literals):** `ocaml/lib/compiler/lexer.mll`
- **Parser (grammar):** `ocaml/lib/compiler/parser.mly`
- **AST:** `ocaml/lib/compiler/ast.ml`
- **Expander (stratification + IR emission):** `ocaml/lib/compiler/expander.ml`
- **Dimensional checker:** `ocaml/lib/compiler/dimcheck.ml`
- **IR types (Rust):** `rust/crates/ir/src/`
- **Calendar conversion (Rust):** `rust/crates/ir/src/caltime.rs`
- **IR schema (the OCaml↔Rust contract):** `ir/schema.json`
- **Language spec (authoritative):** `docs/camdl-language-spec.md`
- **User-feature tour:** `docs/user-features.md`
- **Calendar reference:** `docs/dates.md`

## Pitfalls that have actually bitten us

These are real failure modes that have produced incident reports. Read
them before assuming the language doesn't do something — it usually does.

- **Reinventing existing surface.** The DSL already has `5 'days`,
  `0.087 'per_month`, range syntax `[7:100]`, the dimensional checker,
  and `instant`/`duration` parameter kinds. Before adding a new
  duration / rate / cadence / unit construct, grep this cheatsheet and
  the language spec.
- **Cross-language constants disagree.** Anything that has to agree
  across OCaml and Rust either lives in one place or has a test pinning
  them — never two hand-maintained copies. See
  `docs/dev/incidents/2026-05-22-dual-month-conversion-constants.md`.
- **Calendar months aren't durations.** "+1 month" depends on its
  input *and* on the year:
  `date("2021-01-31") + 1 month = date("2021-02-28")` but
  `date("2020-01-31") + 1 month = date("2020-02-29")` (leap). It's an
  instant operation, not a translation. The language enforces this
  through the ExactDuration/CalendarDuration split. See the typed-time
  proposal.
- **Hard errors over warnings.** When a construct is silently
  ambiguous, the compiler hard-errors with a fix-hint rather than
  warning. This is CLAUDE.md policy and the typed-time proposal's
  acceptance criterion.

## Recent and incoming changes

For things this cheatsheet may lag on, check:

- `docs/dev/proposals/` for in-flight design.
- `docs/dev/incidents/` for known bugs and their resolutions.
- `git log -- ocaml/lib/compiler/lexer.mll ocaml/lib/compiler/parser.mly`
  for actual grammar changes.
