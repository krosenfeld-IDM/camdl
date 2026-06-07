# Compiler error-handling review: `try`/`failwith` vs `Result`

Date: 2026-06-07
Scope: `ocaml/lib/compiler/`, `ocaml/lib/ir/` — exception use vs structured
results in the front end.
Status: reading notes + ranked recommendations. No code changed.

## TL;DR

The premise ("lots of `try`, should use `Result` more") is half-true, and the
true half is **already specced**:

- The *pipeline-level* surface — "accumulate structured diagnostics, don't
  throw" — is the subject of an open proposal,
  [`docs/dev/proposals/2026-06-05-compiler-diagnostic-surface.md`](../proposals/2026-06-05-compiler-diagnostic-surface.md)
  (gh#181, `OPEN`, **unimplemented** — verified: `grep -rn "outcome" ocaml/lib/compiler/` → no
  matches). That proposal is the right home for the `result`-everywhere work;
  this review should not duplicate it.
- What gh#181 does **not** touch is *leaf-level* exception hygiene inside the
  expander and IR reader — the actual `try`/`failwith`/`assert false` sites.
  That is the residual, and it is small.

Raw counts (`grep`, excluding `_build`):

```
$ grep -rn '\btry\b' ocaml/lib ocaml/bin | grep -v _build | wc -l   → 23
   (by file: expander 14, compiler 4, lexer 3, serde 1, dimcheck 1)
$ for p in failwith 'assert false' '\braise\b' invalid_arg; do ...; done
   failwith 7   assert false 1   raise 8   invalid_arg 3
```

But ~half the `try` hits are false positives — function names (`try_eval_const_int`,
`try_mul`) and record fields (`init_entries`, `dim_registry`). The genuine
`try … with` blocks number ~10, and most are **idiomatic and correct** (see
"What is already fine"). The whole residual reduces to **one** recurring real
problem (date parsing raises `failwith` and loses source location) plus three
small consistency nits.

## What is already good (do not touch)

The error architecture is more mature than the `try` count suggests. The
2026-04-19 compiler review already did a pass here.

- **`Diagnostics` module is a clean ADT** (`diagnostics.ml:13–31`): three-level
  `severity = Error | Warning | Info`, structured `diagnostic` with
  `code / loc / message / detail / hint / related`. JSON and ANSI render off the
  same record. This is the currency gh#181 wants to thread end-to-end.
- **Library does not call `exit`** on the compile path. `report_and_exit`
  *raises* `Compile_error` (`diagnostics.ml:240–242`), and the CLI top level
  turns that into `exit 1`. Tests/embedders can catch it.
- **The pipeline is already `Result`-typed at the boundaries.**
  `compile : … -> (Ir.model, string) result` and
  `compile_detail_result : … -> (compile_detail, string) result`
  (`compiler.ml:122, 320`). Parse/expand failures are caught and converted to
  E001 diagnostics rather than propagating as raw exceptions
  (`compiler.ml:46–71, 79–89`).
- **`failwith` in parser semantic actions was already retired** into
  `parser_errors.ml` (n3 in the 2026-04-19 review): Menhir actions can't thread
  a `Diagnostics.t`, so they push `(sp, ep, code, msg, hint)` and `compiler.ml`
  drains them into `ctx.diags` with real source locations
  (`parser_errors.ml:1–22`, drained at `compiler.ml:103–108`).
- **Idiomatic `try` that is NOT an offender — flagging so we don't generate
  noise:**
  - `try … with End_of_file -> ()` for line reading (`expander.ml:287–298`),
    wrapped in `Fun.protect` so the fd closes on every exit path
    (`expander.ml:266`). This is the canonical OCaml read-loop; leave it.
  - `try Hashtbl.find … with Not_found -> <default>`
    (`expander.ml:2460, 4240`; `lexer.mll:27–29`). Lookup-with-default. Fine.
    (`Hashtbl.find_opt` is marginally more modern but the behavior is identical;
    not worth a diff on its own.)
  - **`serde.ml`'s exception-internally / `Result`-at-the-boundary pattern is
    the right idiom**, not a smell: `DeserError` is raised deep in the reader
    (`serde.ml:43–45`) and caught once at the public edge —
    `serde.ml:1084: try Ok (model_of_json mj) with DeserError msg -> Error msg`.
    Threading a `result` through every field accessor would be all cost, no
    benefit. Keep it.

## The one real problem: date parsing raises `failwith`, drops location

`parse_iso_date` / `parse_date_to_float` are the only user-facing parsers that
still signal errors by raising (`expander.ml:123–147`):

```ocaml
let parse_iso_date s =
  match String.split_on_char '-' s with
  | [ys; ms; ds] ->
    (try (int_of_string ys, int_of_string ms, int_of_string ds)
     with _ -> failwith (Printf.sprintf "invalid date literal '%s': ..." s))   (* expander.ml:126–127 *)
  | _ -> failwith (Printf.sprintf "date literal must be YYYY-MM-DD, got '%s'" s)
```

Two distinct defects, both forbidden by CLAUDE.md ("Never use `failwith` … for
user-facing errors"):

1. **`failwith` for a user error.** A malformed date literal in a model is
   user-facing input. The `failwith` escapes to the expand-level catch-all
   (`compiler.ml:82–84: with Failure msg -> Diagnostics.error … ~loc:Diagnostics.no_loc`),
   so the user gets an E001 **with no source location** — `no_loc` renders the
   bare message with no `┌─ file:line:col` block. For a DSL whose error quality
   is a stated first-class goal, "invalid date literal" pointing at nothing is a
   regression from every other diagnostic in the compiler.
2. **Catch-all `with _ ->`** (line 126) swallows *every* exception from
   `int_of_string`, not just the expected `Failure "int_of_string"`. Harmless
   here (the body only calls `int_of_string`), but it is the pattern the next
   reader copies.

Several call sites already *want* a `Result` and wrap the raise to recover it —
e.g. `try Ok (parse_iso_date s) with … -> …` (`expander.ml:1125, 1130`) and
`try Ir.Const (parse_date_to_float …) with …` (`451, 1330, 1807, 1876`). So the
clean fix is to push the `Result` down to the leaf:

```ocaml
val parse_iso_date : string -> (int * int * int, string) result
```

and at the genuine top-level call sites emit a real `Diagnostics.error` with the
date literal's `loc` (the literal already carries a span — `Ast.loc` flows from
`parser_errors.ast_loc_of`). Suggested new code in the E2xx range (parse/
expansion phase), e.g. **E212 "invalid date literal"**, with a hint showing the
expected `YYYY-MM-DD` shape. Classification: **doc-allows / code-offends** — the
spec already says dates are `YYYY-MM-DD`; the code just reports violations badly.

TDD shape (per CLAUDE.md red→green): a fixture with `origin: 2020-13-99` (or
`origin: "not-a-date"`) → assert the compile produces **E212 at the literal's
line**, not E001 at `no_loc`. Confirm it currently yields `no_loc`/E001 (red),
then fix (green).

## Three small consistency nits (each a one-liner)

1. **`ir.ml:300` uses `failwith` where the IR reader uses `DeserError`.**
   `hierarchical_kind_of_string` raises `failwith "unknown hierarchical kind '%s'"`
   on a bad IR field. But the deser boundary only catches `DeserError`
   (`serde.ml:1084`), so a malformed `kind` in IR JSON propagates as an
   **uncaught `Failure`** instead of arriving as `Error msg`. Classification:
   **code-vs-code** — fix by raising `Serde.fail` / `DeserError` here so the
   boundary catches it. Add a deser round-trip test with a bogus `kind`
   asserting `Error _`, not a crash.

2. **`assert false` at `expander.ml:3776`** is guarded by `all_const`
   (`3770–3777`): the list was just filtered to `Const` only, so the `_` arm is
   unreachable. Defensible as an invariant, but it trips the CLAUDE.md
   "never `assert false`" rule for no benefit — a `List.filter_map` over the
   already-resolved list erases the partial match entirely. Cosmetic; bundle it
   with adjacent work, don't make a commit of its own.

3. **`invalid_arg` "unreachable" guards** (`expander.ml:145, 169, 725`) are
   internal invariants with comments asserting the AST is validated upstream.
   Like the `assert false`, they would surface as an E001/`no_loc` if ever hit.
   Leave them — but if the date-parser refactor (above) is done, fold
   `parse_date_to_float`'s `invalid_arg` (145) into the same `Result` so the
   whole date path is exception-free.

   (Note: `inspect.ml` *does* call `exit 1` directly — `544, 1052–1114`. That is
   the `camdlc check`/`inspect` *command* surface living in `lib/`, not the
   compile core; `exit` there is a CLI concern misfiled into the library. Pure
   layering nit, lower priority than anything above.)

## Recommendation / sequencing

1. **Don't reopen the pipeline question — it's gh#181.** When that proposal is
   implemented (passes return `diagnostic list`; one `outcome` type; `compile`
   stops raising `Compile_error`), the boundary catch-alls in `compiler.ml`
   become the natural single place exceptions are tamed. Until then, leave the
   pipeline as-is.
2. **Fix the date parser** (the one real user-facing defect): `Result`-typed
   leaf + E212-with-location. Small, self-contained, TDD-able, high error-quality
   payoff. This is the piece of "use `Result` more" that actually pays.
3. **Land the two consistency nits** (`ir.ml` `DeserError`, the `filter_map`)
   opportunistically.
4. **Explicitly do not** rewrite the `End_of_file`, `Not_found`-default, or
   `serde` `DeserError` idioms into `Result`. They are correct OCaml; converting
   them is churn that trades a clear idiom for plumbing.

Net: the headline "we have lots of `try`, switch to `Result`" resolves to *one*
worthwhile leaf fix plus an already-written proposal for the structural part.
The codebase is in better shape on this axis than the raw `try` count implies.
