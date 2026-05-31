# `read()` TSV with a `#` comment line crashes with `List.combine`

Date: 2026-05-31
Project: camdl
Tags: expander, read, table, diagnostics, error-quality

## Context

Found while building the `sia_anchored_dates` golden for instant-typed
table cells (`docs/dev/proposals/2026-05-31-instant-typed-table-cells.md`).
The SIA schedule TSV originally carried two provenance comment lines
(the CLAUDE.md convention: reference data should document its source).
The compiler crashed.

## Reproduction

A `tables { t : dim × dim : kind = read("f.tsv") }` whose `f.tsv` begins
with a `#` comment line:

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

Removing the comment lines (header row first) compiles cleanly. Verified
by compiling the exact same model against a comment-free vs commented
copy of the TSV: comment-free → EXIT 0; commented → the crash.

## Root cause (hypothesis, not yet code-confirmed)

`read_csv_rows` / `load_table_data` (`ocaml/lib/compiler/expander.ml`)
appears to treat the **first physical line** as the header. A `#`
comment line has no tabs, so it splits to a single column; the header
check then does a `List.combine` of header-columns against dim-names
whose lengths differ (1 vs n_dims), raising the uncaught
`Invalid_argument`.

(Not yet localized to the exact `List.combine` call — needs a read of
`read_csv_rows`'s header handling. The reproduction above is solid; the
mechanism is inferred from the column-count mismatch and the error
text.)

## Why it matters

Two failures in one:

1. **No comment support in `read()` files.** Reference data should be
   able to carry provenance (source URL, fetch date) as `#` lines, per
   the data-step convention. Today it cannot.
2. **Opaque exception instead of a diagnostic.** `Invalid_argument(
   "List.combine")` violates "never use `failwith`/`assert` for
   user-facing errors" — it gives a stack-trace-class message with no
   file, line, or fix hint.

## Fix options

- (a) Skip `#`-prefixed lines in `read_csv_rows` before the header
  scan. Smallest fix; restores provenance-comment support and matches
  the convention.
- (b) Length-check header-columns vs dim-names and emit a clean E-code
  (e.g. E216 family) with the path, the expected vs actual column
  count, and a hint. Should be done regardless of (a) — any malformed
  header (not just comments) should diagnose, not crash.

Recommend both: (a) for the feature, (b) for the error-quality
guarantee. Small, well-scoped — a good `gh` issue.

## Next

File a gh issue with this reproduction. Out of scope for the
instant-table-cells change (that change worked around it by shipping a
comment-free golden TSV).
