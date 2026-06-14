# How camdl resolves and evaluates rate expressions (and where to optimize)

Date: 2026-06-14\
Project: camdl\
Tags: eval, resolved-expr, propensity, ir, optimization, profiling

A reference walkthrough of the rate-expression evaluation pipeline — the code
path that is ~43% of serial PGAS compute (see
[`2026-06-14-pgas-trajectory-io-bottleneck.md`](2026-06-14-pgas-trajectory-io-bottleneck.md))
and ~72% at national scale (see
[`2026-05-29-inference-scaling-and-national-roadmap.md`](2026-05-29-inference-scaling-and-national-roadmap.md)).
Written to ground optimization decisions in how the IR actually executes.

## The pipeline at a glance

A rate like the polio force-of-infection —

```
beta_c * S[l,a] * ( sum(b in age, C_age[a,b] * I_c[l,b] / N_age[l,b]) + iota )
```

goes through **three representations**, each on a different side of the
OCaml↔Rust boundary, before it produces a number:

```
DSL source ──(OCaml camdlc: expand · dimcheck · autodiff · constant-fold)──▶  Expr  (IR JSON)
                                                                                │  deserialize
                                                                                ▼
                                                            Rust: resolve_expr  ──▶  ResolvedExpr
                                                            (once, at CompiledModel::new)
                                                                                │  eval_resolved
                                                                                ▼
                                                            f64  (per transition · per substep · per particle)
```

The point: all the expensive name-handling and validation happens **once**, up
front, so the thing called billions of times in the inner loop is a pointer-walk
over a tree of `usize` indices — no hashing, no allocation, no error handling.

## Stage 1 — `Expr`: the portable, string-keyed AST

`Expr` (`ir/src/expr.rs:214`) is the serializable contract between camdlc and
the runtime — a 15-variant enum that is the entire expression language:

```rust
pub enum Expr {
    Const, Param, Pop, PopSum, Time, Dt,
    BinOp, UnOp, Cond, TimeFunc, TableLookup,
    Projected, UncheckedDim, Reduce, BindingRef,
}
```

- **String-keyed.** `Param("beta_c")`, `Pop("S_ward3_age1")` — names, not
  indices, because the IR is human-readable and language-agnostic.
- **Single-key JSON object per node**, with a **hand-written** deserializer
  (`expr.rs:287`), not derived. The derived `#[serde(untagged)]` path buffered
  every node into an owned `Content` and trial-deserialized each variant —
  profiling a 2 GB IR found ~50% of `simulate` wall in `content_clone`/drop
  (`expr.rs:206-211`). The manual visitor reads the one key and dispatches in a
  single streaming pass. (Load cost, not eval cost — same "strings are
  expensive, do it once" theme.)

By the time `Expr` reaches Rust it has already been **constant-folded by
camdlc** (`CAMDL_NO_CONSTANT_FOLD` toggles this — it changes the emitted IR,
`cli/util.rs:481`). The gravity-coupling sum that is nominally O(P²) over all
patches arrives already collapsed to the O(P·k) nonzero terms — which is why
disabling the fold costs 1.57× (A/B in the I/O note): the folding shrinks the
tree _before_ Rust sees it.

## Stage 2 — `resolve_expr`: strings → indices, once

At `CompiledModel::new()`, `resolve_expr` (`sim/resolved_expr.rs:128`) walks the
`Expr` tree once and produces a parallel `ResolvedExpr` tree
(`resolved_expr.rs:26`) where every string is a `usize` index into a flat array:

| `Expr` (string-keyed)            | `ResolvedExpr` (index-keyed)                      | resolves against        |
| -------------------------------- | ------------------------------------------------- | ----------------------- |
| `Param("beta_c")`                | `Param(3)`                                        | `param_index`           |
| `Pop("S")` (integer compartment) | `IntPop(0)`                                       | `comp_index→global_int` |
| `Pop("V")` (real/ODE)            | `RealPop(2)`                                      | `global_to_real`        |
| `PopSum([...])`                  | `IntPopSum(vec![0,4,7])`                          | (fast path: all-int)    |
| `TableLookup("C_age", idx)`      | `TableLookup{table_idx:1, oob, table_len, index}` | `table_index`           |
| `BindingRef("N_age")`            | `BindingRef(5)`                                   | `binding_index`         |

Bought here: (1) **all name errors surface at construction**, not in the hot
loop; (2) the `Pop` split (`IntPop`/`RealPop`/`IntPopSum`/`MixedPopSum`) is
decided once — the common all-integer sum becomes the single `IntPopSum` fast
arm; (3) table metadata (`oob`, `table_len`) is **inlined into the node**
(`resolved_expr.rs:63`) so eval never chases back through the model.

Post-resolution, the FOI is schematically:

```
BinOp{Mul, BinOp{Mul, Param(3)/*beta_c*/, IntPop(0)/*S*/},
  BinOp{Add,
    Reduce[ BinOp{Div, BinOp{Mul, TableLookup{1,…}, IntPop(12)/*I_c*/}, BindingRef(5)/*N_age*/}, … ],
    Param(7)/*iota*/ }}
```

`N_age` became a **`BindingRef`** — the hinge for the cache below.

## Stage 3 — `eval_resolved`: the infallible tree-walk

`eval_resolved` (`resolved_expr.rs:404`) is the hot function. Contract:
_infallible — no `Result`, no HashMap probes, just array indexing._ One big
`match` that recurses:

```rust
pub fn eval_resolved(expr: &ResolvedExpr, ctx: &EvalCtx<'_>) -> f64 {
    match expr {
        ResolvedExpr::Const(v)      => *v,
        ResolvedExpr::Param(idx)    => ctx.params[*idx],              // array index, no hash
        ResolvedExpr::IntPop(local) => ctx.int_s.counts[*local] as f64,
        ResolvedExpr::IntPopSum(ix) => ix.iter().map(|&i| ctx.int_s.counts[i] as f64).sum(),
        ResolvedExpr::BinOp{op,left,right} => {
            let a = eval_resolved(left, ctx);   // recurse
            let b = eval_resolved(right, ctx);  // recurse
            match op { BinOp::Mul => a*b, BinOp::Div => if b==0.0 {…NaN} else {a/b}, … }
        }
        …
    }
}
```

Everything it needs is in `EvalCtx` (`propensity.rs:13`): `params: &[f64]`,
`int_s` (integer compartment counts), `real_s`, `t`, `dt`. `Param(3)` →
`ctx.params[3]`, one load. No string ever appears.

Two details that shape cost:

- **Error model is a NaN sentinel.** `eval_resolved` can't return `Result`
  (called from ~30 non-`Result` hot sites), so divide-by-zero / `Pow→inf`
  returns `f64::NAN` (`resolved_expr.rs:451`); the NaN propagates to the one
  `Result`-returning boundary, `eval_propensities`, which converts it to a typed
  `SimError`. An out-of-range table lookup stashes `(table,index,len)` on a
  thread-local and returns NaN (`resolved_expr.rs:543`) → named
  `SimError::TableLookup`. Keeps the hot path branch-light.
- **`Reduce` is a left-fold** (`resolved_expr.rs:568`):
  `terms.iter().map(...).sum()` — deliberately bit-identical to OCaml's
  `((t0+t1)+t2)` add-chain. This is why fold and parallel paths stay
  byte-identical.

## The binding cache — `BindingRef` + `CacheScope`

`N_age[l,b]` (the per-(patch,age) population) appears in the denominator of
every infection term for that stratum. So the compiler **hoists** such shared
subexpressions into model-level _bindings_ (`model.resolved.bindings`); each use
becomes `BindingRef(slot)`. A thread-local, generation-stamped cache
(`resolved_expr.rs:271`) memoizes each binding once per propensity-vector eval:

```rust
struct BindingCache { val: Vec<f64>, stamp: Vec<u32>, gen: u32, active: bool, hits: u64 }
thread_local! { static BINDING_CACHE: RefCell<BindingCache> = …; }
```

Lifecycle via RAII `CacheScope`:

- `eval_propensities` calls `CacheScope::enter(n)` (`propensity.rs:536`) →
  **bumps the generation counter** (O(1) invalidation of the prior state's
  values, `resolved_expr.rs:377`).
- The `BindingRef` arm (`resolved_expr.rs:571`): is the cache active and is this
  slot stamped with the current generation? Hit → return `val[slot]`. Miss →
  evaluate the body, store `val[slot]` + `stamp[slot]=gen`.
- `Drop` sets `active=false` (`resolved_expr.rs:392`) so evals outside the
  propensity loop (obs likelihood, gradient) fall through to on-demand.

This pays (+13%; disabling via `CAMDL_NO_BINDING_CACHE` is 1.15× slower).
**But** every `BindingRef` does `BINDING_CACHE.with(|c| …)` — a thread-local
access — once on a hit, twice on a miss. On macOS `thread_local!` goes through
`_tlv_get_addr`, a real function call (not a register offset like Linux ELF
TLS). That is the ~10% (`LocalKey::with` 8.4% + `_tlv_get_addr` 1.5%) in the
serial flame graph: the memoization saves more than that, but the _access
mechanism_ is pure overhead.

## The driver — `eval_propensities`, per substep per particle

`eval_propensities` (`propensity.rs:503`) per call: (1) scans params for
non-finite values (so a bad NUTS proposal is named, `propensity.rs:521`); (2)
builds `EvalCtx`, enters `CacheScope` (`propensity.rs:536`); (3) loops
transitions: `out[i] = eval_resolved(&model.resolved.rates[i], &ctx)`
(`propensity.rs:557`), NaN-checking each. In the particle filter this runs
**once per particle per substep** — 100 particles × 1455 substeps × 577
transitions per csmc pass. That product, not any single rate, is why
`eval_resolved` self-time dominates.

## Optimization map

| serial cost            | pipeline stage                                              | lever                                                                                |
| ---------------------- | ----------------------------------------------------------- | ------------------------------------------------------------------------------------ |
| `eval_resolved` ~43%   | Stage 3: per-node `match` + recursion + `Box` pointer-chase | **flattened/bytecode eval** (below) — the documented ~2–8× lever                     |
| binding-cache TLS ~10% | Stage 3: `BindingRef` → `thread_local!`                     | thread the cache by `&mut` through `EvalCtx` — keeps +13%, drops the `_tlv_get_addr` |
| RNG ~21%               | _downstream_ of eval (`step_one` draws `Binomial(n,p)`)     | mostly inherent; trivial-`p` already short-circuits (`rng.rs:110`)                   |
| fold (saving ~36%)     | Stage 1, compile-time                                       | done; scales hard with patch count                                                   |

## Flattened-eval prototype (the ~2–8× lever)

> **Update (measured):** this section was written before the prototype existed
> and speculated ~2–4×. The real number is **1.27× per-eval** (a naive first
> prototype even measured 0.94× — _slower_). See
> [`2026-06-14-flat-bytecode-evaluator.md`](2026-06-14-flat-bytecode-evaluator.md)
> for the measured result, why the naive version misled, and the shipped
> `CAMDL_EVAL_FLAT` design. The reasoning below is sound on _mechanism_ but
> over-stated the _magnitude_ — `eval_resolved` is already a fast (monomorphic,
> pre-resolved, f64) tree-walk, so there is far less overhead to remove than the
> dynamic-language interpreters the 2–8× folklore comes from.

The remaining `eval_resolved` cost is **interpreter overhead**: walking a
heap-scattered tree of `Box<ResolvedExpr>` node-by-node — recursion (call
overhead + stack frames) and pointer-chasing (each `Box` is a separate
allocation → cache misses; the `Vec::len` ~3% in the profile is bounds-check
noise on that traversal). A flattened (bytecode / RPN) evaluator removes both.

**Compile** `ResolvedExpr` → a contiguous `Vec<Op>` (postorder), once per rate
at `CompiledModel::new` (alongside `resolve_expr`):

```rust
enum Op {                    // flat, no Box — a few words each, contiguous in a Vec
    Const(f64), Param(u32), IntPop(u32), IntPopSum(u32 /*side-table slice*/),
    Time, Dt,
    Bin(BinOp),              // pops 2, pushes 1
    Un(UnOp),                // pops 1, pushes 1
    SumN(u32),               // pops N, pushes sum (Reduce — left-fold order preserved)
    BindingRef(u32),
    TableLookup { table_idx: u32, table_len: u32, oob: OobPolicy }, // index = top of stack
    JumpIfFalse(u32), Jump(u32),   // Cond → control flow (eager-eval avoided)
}

fn flatten(e: &ResolvedExpr, out: &mut Vec<Op>) {
    match e {
        ResolvedExpr::Const(v)       => out.push(Op::Const(*v)),
        ResolvedExpr::BinOp{op,l,r}  => { flatten(l,out); flatten(r,out); out.push(Op::Bin(*op)); }
        ResolvedExpr::Reduce(terms)  => { for t in terms { flatten(t,out); } out.push(Op::SumN(terms.len() as u32)); }
        ResolvedExpr::Cond{pred,then_,else_} => {
            flatten(pred,out);
            let jf = out.len(); out.push(Op::JumpIfFalse(0));     // backpatch
            flatten(then_,out);
            let j = out.len(); out.push(Op::Jump(0));
            out[jf] = Op::JumpIfFalse(out.len() as u32);          // else target
            flatten(else_,out);
            out[j] = Op::Jump(out.len() as u32);                  // end target
        }
        … // Param/IntPop/BindingRef/TableLookup: push their op
    }
}
```

**Evaluate** with a reused stack (no per-call alloc) in a tight loop — no
recursion, sequential memory access:

```rust
fn eval_flat(prog: &[Op], ctx: &EvalCtx, stack: &mut Vec<f64>) -> f64 {
    stack.clear();
    let mut pc = 0;
    while pc < prog.len() {
        match prog[pc] {
            Op::Const(v)   => stack.push(v),
            Op::Param(i)   => stack.push(ctx.params[i as usize]),
            Op::IntPop(i)  => stack.push(ctx.int_s.counts[i as usize] as f64),
            Op::Bin(op)    => { let b = stack.pop().unwrap(); let a = stack.pop().unwrap();
                                stack.push(apply_bin(op, a, b)); }       // a=2nd, b=top → same order
            Op::SumN(n)    => { let at = stack.len() - n as usize;
                                let s: f64 = stack[at..].iter().sum();    // left-fold → bit-identical
                                stack.truncate(at); stack.push(s); }
            Op::JumpIfFalse(t) => { if stack.pop().unwrap() <= 0.0 { pc = t as usize; continue; } }
            Op::Jump(t)    => { pc = t as usize; continue; }
            …
        }
        pc += 1;
    }
    stack.pop().unwrap()
}
```

**Why it's faster** (typical 2–4× on tree interpreters; the roadmap's 2–8×
includes SIMD on top):

1. **No recursion** — the flat loop has no per-node call overhead or stack
   frames.
2. **Cache locality** — `Vec<Op>` is contiguous (sequential prefetch); the boxed
   tree is scattered across heap allocations (a cache miss per node is the real
   cost, and dominates the match-dispatch).
3. **Smaller, predictable dispatch** — `Op` is a flat enum (jump table); the
   loop is branch-predictor-friendly.

**Byte-identity** (the acceptance gate) is preserved by construction: `Bin`
evaluates `a` (second-from-top) `op` `b` (top) in the same order as the
recursive arms; `SumN` left-folds like `Reduce`; `Cond` jumps reproduce
`if pred>0 {then}
else {else}` (only the taken branch evaluates — no eager-eval
drift). Gate it with a `CAMDL_*`-style A/B (flat vs tree) asserting identical
trajectories, exactly like the fold and binding-cache gates.

**Scope / caveats:** a compile step (postorder, one-time); a reused scratch
stack threaded through (no alloc on the hot path); `Cond` backpatching is the
only fiddly part. The win is ~2–4× on the ~43% eval slice → ~1.3–1.6× on whole
serial compute at this model size, and larger at national scale where eval is
~72%.

A bigger, separate lift — **batched/SIMD eval**: the same rate is evaluated for
100–2000 particles at the same `(t, dt, params)` with only `int_s` varying, so a
flat program could evaluate across particles in a SIMD/columnar loop. That is
the top of the 2–8× range but a much larger restructure (per-particle →
per-rate-×-all- particles); the bytecode VM above is the natural first prototype
and a prerequisite.
