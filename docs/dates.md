# Dates and calendar time

camdl's internal time is a **continuous number** in units of the model's
`time_unit`, measured from an `origin`. Real epidemic data is dated. This page
is the single reference for how calendar dates relate to internal time across
the DSL, the data loader, and output — so you can point camdl at a dated file
and read results back as dates, without pre-converting anything by hand.

## The one rule

Dates live **only at the I/O boundary**. Two places translate; everything below
them is a plain `f64`:

```
ISO date  ──parse_time_cell──▶   internal time (f64)   ──date-renderer──▶  ISO date
 (data in, date() literal)          (the whole engine)                     (output)
```

An **instant** is a calendar point, written ISO 8601 `YYYY-MM-DD`. `origin` is
the instant mapped to internal time `t = 0`. Then, for any instant `T`:

```
t = (rata_die(T) − rata_die(origin)) / D(time_unit)        # internal time
T = origin + t · D(time_unit) days                          # the inverse
```

`rata_die` is the proleptic-Gregorian day number; `D(time_unit)` is days-per-unit
(`days=1, weeks=7, months=365.2425/12, years=365.2425`). `t` may be **negative**
(a date before the origin — e.g. a seed time before the first observation) and
fractional under non-day units. A **bare number** in any time position is
*already* internal time; a **date** is converted by the rule above. The
conversion is identical in the OCaml compiler and the Rust runtime (one shared
`rata_die`), so a `date()` literal and the same date in a data file agree exactly.

## Writing dates in a model

```camdl
time_unit = 'days
origin    = date("2020-02-24")        # t = 0 is this calendar day

parameters {
  tau   : instant                      # an absolute time → renders as a date
  delay : duration                     # a relative span → renders as a span
  beta  : rate
}

simulate {
  from = date("2020-01-21")            # = t -34 (34 days before the origin)
  to   = date("2020-06-27")
}
```

- **`date("YYYY-MM-DD")`** is usable anywhere a constant is expected (`origin`,
  `simulate { from/to }`, scheduled event/intervention times). It compiles to the
  internal-time number. A `date()` without a top-level `origin` is an error
  (**E220**).
- **`instant` and `duration` parameter kinds** carry dimension `[T]`, so the
  dimensional checker now covers time:
  - `rate + instant` (or `rate + duration`) is a dimension mismatch (**E302**) —
    a real modeling bug caught at compile time.
  - `rate * duration` is dimensionless (a valid per-event probability factor).
  - An `instant` renders as a **date** against `origin`; a `duration` renders as
    a **span** (no origin needed). Both are origin-relative where it matters: if
    you move `origin`, declare your time anchors as `instant`/`duration` so they
    move with it (a bare `real` anchor will *not*).
  - An `instant` may take a **negative** lower bound — e.g. a seed time that
    falls *before* the origin: `tau : instant in [-40, 120]`. (Negative bounds
    are preserved for `instant`/`real` kinds; `rate`/`positive`/`count` remain
    non-negative by their nature.)

## Loading dated data

The `--data` time column accepts **either** numeric internal time **or** ISO
dates — detected automatically per column:

| column looks like | treated as | needs `origin`? |
|---|---|---|
| `0, 1, 2, …` (any `f64`) | internal time, used directly | no |
| `2020-03-15, …` (ISO dates) | converted via `origin` + `time_unit` | yes |
| mixed numeric + date | hard error | — |

```bash
# dated column, auto-detected and converted via the model's origin
camdl fit run fit.toml                 # model declares origin = date("2020-02-24")
camdl pfilter model.camdl --data cases_dated.tsv ...
```

- **Numeric data is unchanged** — indexed day-numbers `0,1,2` (or fractional
  times) take the existing path and need no `origin`. Adding `origin` to a model
  never reinterprets a numeric column.
- **`--time-format numeric|date`** forces the interpretation (e.g. to reject a
  packed integer like `20200315` that would otherwise parse as a number); the
  default is `auto`.
- **`--time-col NAME`** selects the time column by name.
- A dated column with no `origin` in the model → clear error.
- **Off-grid times warn, they don't fail.** Converted times need not land on the
  integrator step `dt`; the solver snaps within `dt` (Gillespie is exact). The
  one hard error is two distinct observations mapping to the *same* step (obs
  spacing `< dt`) — a real data/`dt` mismatch. Under `'months`/`'years`, calendar
  data never lands on integers (average-length months); this warns, and is fine
  on a continuous axis — use `'days` if you want integer-aligned monthly points.

## Getting dates back out

- **`camdl simulate --dates`** adds a calendar `date` column (the inverse map)
  alongside the canonical numeric `t` in trajectory and observation output
  (single-file and `--obs-dir`). Without `--dates`, output is byte-identical to
  before. Requires `origin`.
- **`camdl fit summary`** renders `instant`-kind estimands as dates when the
  model has an `origin` (e.g. `tau = 23.0  (2020-02-13)`); `duration` estimands
  render as spans. Numeric `t` stays the canonical, diff-stable value.

## International / multi-source data

A bare ISO date is a **civil-calendar label**, not an absolute instant — its day
number is computed straight from `(Y, M, D)`, with no timezone. So:

- A trailing zone designator (`Z`, `+06:00`, `-03:00`, `+05:45`) is **discarded**:
  `2020-03-15+06:00` and `2020-03-15-03:00` both map to the same `t`. Pooling
  surveillance from many countries aligns correctly by civil date onto one axis.
- This is *timezone-independent*, not "assume UTC": two modelers in any two zones
  who write `2020-03-15` get the same internal time. (Whether two locations'
  outbreaks *started* at the same time is a modeling question — per-location seed
  times `τ_i` — not a calendar one.)
- camdl trusts the civil date as written; if an upstream export mislabeled a
  late-night local event to the wrong civil day, that is an upstream concern no
  date policy can recover.

## Not supported (yet)

- **Times of day** (`2020-03-15T13:30`) in dates/data, and **sub-day
  `time_unit`s** (`'hours`/`'minutes`/`'seconds`). A datetime form is rejected
  with a clear error. Fast/hot epidemics don't need these — they need a smaller
  integrator step `dt` (in `'days`), which is independent of the time unit.

## Reference

- **Conversion / parsing:** `rust/crates/ir/src/caltime.rs` (Rust runtime),
  `ocaml/lib/compiler/expander.ml` `days_of_date`/`parse_date_to_float` (compile
  time). The two are pinned to agree.
- **IR fields:** `Model.origin` (the ISO string) and `Model.origin_rata_die`
  (the compiler-derived integer day number the runtime reads). IR schema ≥ 0.6.
- **`time_unit` vs `dt` vs cadence:** `time_unit` is the axis unit; `dt` is the
  integrator step (set by the dynamics, not the data); observation cadence is a
  property of the data. See `docs/camdl-run-spec.md`.
- **Design rationale and test plan:**
  `docs/dev/proposals/2026-05-22-calendar-time.md`.
