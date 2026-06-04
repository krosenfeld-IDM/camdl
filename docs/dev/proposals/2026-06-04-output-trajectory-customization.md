# Output trajectory customization: cadence, format, derived quantities

Status: **Phase 1 implemented** — `every = E` (regular cadence, sub-unit
allowed), `at = [...]` (explicit times), and `format` now parse and wire
through to the IR `output` schedule (`OutRegular` / `OutAtTimes`); `every`
and `at` are mutually exclusive. **Remaining:** `match_observations`
(deferred pending verification of its emission path — `output.rs` returns
`vec![]` for it) and Phase 2 (named derived `quantities`, a cross-language
IR schema change). The original two-phase analysis is preserved below.

## Problem

A model author wants finer-grained trajectory output than the default
(snapshots every `1` `time_unit`). Two distinct wants:

1. **Cadence + format.** "Sample every `0.1 'days`" (fast dynamics need
   sub-unit resolution), and "write `parquet` not `tsv`."
2. **Derived quantities.** Emit named columns computed at output time —
   `prevalence = I / N`, `total_I = sum over strata`, `incidence(...)` —
   so the analyst reads them directly instead of post-processing the raw
   compartment columns.

Both are things users genuinely want, and neither has a CLI equivalent
today (the CLI controls *where* output goes and *whether* observations are
written, not the cadence or the derived columns).

## Current state (verified)

The grammar accepts `output { trajectories { … } }` but the body is parsed
by `func_arg` = `IDENT EQ expr` (parser.mly:326). That rule cannot match
the documented fields:

- `every` and `format` are **lexer keyword tokens** `EVERY`/`FORMAT`
  (lexer.mll:104/107), not `IDENT`. `EVERY` is consumed only by
  observation/intervention/event rules; `FORMAT` is consumed by **no rule
  at all** — the source of the standing `Warning: the token FORMAT is
  unused`.
- `quantities { … }` is `IDENT LBRACE …`, not `IDENT EQ expr`.

So only an empty block parses, and the `List.assoc_opt "every"/"format"`
logic in `output_kv` is dead (those keys can never appear). Probe evidence:

```
output { trajectories { } }              => OK (no-op)
output { trajectories { foo = 1 } }      => OK (plain IDENT key, ignored)
output { trajectories { every = 1 } }    => E001 syntax error
output { trajectories { format = tsv } } => E001 syntax error
output { trajectories { quantities {…} } } => E001 syntax error
```

The decisive asymmetry — and why this is cheap to fix for cadence/format
but not for quantities — is what's **already wired downstream**:

- `expand_output` (expander.ml:3196-3206) **already reads** `ot.otevery`
  → builds the IR `output_schedule` step, and `ot.otformat` → the IR
  `format`. The Rust runtime already honors both. So cadence + format are
  ~90% built; only the grammar entry is missing.
- `ot.otquantities` is **read nowhere**, and IR `output_config`
  (ir.ml:350) has no quantities field — `{ times; format; trajectory;
  observations }`. So derived quantities are genuinely unbuilt end-to-end.

## Phase 1 — cadence + format (grammar-only, small)

Give the trajectories block its own field rule instead of `func_arg`:

```
traj_field:
  | EVERY  EQ e = expr   { `Every e }
  | FORMAT EQ f = IDENT  { `Format f }

trajectories_body:
  | fields = list(traj_field) { (* fold into otevery / otformat *) }
```

`EVERY EQ expr` is exactly how observation schedules already consume
`every = 7 'days` (parser.mly:477), so dimensioned literals like
`every = 0.1 'days` parse for free and resolve through the existing
`resolve_float_expr` path. Nothing changes in the IR, the schema, or Rust —
`otevery`/`otformat` are already consumed.

Scope: parser.mly (new rule + fold), delete the dead `List.assoc_opt`
logic, retire the now-unused `FORMAT`-warning by actually using it. Add a
golden fixture that sets `every` + `format` (today **no** committed
`.camdl` uses an `output {}` block, so this path is wholly untested — the
fixture is the point). Verify the IR schedule step reflects `every`, and a
sub-unit `every = 0.1` produces the expected row count.

This is the change that "unlocks finer-scale trajectories." Low cost, high
value, no cross-language surface.

## Phase 2 — derived quantities (IR schema change, larger)

`quantities { name = expr … }` is a real feature spanning both languages:

1. **Grammar.** A nested `quantities { IDENT EQ expr … }` block →
   `otquantities : (string * expr) list`.
2. **IR schema.** Add `quantities: [(string, expr)]` to `output_config`
   (ir.ml + rust/crates/ir + `ir/schema.json` bump + golden regen) — the
   atomic cross-language procedure in CLAUDE.md.
3. **Expander.** Wire `otquantities` through, dimension-checking each
   expr (they're output-time expressions over compartments/params/`t`,
   same surface as observation `DerivedExpr`).
4. **Runtime.** Evaluate each quantity at every output time and write it
   as a named column. The evaluator already exists (`eval_expr` over the
   trajectory state); this is plumbing it into the trajectory writer.
5. **Tests.** Golden fixture with `prevalence = I / N`; assert the column
   values against hand-computed `I/N` at a few times.

Open question for Phase 2: overlap with observation `DerivedExpr` and with
the (removed) summary surface — derived *trajectory* quantities (per output
time) and derived *summary* scalars (one per run) are different reductions;
keep them distinct.

## Recommendation

Ship **Phase 1 now** — it's a contained grammar fix that delivers the
stated goal (finer-scale trajectories) against already-wired plumbing, with
a golden fixture to lock it. Treat **Phase 2** as a separate IR-schema lift
when derived output columns are actually needed; it carries the full
cross-language cost and deserves its own review.

Until Phase 1 lands, §16 of the language spec documents only the default
schedule + CLI emission (the working reality) and does not show the
`output {}` customization block.
