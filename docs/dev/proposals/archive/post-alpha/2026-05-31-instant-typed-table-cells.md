# Instant-typed table cells: anchored calendar dates in `read()` tables

Date: 2026-05-31 Status: implemented (branch `instant-table-cells`) Area: DSL /
expander (table loading), dimcheck (no change), golden fixtures Related: gh#32
(table cell-kind annotations), `docs/dates.md`,
`docs/dev/proposals/2026-05-22-typed-time-and-dsl-ergonomics.md`

## Problem

SIA (Supplementary Immunization Activity) schedules — the common operational
input for measles/polio campaign modelling — arrive as **calendar dates**, one
or more campaign dates per region/round, in a data file. In a calendar-anchored
model (`origin = date(...)`) the author wants to point an intervention schedule
at those dates directly:

```camdl
tables {
  sia_time : region × round : instant = read("sia_schedule.tsv")
}
interventions {
  sia[r in region, k in round]
    : transfer(fraction = vacc_cov, from = S[r], to = V[r])
      at [ sia_time[r, k] ]
}
```

Before this change the table reader parsed value cells **float-only**
(`load_table_data`: `float_of_string_opt cell` → `Some f`, else **E209**
"expected a number"). An ISO-date cell (`2013-11-01`) errored. The only
workarounds were both bad:

1. Pre-convert dates → day-offsets in a preprocessing script. This is exactly
   the _hidden calendar arithmetic_ the DSL philosophy forbids: the `.tsv` ends
   up holding `714.0` instead of a readable `2013-11-01`, and the offset
   silently depends on the model's `origin`.
2. Hand-list `at [date("2013-11-01"), ...]` inline. Fine for two rounds,
   hopeless for 44 LGAs × several rounds, and you cannot read a schedule from a
   file that way.

The data loader (`--data`, fit side) already auto-detects an ISO-date time
column and converts it via `origin` + `time_unit` (`docs/dates.md`, "Loading
dated data"). The **table** reader was the one place that path was never wired
in.

## Design: `instant`, not `'instant`

The annotation is the **`instant` kind keyword**, not a unit literal:

- The table cell-annotation slot is `param_kind` in the grammar (`parser.mly`:
  `names : dims : kind = ...`), the bare-keyword family
  `rate | probability | positive | count | real | instant | duration`.
- `'`-prefixed tokens (`'days`, `'count`) are `UNIT_IDENT` — a different lexical
  class used on the _dimension_ side (`table_dim_entry`). There is no `'instant`
  token; it would be a parse error.
- `instant` is a _kind_ (semantics: an absolute time point, resolved via
  `origin`), not a _unit_ (an instant has no scale of its own — its scale is the
  model's `time_unit`). Cf. `'count` (unit, dimension P) vs `count` (kind) — the
  tick is precisely the unit-vs-kind discriminator.

The grammar (`names : dims : kind = ...`, gh#32) and the IR carrier
(`table.cell_kind : string option`) **already existed**. `dimcheck` already maps
`instant`/`duration` cell-kinds to dimension `[T]` (`param_dim_of_kind`). The
only missing piece was the reader's date branch.

Dates stay **day-granular** (`parse_iso_date` reads `YYYY-MM-DD`; `rata_die` is
a whole-day count). The continuous axis still supports sub-`dt` stepping; the
calendar layer is days, by design (`docs/dates.md` rejects times-of-day and
sub-day `time_unit`s).

## What changed

`ocaml/lib/compiler/expander.ml`, `load_table_data`:

- Added a `~cell_kind:(string option)` parameter, threaded from the one call
  site in `expand_tables` (where `cell_kind` is already computed).
- In the value-cell parse, when `float_of_string_opt` fails:
  - `cell_kind = Some ("instant" | "duration")` **and** `ctx.origin` set →
    resolve the cell via `parse_date_to_float origin cell
    time_unit` (the
    same function `date()` literals and `date_range` use). Resolution happens at
    **compile time**; the IR stores plain `f64` offsets, so the IR format is
    unchanged (no schema bump).
  - `Some ("instant" | "duration")` but **no** `origin` → **E209** with a hint
    that a top-level `origin = date(...)` is required (a date cannot be resolved
    without an anchor — never silently 0).
  - otherwise → the original E209 "expected a number" (a date in a rate/count
    column is still an error).

A bare numeric cell still takes the float path and is read as internal time
directly (`docs/dates.md`, "a bare number is already internal time"). No change
to dimcheck, the IR schema, Rust, or any existing golden's output.

## Verification

- `ocaml/golden/sia_anchored_dates.camdl` +
  `data/sia_anchored_dates_schedule.tsv`: anchored SIRV,
  `origin = date("2013-01-01")`, instant table keyed by `region × round`,
  intervention `at [sia_time[r,k]]`. Compiles; the six ISO dates resolve to the
  expected day-offsets in the committed IR (north r0 2013-11-01 → 304, r1 → 438,
  r2 → 627; south r0 2013-11-08 → 311, r1 → 445, r2 → 634).
- `make update-golden`: only the three new files appear; no other `*.ir.json`
  changes (the date branch is inert for numeric cells).
- OCaml unit tests (`test_compiler.ml`, `table_cell_type_annotation_gh32`
  group):
  - `instant cells: ISO dates resolve via origin` — asserts the resolved table
    source equals `[304.0; 438.0]` and `cell_kind = Some "instant"`.
  - `instant date cell without origin is E209` — negative control, proves the
    branch is gated on the anchor (non-vacuous).
- Golden round-trip registered as `test_golden "sia_anchored_dates"`.

## Follow-up (separate bug, filed)

While building the golden, found a pre-existing crash unrelated to this feature:
a `read()` TSV whose first line is a `#` comment crashes the compiler with an
uncaught `Invalid_argument("List.combine")` (the comment line is mis-parsed as
the header, so header-columns vs dim-names lengths differ). This is an
opaque-exception bug — it should either skip `#` lines or emit a clean
diagnostic. It bit the SIA golden (whose TSV originally carried provenance
comments); worked around by removing the comments. Filed separately; not in
scope here.
