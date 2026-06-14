# A flat-bytecode rate evaluator (`CAMDL_EVAL_FLAT`): 1.27× per-eval, and why the first answer was wrong

Date: 2026-06-14\
Project: camdl\
Tags: eval, bytecode, propensity, optimization, adversarial-verification

Companion to
[`2026-06-14-expression-eval-walkthrough.md`](2026-06-14-expression-eval-walkthrough.md)
(how `eval_resolved` works) and
[`2026-06-14-pgas-trajectory-io-bottleneck.md`](2026-06-14-pgas-trajectory-io-bottleneck.md)
(where PGAS time goes).

## Question

`eval_resolved` (the recursive `ResolvedExpr` tree-walk) is ~43% of serial PGAS
compute, ~72% at national scale. Can a flat-bytecode VM — compile each rate to a
`Vec<Op>` once, execute with a stack machine — beat it?

## Result

Yes, modestly, and only with a well-engineered VM. On the real polio metapop
model (577 rates), median-of-9, **every variant bit-exact vs `eval_resolved`**:

| evaluator                             | ns/eval  | speedup    |
| ------------------------------------- | -------- | ---------- |
| `eval_resolved` (recursive tree-walk) | 60.2     | 1.000×     |
| `eval_flat`, no superinstructions     | 49.5     | 1.217×     |
| **`eval_flat`** (full)                | **47.5** | **1.268×** |

Stable across runs at **~1.27×** per-eval. Eval is ~half of sim-thread compute,
so the whole-fit ceiling is **~1.13×** here, **~1.18×** at national scale — once
wired under a `CacheScope` (it is; see below). It compounds with cores and the
[BufWriter I/O fix](2026-06-14-pgas-trajectory-io-bottleneck.md).

## The journey — and why the first answer was wrong

The honest part. A naive prototype measured **0.94× (slower)**, and we nearly
closed the loop concluding "a scalar flat VM doesn't pay for camdl's shallow
rates." That conclusion was **wrong**, and an adversarial pass caught it.

| step                                               | speedup   | what                                     |
| -------------------------------------------------- | --------- | ---------------------------------------- |
| naive flat VM (delegating, checked `Vec` stack)    | **0.87×** | the misleading prototype                 |
| full-flatten (no delegation) + unchecked stack     | ~1.22×    | the two structural levers                |
| + `&mut` binding cache (no `thread_local!`)        | ~1.24×    | drops the macOS `_tlv_get_addr` TLS cost |
| + superinstruction opcodes (`Op::Add/Sub/Mul/Div`) | **1.27×** | one dispatch instead of two              |

Two things made the naive prototype lose, and neither is a property of the idea:

1. **Delegation.** The naive VM delegated `IntPopSum`/`BindingRef` back to
   `eval_resolved` — 2592 of ~11,200 ops (23%) were the tree-walk _wearing a VM
   costume_, paying its full cost plus stack overhead. Flattening them flips the
   sign.
2. **A bounds-checked `Vec` stack.** A raw-pointer buffer pre-sized to the
   tape's max depth (`get_unchecked`, no realloc) recovers the rest.

The "bytecode beats tree-walk" folklore comes from _dynamic-language_
interpreters (vtable dispatch, boxed values, env lookups). `eval_resolved` is
the _fast_ kind of tree-walk already — monomorphic `match`, `f64` in registers,
pre-resolved `params[i]` indices, inlined small subtrees — so there is little
overhead for bytecode to remove and the win is bounded at ~1.27×, not 2–8×. (The
2–8× roadmap figure was "SIMD/flattened"; the **SIMD-across-particles** half is
a separate, larger lever this work does not attempt.)

**Process lesson:** the naive prototype plus a confident, mechanistic-sounding
explanation of _why_ it "should" be slow both pointed the wrong way. Two
adversarial subagents — a validity auditor (is the test fair? it was) and a
steelman (build the strongest VM; it hit 1.25×) — overturned it, and an
independent re-run confirmed. The microbench on synthetic _deep_ trees (2.5×)
had over-stated the win for a different reason; the real rates have a **median
of 3–4 AST nodes**, so only the genuinely-flattened arithmetic matters and the
gains are real but small. Measure on the real corpus; steelman a negative result
before banking it.

## Design (the canonical `flat_eval` module)

- **Full flatten.**
  `Const/Param/Pop/IntPopSum/MixedPopSum/BinOp/UnOp/Reduce/Cond/
  TimeFunc/Projected/BindingRef`
  all become ops. The single deliberately-delegated node is `TableLookup` (its
  OOB thread-local machinery is complex and rare; `delegate=0` on this model).
- **Type-total `emit`.** Explicit arm for every `ResolvedExpr` variant, **no
  catch-all** — a new variant is a compile error, so the conversion can never
  silently fall through to a delegate.
- **Unchecked raw-pointer stack.** `compute_max_depth` sizes the buffer at
  flatten time; the executor holds a raw `*mut f64` with `get_unchecked`, no
  realloc.
- **`&mut` binding cache** (`FlatCache`), generation-stamped exactly like
  `resolved_expr::CacheScope`, threaded by `&mut` — direct field reads, no
  per-binding-op thread-local access.
- **Superinstructions.** `Op::Add/Sub/Mul/Div` dispatch directly; the rest go
  via `Op::BinOther → apply_bin`. Worth +4.2% within-run.

Byte-identity rules that matter: `Cond` jumps on `!(pred > 0.0)` (NaN takes the
else branch, ≠ `pred <= 0.0`); `SumN` left-folds like `Reduce`; the div-by-zero
/ Pow / Sqrt / UnOp degenerate handling matches `apply` semantics.

## Wiring (`CAMDL_EVAL_FLAT`, default OFF)

Opt-in, presence-based, parallel to `CAMDL_EVAL_UNRESOLVED` (the two non-default
evaluators). Default OFF → `eval_propensities` takes the unchanged
`eval_resolved` / `CacheScope` path; default models build no `FlatVm` and pay
nothing.

- `ResolvedModel.flat_vm: Option<FlatVm>` built once at `CompiledModel::new`,
  only when the toggle is on.
- A per-thread `FLAT_STATE { cache, scratch }` (same model as `BINDING_CACHE` —
  PF/PGAS parallelise across particles), borrowed **once per propensity-vector
  eval** (so the TLS cost is per-call, not per-binding-op).
- `eval_propensities` branches on `flat_vm`; the flat loop replicates the
  default path's per-rate NaN→`SimError::TableLookup`/`NumericalCollapse` and
  negative-rate handling verbatim. It does NOT enter `CacheScope` (it uses its
  own `FlatCache`).

**Byte-identity gate (the invariant):** flat-on vs flat-off must produce
identical trajectories. Verified — `pfilter` loglik is bit-identical with and
without `CAMDL_EVAL_FLAT=1`:

```
seed 7   : -1318.317258119415  (both)
seed 99  : -1318.3364881917132 (both)
seed 123 : -1318.3657…          (both)
```

Default-off `make test` green (DRIFT 0) — the production path is untouched.

## Repro

```bash
# per-eval A/B on a model IR (bit-exact + median timing):
cargo bench -p sim --bench flat_eval -- <model.ir.json> label

# end-to-end byte-identity (must match):
camdl pfilter model.camdl --params truth.toml --data data/afp.tsv --particles 200 --seed 7
CAMDL_EVAL_FLAT=1 camdl pfilter model.camdl --params truth.toml --data data/afp.tsv --particles 200 --seed 7
```

## Next

- End-to-end `pfilter`/`fit` wall A/B (the real whole-fit factor, expected
  ~1.10–1.13× here, more at scale).
- If it earns its keep: promote toward default, or keep opt-in. The remaining
  scalar headroom is small; the next real lever is SIMD-across-particles (a much
  larger restructure), for which this VM is a stepping stone.
