---
date: 2026-06-07
status: proposal
related: ../../rust/crates/sim/src/resolved_expr.rs, ../../rust/crates/sim/src/propensity.rs
evidence: ../notes/ (cost-report + samply profile of a dense spatial model)
---

# Runtime binding cache: evaluate each model binding once per propensity step

This is a **Rust** runtime change (the propensity evaluator). The OCaml compiler
already emits the shared bindings (`N[l]`, `I_agg[l]`, …) via Fix-B hoisting;
nothing OCaml-side changes. The win is purely in how the runtime *re-uses* them.

## Problem

`BindingRef` is evaluated on demand, with no memoization:

```rust
// resolved_expr.rs, today:
ResolvedExpr::BindingRef(slot) => eval_resolved(&ctx.model.resolved.bindings[*slot], ctx),
```

In a spatially-coupled model the per-source aggregates `N[q]` (population) and
`I_agg[q]` (infectious) appear once per destination stratum in the FOI
`sum(q, W[l,q] * I_agg[q] / N[q])`. On a dense P=44, A=21 model that is **945
references each**, and every reference re-runs the binding's PopSum from scratch
*within a single propensity-vector evaluation*:

```
cost report (gen_spatial P=44 dense):
  N_p0      state  size=1  refs=945  ~saved=944    ← "saved" is gated on caching
  I_agg_p0  state  size=1  refs=945  ~saved=944
```

The profile lands the cost exactly there: on the simulation thread,
`sim::resolved_expr::eval_resolved` is **46–54% of compute** (46% on this P=44
model; 54% on a national-scale model), dominated by these redundant `BindingRef`
re-evaluations. Hoisting more bindings does **not** help on its own — a
`BindingRef` is recomputed on every reference, so the saving only exists once the
cache does.

## Design

Memoize binding values for the lifetime of one propensity-vector evaluation (one
state snapshot of one cell/particle). All rates for that state share the cache;
the next state bumps a generation counter to invalidate in O(1).

```rust
// EvalCtx gains a per-evaluation binding cache. Interior-mutable (Cell) so the
// existing `eval_resolved(expr, &EvalCtx)` signature is unchanged:
pub struct EvalCtx<'a> {
    // … existing fields (model, int_s, real_s, params, t, dt, …) …
    pub bind_val: &'a [Cell<f64>],   // one slot per binding (allocated once, reused)
    pub bind_gen: &'a [Cell<u32>],   // per-slot generation stamp
    pub gen:      &'a Cell<u32>,     // current generation; bump = invalidate all
}

// resolved_expr.rs:
ResolvedExpr::BindingRef(slot) => {
    let g = ctx.gen.get();
    if ctx.bind_gen[*slot].get() == g {
        ctx.bind_val[*slot].get()                              // hit
    } else {
        let v = eval_resolved(&ctx.model.resolved.bindings[*slot], ctx);
        ctx.bind_val[*slot].set(v);
        ctx.bind_gen[*slot].set(g);
        v                                                      // miss → fill
    }
}
```

Invalidation is one increment, where the state the rates read changes:

```rust
// propensity / backend step, before computing all transition rates for a state:
ctx.gen.set(ctx.gen.get().wrapping_add(1));
```

Notes that keep it correct:
- Bindings are topologically ordered (a `BindingRef` only references earlier
  ones), so a miss that recursively evals a body hits the earlier slots' caches.
- The cache is **per cell/particle propensity eval** and `Cell` is `!Sync`, so
  parallelism must stay *across* cells/particles (each owning its own
  `bind_val`/`bind_gen`), never *within* one propensity vector — which is already
  how the backends batch. Confirm this before wiring the construction sites.
- A generation stamp (not `clear()`) makes invalidation O(1) regardless of
  binding count.

## Expected speedup

`eval_resolved` ≈ 54% of sim-thread compute (national model); within it the
redundant work is `N[q]`/`I_agg[q]` recomputed ~945× each (a PopSum of ~105 / ~21
terms). Caching collapses 945 evals → 1 per binding per step. The irreducible FOI
sum (P² multiply-adds) and the non-rate 46% (RNG, output, alloc) remain:

```
eval_resolved 3× faster → 1/(0.46 + 0.54/3) = 1.56× overall
eval_resolved 4× faster → 1/(0.46 + 0.135) = 1.68×
hard ceiling (eval_resolved → 0)           = 2.17×
```

**Estimate: ~1.5× overall** — eval_resolved ~3× faster (the PopSum recomputation
is the bulk; the FOI sum is not).

## Before / after — estimate vs realized

Benchmark: `gen_spatial P=44 A=21` dense coupling, chain_binomial, dt=1, seed 1
(reproducible via `scripts/gen_scaling_models.py`; the colleague's national
model is private and not used here).

```
                          wall (s)    eval_resolved (% busy thread)
  before (no cache)         1.57           46%        ← measured
  after  (binding cache)    TBD            TBD         ← fill on implementation
  predicted (this run)      ~1.1          ~15%        (≈1.4× — setup-heavy short run)
```

The 365-day run is setup-/IO-heavy, which dilutes the factor; the headline
before/after will also run a longer horizon (per-step eval dominates) so the
realized factor is comparable to the ~1.5× estimate. Profile artifacts:
`docs/dev/notes/assets/` (`before` captured; `after` on implementation).

## Lift / risk / gate

~50–100 lines (`resolved_expr.rs` cache arm + `EvalCtx` fields + the propensity
/ backend construction sites + invalidation). Medium lift; hot-path,
inference-adjacent. Gate:

1. **Byte-identical A/B** (`gate_constant_fold_ab.rs` pattern): same model,
   cache on vs off, assert identical trajectories under every backend, with a
   non-vacuity check that the cache actually serves hits (else it proves
   nothing).
2. Re-run the profile above; record realized wall + `eval_resolved %` in the
   table.

No OCaml change. Deferred follow-up: a per-dependency-class generation (Time /
Param bindings cacheable across more steps) — only if the profile after step 1
still shows binding re-eval; the per-step cache already captures the FOI win.
