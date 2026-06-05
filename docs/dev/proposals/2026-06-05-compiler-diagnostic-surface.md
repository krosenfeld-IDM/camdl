---
date: 2026-06-05
status: proposal
related: ../../ocaml/lib/compiler/compiler.ml, ../../ocaml/lib/compiler/diagnostics.ml
issue: gh#181
supersedes-partially: the gh#170 front-end-unification (collect_detail), which this generalizes
---

# Compiler diagnostic/result surface: accumulate, don't throw

## Problem

The compiler expresses one notion — "did this source compile, and what is
wrong with it" — four incompatible ways, so no caller can handle errors
uniformly:

| entry point | type | how a failure escapes |
|---|---|---|
| `compile_detail_result` | `(compile_detail, string) result` | front-end **only**; `Error` = a *rendered string* |
| `run_validate` | `compile_detail -> bool` | diagnostics side-effected into a `mutable ctx` |
| `run_dimcheck` / `run_lint` | `compile_detail -> unit` | pure side-effect into `ctx` |
| `compile` | `(Ir.model, string) result` | **raises** `Compile_error of string` on validate/dimcheck (via `report_and_exit`) |

Consequences, each verified in `compiler.ml` / `diagnostics.ml`:

1. **The `result` type lies for late-phase errors.** `compile`'s signature
   promises errors as `Error` values, but validate/dimcheck failures
   *raise* `Compile_error` (`report_and_exit`, diagnostics.ml:242, raises —
   it does not `exit`). A caller writing `match compile src with Ok … | Error …`
   hits an uncaught exception on, e.g., E507.
2. **Structure is flattened at every boundary.** Both the exception and the
   `result` error carry a pre-rendered *string*, discarding the structured
   `diagnostic list` (severity/code/loc/hint). A library caller cannot filter
   by code or re-render.
3. **Passes report output by mutation, not by type.** `run_dimcheck : … -> unit`
   *produces* diagnostics but its type doesn't say so — output is smuggled
   through `ctx.diags` (a `mutable diagnostic list`, cons-prepended, hence the
   recurring "reverse to source order" step).
4. **Two entry points, same type, different pipelines.** `compile_detail_result`
   (front-end only) and `compile` (full) both return `result`; the type can't
   tell you one validated and one didn't, so a caller picks the short one and
   silently skips validation. This is the gh#170 root (`check` used the short
   path) and the gh#160 symptom (`check` returned a model `simulate` rejected).

These are the same class CLAUDE.md names: stringly/flag-riddled data where an
ADT belongs, and illegal states (an unvalidated model used as if valid) left
representable.

## Sound types to keep

- `severity = Error | Warning | Info` — a clean ADT.
- `diagnostic = { severity; code; loc; message; detail; hint; related }` — the
  real currency.
- `collect_detail` (gh#170) already runs the full pipeline and returns
  diagnostics as values without raising — this is the right shape; the work is
  to make it *the* surface, not a parallel one.

## Target design

1. **Passes return their diagnostics.** `run_validate / run_dimcheck /
   run_lint : compile_detail -> diagnostic list`. No mutation, no `bool`/`unit`.
   The pipeline becomes a fold that accumulates and short-circuits on the first
   `Error`-severity result.

2. **One structured outcome type** for every caller:
   ```ocaml
   type 'a outcome = {
     value       : 'a option;          (* Some iff no Error-severity diag *)
     diagnostics : diagnostic list;    (* errors + warnings + infos, source order *)
     source      : Source_cache.t;     (* for rendering *)
   }
   val compile : ?name:string -> ?filename:string -> string -> Ir.model outcome
   ```
   Errors are *values*; nothing in the library raises. `value = None` exactly
   when `diagnostics` contains an `Error`. (This is `collect_detail`
   generalized + a clean projection.)

3. **`report_and_exit` leaves the library.** Rendering-and-exiting is a CLI
   concern. The CLI top-level (and only it) does
   `match compile src with { value = Some m; _ } -> … | { diagnostics; source; _ } -> render diagnostics source; exit 1`.
   If an exception is kept anywhere, it carries `diagnostic list`, not a string.

4. **Make `Diagnostics.t` immutable (or local).** Each pass returns a list;
   the fold concatenates. Removes the `mutable` + cons-reverse dance.

## Migration (incremental, each step green)

1. Add the `outcome` type + `compile` returning it, implemented over the
   existing `collect_detail` (no behavior change — pure addition).
2. Repoint internal callers (`run_check` already routes through
   `collect_detail`; `simulate`/`fit`/CLI compile) to the `outcome` API.
3. Change `run_validate/dimcheck/lint` to return `diagnostic list`; rewrite the
   pipeline as a fold; delete the mutable-accumulator reliance.
4. Delete `report_and_exit` from the library; move render+exit to the CLI
   top-level. Delete `compile_detail_result` and the string-typed `result`
   entry points once callers are migrated.
5. Each step gates on a clean `make test` (OCaml unit + golden + integration);
   the gh#181-flagged caller (a `result` consumer seeing E507 as an exception)
   gets a regression test asserting it now arrives as a value.

## Aspirational (separate, larger): phantom-typed validated model

Distinguish `Ir.model` (unvalidated, straight from the expander) from a
`Validated.t` that is *only* constructible by passing validation, and have
`simulate`/`fit` require `Validated.t`. Then an unvalidated model cannot reach
the runtime — the gh#160 class becomes a compile error by construction, not a
runtime E507. Bigger change (touches the OCaml↔Rust boundary and every
runtime entry); call it out, don't bundle it.

## Out of scope

The sibling type-design issues — #107 (`bool always_active` / `ParamKind`
enums), #101 (lineage ID newtypes), #98 (typed-time unification) — are the same
"ADTs over flags/strings" family but independent; this proposal is the
diagnostic/result surface only.
