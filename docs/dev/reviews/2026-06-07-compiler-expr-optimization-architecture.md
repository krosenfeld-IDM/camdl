# Expression optimization architecture: evaluating the "FOI / dependency-class / MIR" proposal

Date: 2026-06-07
Scope: the OCaml expr passes (`expander`, `autodiff`, `constant_fold`, `lint`)
and the Rust hot-path evaluator (`resolved_expr`, `compiled_model`,
`propensity`, `eval_stats`, `inference/numerics`).
Status: evaluation of an external design discussion against the current tree,
with verification and a sequenced recommendation. No code changed.

## What this reviews

An external discussion proposed a cluster of compiler-optimization moves for
camdl: recognizing force-of-infection (FOI) structure, a numerically-stable
`hazard_prob` primitive, a *dependency-classification* analysis generalizing the
current `references_state` boolean, **cached** bindings, an *optimization/cost
report*, and a typed internal MIR carrying type/dim/dependency/span facts. A
second pass argued — correctly, in my read — that the existing optimizations are
"good, targeted, empirically guarded," and the right move is **not** to rip them
out but to grow a shared expression-analysis/rewrite layer underneath them.

The discussion was written against the public `main` branch. Below I verify its
load-bearing claims against *this* tree (`feature/unified-timeline`), correct the
few that drifted, and translate the recommendations into a sequence that fits
the maintainer's design rules — measure before optimizing, and consolidate to
the natural seam, not past it.

## Verification: the discussion's claims vs. current code

Verified by an Explore pass over `rust/crates/sim/` plus direct reads of the
OCaml passes. Evidence is file:line.

| Claim | Status | Evidence |
| --- | --- | --- |
| `ResolvedExpr` mirrors the IR expr with pre-resolved `usize` indices; built once at `CompiledModel::new()`; evaluator is infallible (`-> f64`, no `Result`, no `HashMap` in the hot loop) | **TRUE** | `resolved_expr.rs:23–79` (enum incl. `Param(usize)`, `IntPop/RealPop(usize)`, `TableLookup{table_idx,…}`, `Reduce`, `BindingRef(usize)`); built `compiled_model.rs:845–852`; `eval_resolved(expr, ctx) -> f64` `resolved_expr.rs:249` |
| `BindingRef` is evaluated **on demand, with no caching** — repeated refs in one propensity pass recompute the body | **TRUE** | `resolved_expr.rs:400–402: BindingRef(slot) => eval_resolved(&ctx.model.resolved.bindings[*slot], ctx)`; no cache field on `EvalCtx` (`propensity.rs:13–37`) or `CompiledModel` (only `table_values_cache`, `time_func_cache`) |
| `references_state` is a **boolean**; no richer dependency lattice exists | **TRUE** | `resolved_expr.rs:84–102`, `pub fn references_state(expr) -> bool`; `BindingRef(_) => true` (`:99`) |
| `constant_fold` is narrow: resolves const-indexed inline `TableLookup`, drops `Const 0.0` from `Reduce`, **deliberately does not fold Div/Pow/Mod**; A/B-gated byte-identical | **TRUE** | `constant_fold.ml:53–62` (`fold_bin_consts`: Add/Sub/Mul/Min/Max only), `:95–97` (drop-zero `Reduce`), header `:20–27` cites `gate_constant_fold_ab.rs` |
| `Reduce` IR node replaces deep left-nested `Add` chains (serde recursion limit past ~50 patches); currently sum-only | **TRUE** | exhaustive matches in `autodiff.ml:105`, `constant_fold.ml:95`; folded in `:96` |
| Shared-binding extraction ("Fix B") hoists state-only, **param-free**, context-independent `let`s into `model.bindings`, referenced by `BindingRef` | **TRUE** | `expander.ml:43–57` (design comment), `let_is_hoistable` `:1612–1623`, `register_hoisted_binding` `:1631–1637` |
| The hoist excludes params because **`BindingRef` differentiates to 0** — a param-dependent binding would silently produce a wrong gradient | **TRUE — and load-bearing** | `autodiff.ml:196: BindingRef _ -> Const 0.0`; rationale `:193–195` |
| `autodiff.simplify` is a **second** simplifier with a different policy than `constant_fold` (it *does* fold `Div` when denom≠0, `Pow`, and unary `Exp/Log/Sqrt/Sin/Cos/Tanh/Abs`) | **TRUE** | `autodiff.ml:203–265` (cf. `constant_fold.ml:55–62` which refuses Div/Pow) |
| `eval_stats` has process-global atomic fallback counters; `CAMDL_EVAL_UNRESOLVED` reroutes through the slow string-keyed evaluator for differential validation | **TRUE** | `eval_stats.rs:17–21` (`DIV_BY_ZERO`, `POW_NAN_INF`, `UNOP_NAN`, …), switch `:43–74` |
| `dt` is exposed to expr eval via `EvalCtx` for hazard-correction expressions | **TRUE** | `propensity.rs:19–27`; `resolved_expr.rs:278: Dt => ctx.dt` |
| Per-pass compiler timing exists (`CAMDL_TIME_PASSES`) | **TRUE** | `Passtime.time/record` wired through `compiler.ml:44,61,79,…`; `passtime.ml` |

### Corrections — claims that drifted

1. **The numerically-stable hazard helper already exists** — the discussion
   frames `hazard_prob` / `probability_from_hazard` as something to *add*. It is
   already in the tree:

   ```rust
   // inference/numerics.rs:37–44
   pub fn prob_q_from_rate_dt(r: f64, dt: f64) -> (f64, f64) {
       let neg_x = -r * dt;
       let q = neg_x.exp();
       let p = -(neg_x).exp_m1();   // 1 - exp(neg_x), no cancellation near 0
       (p, q)
   }
   ```

   The real gap is narrower and more interesting: this stable form is used by the
   **backends** (chain-binomial / tau-leap), but the **hot-path expr evaluator
   does not use it**. A model author who writes the discretization correction
   *in the DSL* as `(1 - exp(-(γ+μ)*dt))/dt` gets the naive `BinOp::Sub` of
   `1.0` and `(...).exp()` (`resolved_expr.rs`, `Sub`/`Exp` arms) — i.e. the
   cancellation the helper was written to avoid. So the recommendation is not
   "write `expm1`"; it is "**connect** the helper the backend already trusts to
   what the user can write." (`ln_1p`/`log1p` is genuinely absent from
   `rust/crates/sim/src/` — confirmed by grep.)

2. **L401 exists, but in the expander, not the lint module.** The discussion
   cites "L401" as the Euler-correction lint. It is real — but it is an inline
   emit inside the expander (`expander.ml:5023–5129`,
   `Diagnostics.warning … ~code:"L401"` at `:5121`), while L402 lives in the
   structured `Lint` pass (`lint.ml:185–…`). `lint.ml:15` even says "Today only
   the dead-compartment check (L402) is implemented" — true *of the Lint
   module*, misleading about the compiler as a whole. This split is itself a
   data point for the discussion's central thesis (ad-hoc walkers scattered
   across passes), and it means the hazard-pattern **detector** (L401) and the
   stable **computation** (`prob_q_from_rate_dt`) already coexist — unconnected.

3. **Two boolean dependency classifiers already exist, one per language, and
   they disagree by design** — which is the strongest concrete argument for the
   discussion's "generalize `references_state` to a dependency class":
   - OCaml `autodiff.ml:196`: `BindingRef _ -> Const 0.0` — treats a binding as
     **param-free** (so `d/dp = 0`).
   - Rust `resolved_expr.rs:99`: `BindingRef(_) => true` — treats a binding as
     **state-derived**.

   Both are *correct for their question* (one asks "does this depend on a
   param?", the other "does this depend on state?"), but each is a one-bit
   projection of the same missing lattice, computed independently, in different
   languages, with no shared source of truth. That is exactly the seam the
   discussion points at.

## The architecture as it actually stands

There are **four** representations, not the discussion's hoped-for five:

```
Ast            ocaml/lib/compiler/ast.ml      surface syntax + Ast.loc spans
  │  Expander.expand  (resolve names/indexes, stratify, hoist bindings)
Ir.expr        ocaml/lib/ir/ir.ml             flat, fully-expanded, transport contract
  │  serialize → ir.json → deserialize
ResolvedExpr   rust/.../resolved_expr.rs       indices, infallible hot-path eval
```

The `Ir.expr` grammar (verified exhaustive via the autodiff/constant_fold
matches): `Const | Param | Pop | PopSum | Time | Dt | TimeFunc | BinOp | UnOp |
Cond | TableLookup | Reduce | BindingRef | Projected | UncheckedDim`. It carries
**no per-node facts** — no resolved type, dimension, dependency class, or span.
Dimensions are recomputed by `dimcheck` each run; dependency is re-derived by
each pass that needs it. The discussion's "typed MIR" is precisely the
*absent* layer between `Ast` and `Ir.expr`: the expander lowers straight from
surface AST to the serialized contract, so there is nowhere to *attach* facts
that shouldn't cross the OCaml↔Rust boundary.

That absence is not a bug — `Ir.expr` is correctly minimal as a contract. The
question the discussion raises is whether the *passes* would be simpler and less
duplicative if they read facts off a shared annotated form instead of each
re-walking the tree. On the evidence (two boolean classifiers, two simplifiers,
L401-in-expander vs L402-in-lint), the answer is "yes, modestly."

## The one correctness risk to act on regardless

The hoist/autodiff contract is **load-bearing and enforced only by a front-end
predicate**: `let_is_hoistable` (`expander.ml:1612`) must guarantee a hoisted
binding's body is param-free, because `autodiff.ml:196` *unconditionally*
differentiates `BindingRef` to `0`. If a param ever leaks into a hoisted
binding, every gradient through it is silently wrong — and silently-wrong
gradients are the worst failure mode for this software (flat NUTS directions,
un-identifiable params, posterior == prior, no error). This is the same class as
the `TimeFunc`-frozen-param bug already on file (gh#186).

Independent of any optimization work, add a **post-expansion invariant check**:
for every registered binding in `model.bindings`, assert
`free_params(body) = ∅`. Today the property is trusted from the eligibility
heuristic; it should be *checked* on the resulting IR. Cheap, and it converts a
latent silent-wrong-gradient into a loud compile-time failure. TDD: construct
(or hand-edit) a model where a param reaches a hoisted binding, assert the
invariant fires; confirm it passes on the golden corpus.

## Recommended sequence

Honoring "measure before you optimize" and "consolidate to the seam, not past
it." Each step is independently shippable and most are behavior-preserving.

1. **Binding param-free invariant check** (correctness, not perf). As above.
   Do this first — it's cheap insurance on an existing risk.

2. **Dependency classification as the shared seam.** Replace the two ad-hoc
   booleans with one analysis that returns a small lattice
   (`Const | Data | Param | Time | State | Projected`, join-semilattice). Author
   it once where the facts live; project it to the existing booleans so callers
   don't change behavior:
   - `references_state(e)  ≡  dep(e) ⊒ State`
   - autodiff's `BindingRef → 0` becomes "binding is `Param`-free", now *derived*
     rather than asserted.
   This is the unification the discussion wants and it directly retires the
   asymmetry in correction (3). **Open design question for us:** author it in
   OCaml (over `Ir.expr`, the natural place, feeding the invariant in step 1 and
   reports in step 3) or in Rust (over `ResolvedExpr`, where the runtime caching
   in step 4 needs it)? My lean: OCaml is the source of truth for *structure*
   (it drives hoisting + reports), Rust needs only the coarse runtime
   generation class for caching — so compute the lattice in OCaml, and let Rust
   keep a minimal runtime projection. Worth deciding together before building.

3. **Optimization / cost report** *before* any new optimizer — the discussion's
   own ordering, and the right one. A `camdlc inspect MODEL --cost-report` (or
   `CAMDL_*` env, mirroring `CAMDL_TIME_PASSES`) over the compiled IR: per-rate
   node counts, `Reduce` term counts before/after fold, bindings by dep class
   with reference counts and "node-visits saved if cached," and a count of
   rewrite-eligible idioms (`1 - exp(-x)`, duplicated subexpressions). This is
   the analogue of `eval_stats` (which already reports *numerical* pathologies)
   for *cost*. It tells us whether steps 4–5 are worth doing on real models
   (national-scale spatial) rather than assumed.

4. **Cached binding evaluation**, gated by the dep class from step 2. The
   discussion's highest-value runtime move after `ResolvedExpr`, and verified to
   be a real gap (no cache exists today). Start with the simplest useful
   generation: `State`-class bindings cached once per propensity/state
   evaluation, invalidated when state advances. Keep an env switch to disable
   (A/B), and gate on the existing byte-identical trajectory test pattern
   (`gate_constant_fold_ab.rs` is the template). Only build it if step 3 shows
   bindings are actually re-evaluated enough to matter — on a single-patch SIR
   it won't be; on a 64-patch spatial FOI it should be.

5. **Connect the hazard helper to the surface** (numerical correctness, not
   speed). Given L401 already *detects* the `1 - exp(-rate*literal)` pattern and
   `prob_q_from_rate_dt` already *computes* it stably, the missing piece is a
   blessed way to invoke the stable form — either a named DSL primitive
   (`hazard_prob(rate, dt)`, which the human-first DSL philosophy favors over a
   silent rewrite) or a guarded evaluator rewrite of the recognized pattern.
   Gate with a trajectory/likelihood comparison test showing the stable form
   only *improves* accuracy near zero. This is a numerical-stability change with
   a real (if small) effect on results — it touches the "doing uncertainty
   right" priority, so treat it as inference-adjacent, not cosmetic.

### Deliberately deferred

- **A bespoke sparse-FOI kernel.** `constant_fold` already collapses dense
  `O(P²)` coupling to `O(P·k)` by dropping zero-`W` terms, A/B-proven. Do not
  build a hand-written kernel until step 3's report shows tree-eval over the
  *remaining* `k` terms is still the bottleneck. (This matches the
  national-scale roadmap's "profiling-gated sequencing.")
- **A full typed MIR.** The annotated layer is worth *approaching* via step 2
  (dependency facts) and step 3 (cost facts attached to the report), not via a
  big-bang new IR. If those facts start wanting to live on the nodes rather than
  be recomputed, that is the signal to materialize the MIR — earned, not
  speculative.
- **E-graphs / equality saturation.** The current wins are domain-specific
  (sparse collapse, shared aggregates, gradient simplification); none needs a
  general rewrite engine yet. Revisit only if algebraic-rewrite complexity
  actually explodes.
- **Merging the two simplifiers.** `autodiff.simplify` and `constant_fold`
  differ for a *reason* (gradient exprs tolerate Div/Pow folding; rate exprs
  defer degenerate Div/Pow to the evaluator to stay byte-identical with
  runtime). That is "consolidate to the seam, not past it": share the traversal
  boilerplate (a `map_expr`/`fold_expr` in a small `Expr_tools`), keep the two
  *policies* distinct. Unifying the policies would be a leaky abstraction.

## Bottom line

The discussion is high-quality and its big claims hold up against the code; the
maintainer's existing optimizations are well-targeted and should stay. The
genuinely actionable core is small and ordered: **(1)** check the binding
param-free invariant now (it guards a live silent-wrong-gradient risk), **(2)**
replace the two one-bit classifiers with one dependency lattice (the real seam),
**(3)** build the cost report before any new optimizer, then **(4)** cache
bindings and **(5)** wire the existing hazard helper to the surface — each gated
on evidence and a byte-identical/accuracy test. Everything heavier (FOI kernels,
typed MIR, e-graphs) is correctly deferred until a measurement asks for it.
