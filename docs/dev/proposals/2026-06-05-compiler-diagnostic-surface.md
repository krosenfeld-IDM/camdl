---
date: 2026-06-05
status: proposal
related: ../../ocaml/lib/compiler/compiler.ml, ../../ocaml/lib/compiler/diagnostics.ml
issue: gh#181
supersedes-partially: the gh#170 front-end-unification (collect_detail), which this generalizes
---

# Compiler diagnostic/result surface: accumulate, don't throw

## Problem

The compiler expresses one notion — "did this source compile, and what is wrong
with it" — four incompatible ways, so no caller can handle errors uniformly:

| entry point                 | type                              | how a failure escapes                                                             |
| --------------------------- | --------------------------------- | --------------------------------------------------------------------------------- |
| `compile_detail_result`     | `(compile_detail, string) result` | front-end **only**; `Error` = a _rendered string_                                 |
| `run_validate`              | `compile_detail -> bool`          | diagnostics side-effected into a `mutable ctx`                                    |
| `run_dimcheck` / `run_lint` | `compile_detail -> unit`          | pure side-effect into `ctx`                                                       |
| `compile`                   | `(Ir.model, string) result`       | **raises** `Compile_error of string` on validate/dimcheck (via `report_and_exit`) |

Consequences, each verified in `compiler.ml` / `diagnostics.ml`:

1. **The `result` type lies for late-phase errors.** `compile`'s signature
   promises errors as `Error` values, but validate/dimcheck failures _raise_
   `Compile_error` (`report_and_exit`, diagnostics.ml:242, raises — it does not
   `exit`). A caller writing `match compile src with Ok … | Error …` hits an
   uncaught exception on, e.g., E507.
2. **Structure is flattened at every boundary.** Both the exception and the
   `result` error carry a pre-rendered _string_, discarding the structured
   `diagnostic list` (severity/code/loc/hint). A library caller cannot filter by
   code or re-render.
3. **Passes report output by mutation, not by type.** `run_dimcheck : … -> unit`
   _produces_ diagnostics but its type doesn't say so — output is smuggled
   through `ctx.diags` (a `mutable diagnostic list`, cons-prepended, hence the
   recurring "reverse to source order" step).
4. **Two entry points, same type, different pipelines.** `compile_detail_result`
   (front-end only) and `compile` (full) both return `result`; the type can't
   tell you one validated and one didn't, so a caller picks the short one and
   silently skips validation. This is the gh#170 root (`check` used the short
   path) and the gh#160 symptom (`check` returned a model `simulate` rejected).
5. **Location is discarded at 85% of emit sites.** 111 of 131
   `Diagnostics.{error,warning,info}` calls pass `Diagnostics.no_loc`
   (`grep -rc '~loc:Diagnostics.no_loc' ocaml/lib` over a total of 131 emit
   sites). Some of that is honest — post-expansion structural errors (E5xx,
   "duplicate compartment after stratification") have no single source span
   because stratification synthesized the clash from two origins. Much is not:
   the front-end date-literal parser `failwith`s into an E001 at `no_loc`, and
   `run_dimcheck` / `run_validate` / `run_lint` re-emit every downstream
   diagnostic at `no_loc` even where the AST carries a span. The `loc` type is
   rich; the plumbing throws it away. The surface refactor is the moment to
   thread real spans through the pass-return values, so this should land with
   it rather than as a separate sweep.

These are the same class CLAUDE.md names: stringly/flag-riddled data where an
ADT belongs, and illegal states (an unvalidated model used as if valid) left
representable.

## Sound types to keep

- `severity = Error | Warning | Info` — a clean ADT.
- `diagnostic = { severity; code; loc; message; detail; hint; related }` — the
  real currency.
- `collect_detail` (gh#170) already runs the full pipeline and returns
  diagnostics as values without raising — this is the right shape; the work is
  to make it _the_ surface, not a parallel one.

## Target design

1. **Passes return their diagnostics.**
   `run_validate / run_dimcheck /
   run_lint : compile_detail -> diagnostic list`.
   No mutation, no `bool`/`unit`. The pipeline becomes a fold that accumulates
   and short-circuits on the first `Error`-severity result.

2. **One structured outcome type** for every caller:
   ```ocaml
   type 'a outcome = {
     value       : 'a option;          (* Some iff no Error-severity diag *)
     diagnostics : diagnostic list;    (* errors + warnings + infos, source order *)
     source      : Source_cache.t;     (* for rendering *)
   }
   val compile : ?name:string -> ?filename:string -> string -> Ir.model outcome
   ```
   Errors are _values_; nothing in the library raises. `value = None` exactly
   when `diagnostics` contains an `Error`. (This is `collect_detail`
   generalized + a clean projection.)

3. **`report_and_exit` leaves the library.** Rendering-and-exiting is a CLI
   concern. The CLI top-level (and only it) does
   `match compile src with { value = Some m; _ } -> … | { diagnostics; source; _ } -> render diagnostics source; exit 1`.
   If an exception is kept anywhere, it carries `diagnostic list`, not a string.

4. **Make `Diagnostics.t` immutable (or local).** Each pass returns a list; the
   fold concatenates. Removes the `mutable` + cons-reverse dance.

## Design note: accumulate (applicative) vs sequence (monad)

`outcome` is not a `Result` and not a short-circuiting error monad. Structurally
it is `(value : 'a option, diagnostics : diagnostic list)` — i.e.
`MaybeT (Writer (diagnostic list))`: a Writer effect that accumulates the
diagnostic log monoidally, over a Maybe effect that carries the value. That
combination *is* a lawful monad — unlike `Validation` / `Either`-with-
accumulation, which is applicative-only (its `bind` needs the success value to
choose the next step, so it cannot run a failed step's successor to collect more
errors; accumulation is inherently the applicative `<*>`, per McBride & Paterson,
*Applicative Programming with Effects*, JFP 2008). What buys the monad back is
that errors accumulate in a *separate channel* (the Writer log) from
success/failure (the Maybe) — and the same split is what lets `outcome`
represent "compiled successfully **with** warnings," which an `Either` cannot.

Two combinators, two jobs:

- **Sequential, dependent phases → monadic `let*` (bind).** expand → dimcheck →
  autodiff: if expand structurally fails there is no model to dimcheck, so
  short-circuit the *value* while retaining the log. This is the pipeline fold.
- **Independent sibling checks → applicative `let+ … and+ …` / traverse.**
  Within dimcheck, N transitions each produce their own diagnostics; run all,
  concat the lists. Do not `bind` siblings — bind short-circuits at the first
  bad one and hides the rest.

In OCaml (4.08+ binding operators) that is a ~15-line module:

```ocaml
module Outcome : sig
  type 'a t = { value : 'a option; diags : Diagnostics.diagnostic list }
  val return  : 'a -> 'a t
  val ( let* ) : 'a t -> ('a -> 'b t) -> 'b t   (* sequence: short-circuit value, keep log *)
  val ( let+ ) : 'a t -> ('a -> 'b) -> 'b t
  val ( and+ ) : 'a t -> 'b t -> ('a * 'b) t    (* accumulate: concat both logs *)
end
```

`( and+ )` (the applicative product) is where accumulation lives; `( let* )` is
where short-circuit lives. A pass is `traverse` over its siblings with `and+`;
the pipeline is `let*` over its phases.

Peer compilers split the same way by different means. Stan's compiler (stanc3 —
OCaml + Menhir, this project's stack) reports the *first* semantic error via an
internal exception (`exception TypecheckerException of Semantic_error.t`, caught
at the boundary and turned into a `Result.t`) while *accumulating warnings* in a
`Warnings.t list ref` (`src/frontend/Typechecker.ml`) — almost exactly camdl's
current `Compile_error` + `mutable diags`. rustc and GHC instead accumulate and
recover: rustc threads a side-effecting diagnostics context (`DiagCtxt`) and uses
`ErrorGuaranteed` as a type-level witness that an error was reported (the same
idea as the phantom-typed `Validated.t` below); GHC's typechecker monad (`TcRn`)
accumulates into an error bag and recovers. The `outcome` type puts camdl in the
accumulate camp **without** a global mutable sink — cleaner than the stanc3
baseline, not a remediation of something uniquely broken.

## Migration (incremental, each step green)

1. Add the `outcome` type + `compile` returning it, implemented over the
   existing `collect_detail` (no behavior change — pure addition).
2. Repoint internal callers (`run_check` already routes through
   `collect_detail`; `simulate`/`fit`/CLI compile) to the `outcome` API.
3. Change `run_validate/dimcheck/lint` to return `diagnostic list`; rewrite the
   pipeline as a fold; delete the mutable-accumulator reliance.
4. Delete `report_and_exit` from the library; move render+exit to the CLI
   top-level. Delete `compile_detail_result` and the string-typed `result` entry
   points once callers are migrated.
5. Each step gates on a clean `make test` (OCaml unit + golden + integration);
   the gh#181-flagged caller (a `result` consumer seeing E507 as an exception)
   gets a regression test asserting it now arrives as a value.

## Aspirational (separate, larger): phantom-typed validated model

Distinguish `Ir.model` (unvalidated, straight from the expander) from a
`Validated.t` that is _only_ constructible by passing validation, and have
`simulate`/`fit` require `Validated.t`. Then an unvalidated model cannot reach
the runtime — the gh#160 class becomes a compile error by construction, not a
runtime E507. Bigger change (touches the OCaml↔Rust boundary and every runtime
entry); call it out, don't bundle it.

## Out of scope

The sibling type-design issues — #107 (`bool always_active` / `ParamKind`
enums), #101 (lineage ID newtypes), #98 (typed-time unification) — are the same
"ADTs over flags/strings" family but independent; this proposal is the
diagnostic/result surface only.
