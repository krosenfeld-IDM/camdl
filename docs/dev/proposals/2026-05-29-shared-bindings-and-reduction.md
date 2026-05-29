---
status: draft
date: 2026-05-29
title: Shared per-coordinate bindings + a reduction node in the IR
author: internal
scope: ir/schema.json, ocaml/lib/ir (ir.ml, serde, autodiff.ml), ocaml/lib/compiler/expander.ml, rust/crates/ir/src/expr.rs, rust/crates/sim/src/{resolved_expr.rs, propensity.rs, compiled_model.rs, chain_binomial.rs, inference/pgas_grad.rs}
motivates: docs/dev/notes/2026-05-29-foi-scaling-bench.md (Fix B + Fix D)
relates:
  - docs/dev/proposals/2026-05-26-typed-indexed-reference-resolver.md (OCaml resolver; sibling, no overlap in IR nodes)
  - docs/dev/reviews/2026-05-26-upstream-rust-engine-review.md (Critical #1: parameterized forcing/tables frozen — same evaluation-boundary surface)
non-goals:
  - sparse spatial kernels / top-k neighbours (changes the model; separate)
  - GPU / SIMD particle batching (separate)
  - the serde-untagged load win (Fix E, landed 5f3de94)
---

# Shared per-coordinate bindings + a reduction node in the IR

## Required-reading checkpoint (receipts)

Verified against the current tree, not from memory:

- **`sum` lowering** — `ocaml/lib/compiler/expander.ml:1590-1601`: `ESum (v,d,body)`
  maps the body over each dimension level and `List.fold_left (fun acc t -> BinOp{op=Add;...})`
  — a left-nested `Add` chain, P-1 deep for P levels. No reduction node.
- **`let` inlining** — `expander.ml:1857-1922` (`resolve_ident_name`) and `1525-1540`
  (indexed lets): a `let`-bound name resolves by `resolve_expr ctx … lb.lbody` —
  the body is re-expanded *fresh at every use site*. A `let` used in P·A
  transitions is duplicated P·A times. No shared-subexpression node.
- **`Expr` type** — `ocaml/lib/ir/ir.ml:15-41` and `ir/schema.json:93` (`"expr"`):
  `Const | Param | Pop | PopSum | Time | Dt | BinOp | UnOp | Cond | TimeFunc |
  TableLookup | Projected | UncheckedDim`. No reduction, no binding reference.
- **Autodiff** — `ocaml/lib/ir/autodiff.ml:15-186` (`differentiate`), invoked at
  `compiler.ml:208-235` *after* `Expander.expand_detail`. It differentiates the
  fully-inlined per-transition rate, per parameter, with purely tree-structured
  recursion (no DAG/sharing). This is why `rate_grad` is the measured ~5× IR
  multiplier — every parameter re-differentiates the whole inlined tree.
- **Rust eval** — `rust/crates/sim/src/resolved_expr.rs`: `ResolvedExpr` mirrors
  `Expr` 1:1; `eval_resolved` walks each tree fresh, no cross-transition memo.
- **IR-as-contract** — a new `Expr` variant changes atomically: `ir.ml` (type) +
  the OCaml serde + `ir/schema.json` + `rust/crates/ir/src/expr.rs` + every Rust
  match site, then `ir/VERSION` bump + golden regen (per CLAUDE.md "Changing the
  IR schema").

## Problem

The Kano measles model is O(P²·A²) in IR size and O(P²·A) per chain-binomial step
(P=patches, A=ages), where it should be ~O(P·A + P²). Measured: 2.6 GB IR, 8.2 GB
RAM, 45 s; IR size scales O(P²) in patches; a step is 14.5× slower at P=32 from
the spatial term alone; and the inlined sum nests deep enough to trip serde's
recursion limit (**unparseable past ~50 patches**). Root cause: the force-of-
infection per-patch aggregates `N[l]`, `I_agg[l]` and the spatial sum
`Σ_q W[l,q]·I_agg[q]/N[q]` are flat-inlined into every `(l,a)` infection rate
(and re-differentiated into every parameter's gradient). See the scaling note.

## Design (in types)

Two additions to the expression IR, plus a model-level binding table.

### D — a reduction node

After stratification expansion `Pop` references are concrete names, so the
reduction operates over already-substituted terms (not a runtime loop over a
dimension). It is an **n-ary associative reduce**:

```ocaml
(* ocaml/lib/ir/ir.ml *)
| Reduce of { op : reduce_op; terms : expr list }   (* op = Sum | Prod *)
```
```rust
// rust/crates/ir/src/expr.rs
Reduce { op: ReduceOp, terms: Vec<Expr> }            // ReduceOp::{Sum,Prod}
```
JSON: `{"reduce": {"op": "sum", "terms": [ <expr>, … ]}}`.

`expander.ml`'s `ESum` emits `Reduce{Sum, terms}` instead of the `Add` fold.
**What D buys:** kills the parse cliff (depth 1, not P), and a constant-factor IR
shrink (one node + a flat `Vec`, vs P-1 `BinOp` objects). It does **not** change
the asymptotic size — there are still P terms. D is *foundational for B*: B's
binding bodies are themselves sums and must use `Reduce` to stay shallow.

Gradient (autodiff): `d/dp Reduce{Sum, t_i} = Reduce{Sum, d/dp t_i}` — linear,
trivial. `Prod` via the standard product-rule expansion (or defer `Prod` to a
later increment; `sum` is all the FOI needs).

### B — shared per-coordinate bindings

Preserve `let N[l]`, `let I_agg[l]`, and (newly extracted) the spatial force
`F[l]` as **named, computed-once-per-step** values, instead of inlining. Add a
model-level, topologically-ordered binding list and an `Expr` that references a
binding by index:

```ocaml
(* ocaml/lib/ir/ir.ml — new model field *)
bindings : binding list             (* topologically ordered; no forward refs *)
and binding = { bname : string; bexpr : expr }   (* bexpr may use BindingRef of earlier bindings *)
(* new expr variant *)
| BindingRef of int                 (* index into model.bindings *)
```
```rust
// rust/crates/ir/src/expr.rs
BindingRef(usize)
// rust/crates/ir/src/model.rs
pub bindings: Vec<Binding>,          // pub struct Binding { pub name: String, pub expr: Expr }
```

`expander.ml` stops inlining a `let X[i in dim]`: it emits one `binding` per
concrete coordinate (`N_kano_dala`, …) whose `bexpr` is `Reduce{Sum, …}`, and
rewrites use-sites to `BindingRef(idx)`. Bindings are emitted in dependency order
(`I_agg`, `N` before `F`; `F` before the rates that read it).

#### Evaluation model

`CompiledModel` resolves `BindingRef` to a slot index and stores the bindings as
`ResolvedExpr`s in topo order. `step_one` gains a **preamble**: before evaluating
any transition rate, evaluate every binding's `ResolvedExpr` into a
`scratch.bindings: Vec<f64>` slot array (in order; a binding may read earlier
slots via `ResolvedExpr::BindingRef(slot)`). Rate eval then reads slots in O(1).

```rust
// resolved_expr.rs
ResolvedExpr::BindingRef(slot)        // eval: ctx.bindings[*slot]
ResolvedExpr::Reduce { op, terms }    // eval: terms.iter().map(eval).{sum,product}()
```

Per-step cost: O(P·A·C) for the `N`/`I_agg` bindings + O(P²) for `F` (the W
matrix is irreducibly P², short of sparsity — a non-goal here) + O(P·A) for the
now-O(1) rates. IR size: O(P² + P·A). Both asymptotically below today's O(P²·A·…).

#### Gradients (the hard part)

The architecture is "OCaml emits derivative expressions, Rust evaluates them; no
runtime autodiff" (CLAUDE.md). Keep that, but make gradients share the binding
DAG. **Recommended (option 1):** autodiff emits, per binding `b` and estimated
parameter `p`, a derivative binding `db/dp` (an `Expr` that may reference other
bindings' value-slots *and* grad-slots via `BindingRef`/a grad-slot ref). A
rate's `rate_grad[p]` then references grad-slots instead of re-differentiating
the inlined sum. Runtime evaluates value-slots and grad-slots in the same topo
preamble. Chain rule through a binding ref: `d/dp BindingRef(b) = GradRef(b,p)`.
This shrinks `rate_grad` from O(P²·A²·#params) to O((P²+P·A)·#params), shared
across all transitions.

*Alternative (option 2):* runtime forward-mode dual-number AD over the
binding DAG + rate tree — eliminates emitted gradients entirely (subsumes Fix A's
`rate_grad` bloat) but changes the inference gradient path and so carries more
risk. The Rust side already has `eval_resolved_deriv` (forward-mode AD on
resolved trees), so option 2 is feasible; deferring it keeps this proposal's risk
bounded. **Decision for review:** option 1 first; option 2 as a follow-up if the
emitted grad-binding bookkeeping proves heavier than dual-number eval.

## Why this is correct / safe

- **Determinism (paired-seed CRN).** Bindings are deterministic functions of
  state/params/t evaluated *before* any RNG draw in `step_one`; they do not
  consume the RNG and do not reorder transitions or draws. Forward trajectories
  must be **byte-identical** before/after — this is the primary correctness gate
  (golden trajectory invariance test), and the way the refactor proves it didn't
  change the model.
- **Engine review Critical #1 interaction (helps, doesn't fight).** #1 is that
  parameter-dependent forcing/tables are frozen as `f64` at construction.
  Bindings are the *opposite* pattern — `ResolvedExpr` re-evaluated each step with
  the current `params` — so B reinforces #1's fix ("any value depending on
  params/state/t stays an expression"). The two should share the
  `structure / evaluation-context` split #1's structural section recommends.
- **Numerics.** Reduce associativity: `Reduce{Sum}` must fold left-to-right to
  match today's `fold_left` Add order bit-for-bit (float addition is not
  associative); the golden trajectory test enforces this.

## Phasing

| phase | change | win | risk | measured by |
| --- | --- | --- | --- | --- |
| **D** | `Reduce` node; `ESum` emits it; eval + autodiff + golden regen | parse cliff gone (P>50 parses); constant-factor IR/RAM | low (linear grad; trajectory-invariant) | scaling sweep (cliff, IR bytes), `load_parse_compile` |
| **B1** | bindings + `BindingRef` + step preamble (values only); rates O(1); **gradients still inlined** | asymptotic IR + per-step eval; RAM | medium (eval-boundary; trajectory-invariance gate) | `eval_propensities`/`step_one` slope flips to O(P·A+P²); IR O(P²+PA) |
| **B2** | emitted shared binding-gradients; `rate_grad` references grad-slots | asymptotic gradient IR + PGAS eval | high (inference math) | gradient round-trip vs finite-diff; PGAS golden |

Each phase is independently shippable and gives a before/after bar series in
`deser_load_before_after.tsv` (migrate it to long format `model,stage,load_us`
when B1 lands, so the chart stacks `untagged → +E → +D → +B1 → +B2`).

## Test plan

- **Trajectory invariance (the gate):** every golden model simulated with a fixed
  seed produces a byte-identical `Trajectory` before and after each phase. New
  test asserting this over `ir/golden/*` on chain-binomial + tau-leap.
- **Reduce associativity:** unit test that `Reduce{Sum,[a,b,c]}` equals
  `((a+b)+c)` bit-for-bit (the old fold order).
- **Gradient correctness (B2):** emitted shared grads match a finite-difference
  check on a small spatial model, and match the pre-B2 inlined grads.
- **Scaling:** the existing `make bench-scaling` / `bench-micro` show the cliff
  removed (D), the IR-size and per-step slopes drop (B1), and grad IR drops (B2).
- **IR round-trip:** golden_deser + new `Reduce`/`BindingRef` round-trip + the
  serde manual-Deserialize arm (Fix E) updated for the new variants.

## Risks & open questions

1. **Gradient option 1 vs 2** (emitted shared grads vs runtime dual numbers) —
   pick at review; option 1 first.
2. **`bindings` placement in the IR** — model-level list vs per-transition. A
   single model-level topo-ordered list is simplest and matches the
   compute-once-per-step model; confirm no per-transition binding scoping is
   needed (the FOI bindings are global per step).
3. **Balance/events/observations** also reference state; they should read the
   same binding slots (e.g. `N[l]` in `births`/observations) — confirm all
   `Expr` sites route through the preamble.
4. **`UncheckedDim`, `Cond`** inside binding bodies — autodiff already handles
   them tree-wise; ensure the binding-grad emission composes.
5. Does any consumer assume `Expr` is fully self-contained (no external slot
   refs)? Audit lineage/diagnostics expr walkers before B1.
