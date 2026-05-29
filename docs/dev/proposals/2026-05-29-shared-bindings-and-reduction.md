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

## Adversarial review revisions (2026-05-29) — supersede the body below where they conflict

Two independent reviewers read the code. Verdict: direction sound, **D genuinely
low-risk and shippable**; **B1/B2 underscoped** with two silent-wrong-answer traps.
Changes:

### Scope guard & landing conditions (read first — applies to any implementer)

- **Implement D, then B1. DO NOT implement B2 (shared/emitted binding-gradients).**
  B2 is **out of scope for this proposal** and must not be built without a fresh
  decision. Reason: production gradients are the OCaml-emitted `rate_grad`, and
  `autodiff` maps `Pop`/`PopSum → 0` (state is conditioned-on in PGAS). **Every
  binding in the current FOI is state-only ⇒ `d(binding)/dp ≡ 0`**, so B2's
  machinery buys nothing on the gradient once B1 shrinks the rate trees. If a
  future model ever puts an estimated `Param` inside a binding body, *stop and
  re-open this decision* — and prefer the dual-number path (option 2, reuses the
  FD-validated `eval_resolved_deriv`) plus a new **spatial** FD gradient check
  (today's covers only non-spatial `sir_basic`).
- **Landing condition — all backends, byte-identical.** A phase may land only if
  every golden simulates to a **byte-identical `Trajectory`** (fixed seed) under
  **all four backends — chain-binomial, tau-leap, Gillespie, ODE** — before and
  after. The binding preamble + `EvalCtx.bindings` must be wired into all of them
  (and `intervention.rs`/`observe`), not just `chain_binomial::step_one`. A green
  chain-binomial run is *not* sufficient to land.
- **Compile gate.** All exhaustive `Expr`/`ResolvedExpr` match sites (the
  completeness checklist below) handle the new variants; the four catch-all guard
  sites consult per-binding flags. Both are blocking, not follow-ups.
- **Cliff gate (D).** A P≥64 spatial model that fails to parse today must parse
  after D.

### Test/golden coverage to build BEFORE B1 (current goldens have blind spots)

The existing spatial goldens are *associativity-blind* (`sir_spatial_sum`'s
`N = S+I+R` is 3 terms — every fold order is bit-identical, so a reassociation
regression is undetectable). Build these first; they are the gate, not an extra:

1. **Large mixed int/real aggregate** — a model with a `≥8`-term sum mixing
   integer and **real** compartments (e.g. an environmental-reservoir total), so
   `MixedPopSum`'s int-then-real fold order is actually exercised and trap #1
   would fail the gate. (MixedPopSum is likely *untested* by current goldens.)
2. **Forcing inside a binding** — a `let` whose body transitively reads
   `school(t)`, referenced by a rate, run under **Gillespie**, to catch trap #2
   (`expr_is_time_dependent` mis-classifying a `BindingRef` → frozen dynamics).
3. **All-backend matrix** — one binding+reduction model run under chain-binomial,
   tau-leap, Gillespie, ODE, asserting byte-identical trajectories (the landing
   condition above).
4. **Cliff probe** — a P≥64 model as a parse test (too large to commit as a
   golden; lives in the scaling bench), proving D removes the cliff.
   Note overlap: real-compartments-under-chain-binomial is *also* engine bug
   #2/#13, so a real-comp golden may surface that separately — coordinate.

### Two correctness traps to design out (both → a *different number*, not a crash)

1. **Float reassociation via `normalize_expr` (the #1 risk).** Today
   `normalize_expr` (`expander.ml:847-875`) collapses `Pop`/`PopSum` `Add`-chains
   into a flat `PopSum`, which resolves to `IntPopSum`/`MixedPopSum`
   (`resolved_expr.rs:239-251`) — and `MixedPopSum` sums **int terms then real
   terms**, a specific fold order. If a binding body like `N[l]` (5 comps × 21
   ages = 105 terms, possibly mixed int/real) is re-emitted as a *source-order*
   `Reduce{Sum}`, the summation order changes → one ULP in `N` → flips a single
   `rng.binomial(n_src, p_total)` at a probability boundary → the trajectory
   diverges. Small goldens (`sir_spatial_sum` has a 3-term `N`) are
   associativity-blind and would pass. **Rule:** binding bodies that are pure
   `Pop`/`PopSum` additive chains MUST flow through `normalize_expr` and resolve
   to `IntPopSum`/`MixedPopSum` (preserving int-then-real order), **not** `Reduce`.
   Reserve `Reduce` for the spatial sum whose terms are `Mul`-trees (which
   `normalize_expr` already declines to collapse, `expander.ml:1597`). Add a test:
   an extracted `N[l]` binding resolves byte-identically to the inlined `PopSum`,
   on a model with a mixed int/real, ≥8-term `N`.

2. **Catch-all `match` arms silently mis-answer for `BindingRef` (guard bypass).**
   Several functions have `_ => false`/`_ => {}` and would mis-classify a
   `BindingRef`, **silently bypassing safety guards**:
   - `compiled_model.rs:179 expr_is_time_dependent` (`_=>false`) — **most
     dangerous**: a `BindingRef` to a binding that transitively reads `school(t)`
     → "time-independent" → frozen at t=0 under Gillespie = silent wrong dynamics
     (the function's own doc warns of exactly this).
   - `resolved_expr.rs:78 references_state`, `compiled_model.rs:129
     collect_int_comp_deps`, `cli/src/eval.rs:64 references_compartments`.
   These operate on a single `Expr` with **no access to the bindings table**, so
   they cannot answer for a `BindingRef` without an API change. **Fix:** precompute
   per-binding flags (`references_state`, `is_time_dependent`, `param_refs`) once
   at `CompiledModel::new` and have these sites consult them through a `BindingRef`.

### Design corrections

- **`BindingRef` must be by-NAME, not by-index.** Every existing IR cross-ref is
  by-name in JSON, resolved to a `usize` slot at `CompiledModel::new`
  (`Param→param_index`, `Pop→comp_index`, `TableLookup→table_index`); raw `usize`
  lives only in `ResolvedExpr`. So: `Expr::BindingRef(String)` (JSON
  `{"binding_ref":"F_kano_dala"}`) → `ResolvedExpr::BindingRef(slot)` via a new
  `binding_index` map in `ResolveCtx`. By-index is fragile to the reorder B2's
  grad-binding insertion performs, and `BindingRef(47)` is unreadable in goldens.
- **The preamble is needed in ALL backends, not just `step_one`.** `step_one`
  lives only in `chain_binomial.rs`; rates are evaluated by chain-binomial,
  tau-leap, Gillespie, ODE, **and** `intervention.rs`/`observe`. `EvalCtx`
  (`propensity.rs:13`) gains `bindings: &[f64]`, and every `EvalCtx` construction
  site + each backend's main loop must compute the binding slots before evaluating
  rates. (The test plan already asserts invariance on tau-leap — so this is
  blocking, not optional.)
- **Binding extraction needs a free-variable precondition.** "FOI bindings are
  global per step" is true for Kano (`N[l]`, `I_lga[l]` are indexed only by `l`)
  but **not guaranteed in general**: `expander.ml:1540` resolves an indexed-let
  body against `inner_env @ env`, where `env` carries the *enclosing transition's*
  indices, so a model author can let a transition-local index (the `a` of
  `infection[l,a]`) leak into a `let X[l] = …` body. **Rule + new machinery:**
  extract a `let` only if every free variable of its body is bound by the let's
  own declared indices; otherwise keep it inlined. A free-var scan against
  `lb.lindices` does not exist today and must be built (with a diagnostic).

### Completeness checklist (a missed exhaustive arm = compile error; do atomically)

New variants `Reduce` + `BindingRef` must be handled in **all**: OCaml — `ir.ml`,
`serde.ml` (single file; both `expr_to_json`/`expr_of_json` **and** the model
record for the new `bindings` field), `dimcheck.ml:246 infer` (decide: `Reduce`
unifies its terms like `Add`; `BindingRef` takes the binding's inferred dim — so
thread the bindings table into `infer`), `validate.ml`, `lineage.ml:97,152,174`
(**semantic**: parent decomposition can't see through a state-bearing
`BindingRef` — either inline bindings for `#[lineage]` transitions or reject
extraction there), `pp_expr.ml:26` (the `camdl inspect` renderer), `autodiff.ml`.
Rust — `expr.rs` (enum + the Fix-E hand-written `Deserialize` arm), `validate.rs`,
`resolved_expr.rs` (`resolve_expr` + `eval_resolved` + `references_state`), **both**
derivative evaluators (`propensity.rs:228 eval_expr_deriv` *and*
`resolved_expr.rs:441 eval_resolved_deriv`), `inference/hierarchical.rs:82`,
`inference/pgas.rs:1486 collect_param_refs` (**correctness-critical**: a param
reachable only through a `BindingRef` must still be collected or NUTS sees a zero
gradient for it). Plus `model.rs` + schema for the `bindings` field.

### B2 (shared gradients) — reassess; likely unnecessary for the current model

Production gradients use the OCaml-emitted `rate_grad` evaluated by `eval_resolved`
(not the test-only `eval_resolved_deriv`). Crucially, `autodiff.ml:18-20` maps
`Pop`/`PopSum → 0` (state is conditioned-on in the PGAS θ|X step). **Every binding
in the Kano FOI is state-only ⇒ `d(binding)/dp ≡ 0` ⇒ the diamond can't
double-count and B2's emitted-grad-DAG buys *nothing* on the gradient** — once B1
shrinks the rate trees, the inlined grads are already tiny. So:
- Add a gate: scan binding bodies for `Param`. If none (verified true today),
  **skip B2** — keep gradients inlined (B1 row already allows this).
- If a parameter ever enters a binding (fitted gravity exponent, etc.), prefer
  **option 2** (runtime dual numbers via the FD-validated `eval_resolved_deriv`)
  over option 1 — reviewer 1 judged option 1's emitted-grad bookkeeping the
  *higher* correctness risk (two `simplify_fixpoint` passes can yield ULP-divergent
  trees for the same `db/dp`; grad-slot topo order must be proven). The current
  FD gradient check covers only `sir_basic` (non-spatial) — any B2 needs a
  **spatial FD gradient check** added first, or it can land green while wrong.

### Factual corrections to the body below

- "`ResolvedExpr` mirrors `Expr` 1:1" is wrong: `Pop→{IntPop,RealPop}`,
  `PopSum→{IntPopSum,MixedPopSum}` at resolve time. New variants need bespoke
  `ResolvedExpr` twins + resolve arms.
- Regen: `make update-expected` and `ir/expected/` **do not exist**; the target is
  `make update-golden` (+ `update-ocaml-golden`), and there are **two** golden
  trees (`ir/golden/`, `ocaml/golden/`). Version skew to resolve: `ir/VERSION`=`0.6`
  but golden JSON carries `"version":"0.3"` and `golden_deser.rs:40` asserts `0.3`.
- Add a **spatial model to `ir/golden/`** (or a P≈64 synthetic) to the
  trajectory-invariance gate — the small goldens are associativity-blind and
  would let trap #1 ship green.

**Net:** proceed with **D** as scoped (it only needs the `Reduce` arms above; it
kills the cliff and is trajectory-invariant by the left-fold rule). Re-scope
**B1** against this checklist before implementing; treat **B2** as conditional.

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
