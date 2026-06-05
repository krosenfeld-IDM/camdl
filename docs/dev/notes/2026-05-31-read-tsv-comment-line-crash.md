# `read()` TSV with a `#` comment line crashes with `List.combine`

Date: 2026-05-31 Project: camdl Tags: expander, read, table, diagnostics,
error-quality

## Context

Found while building the `sia_anchored_dates` golden for instant-typed table
cells (`docs/dev/proposals/2026-05-31-instant-typed-table-cells.md`). The SIA
schedule TSV originally carried two provenance comment lines (the CLAUDE.md
convention: reference data should document its source). The compiler crashed.

## Reproduction

A `tables { t : dim × dim : kind = read("f.tsv") }` whose `f.tsv` begins with a
`#` comment line:

```
# SIA campaign schedule: (region, round) -> ISO date
# resolved via origin
region	round	sia_time
north	r0	2013-11-01
...
```

```
$ camdlc model.camdl
Error: Invalid_argument("List.combine")
```

Removing the comment lines (header row first) compiles cleanly. Verified by
compiling the exact same model against a comment-free vs commented copy of the
TSV: comment-free → EXIT 0; commented → the crash.

## Root cause (confirmed against `ocaml/lib/compiler/expander.ml`)

`read_csv_rows` reads the **first physical line** as the header unconditionally
(`expander.ml:268`, `let header_line = input_line ic`). The `#`-skip lives
_inside the data loop only_ (`expander.ml:276`, `line.[0] = '#'`), so it never
protects the header. A leading `#` comment therefore becomes `header_cols`.

In `load_table_data`'s `on_header`, `header_dims` is truncated to
`min n_dims (List.length header_cols)` (`expander.ml:328-330`), and the mismatch
branch then zips it against the full `dim_names`:

```ocaml
(* expander.ml:355 *)
) (List.combine dim_names header_dims)
```

When the comment splits to fewer columns than `n_dims` (a 1-column, tab-free
comment vs a 2-dim `region × round` table → 1 vs 2), `List.combine` raises
`Invalid_argument`. Confirmed with the reproduction above and a column-count
variant: a `#`-comment that splits to _exactly_ `n_dims` columns (e.g.
`#region\t#round`) does **not** crash — instead the real header row is consumed
as a data row and surfaces as `E207` ("'region' in column 1 ... is not a valid
'region' level"). So the symptom is column-count-dependent, exactly as the
truncation predicts.

The same `read_csv_rows` header read backs all three `read()` consumers
(`load_table_data` :298, `read_dim_column_from_file` :560,
`load_interpolated_for_level` :3197); a fix in `read_csv_rows` covers all of
them.

## Why it matters

Two failures in one:

1. **No comment support in `read()` files.** Reference data should be able to
   carry provenance (source URL, fetch date) as `#` lines, per the data-step
   convention. Today it cannot.
2. **Opaque exception instead of a diagnostic.**
   `Invalid_argument(
   "List.combine")` violates "never use
   `failwith`/`assert` for user-facing errors" — it gives a stack-trace-class
   message with no file, line, or fix hint.

## Fix options

- (a) Skip `#`-prefixed lines in `read_csv_rows` before the header scan.
  Smallest fix; restores provenance-comment support and matches the convention.
- (b) Length-check header-columns vs dim-names and emit a clean E-code (e.g.
  E216 family) with the path, the expected vs actual column count, and a hint.
  Should be done regardless of (a) — any malformed header (not just comments)
  should diagnose, not crash.

Recommend both: (a) for the feature, (b) for the error-quality guarantee. Small,
well-scoped — a good `gh` issue.

## Next

Filed as **gh#144** with this reproduction and the confirmed root cause. Out of
scope for the instant-table-cells change (that change worked around it by
shipping a comment-free golden TSV).
