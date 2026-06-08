# Runtime binding cache: finding the lever, then landing 2.74×

Date: 2026-06-07
Project: camdl (compiler / runtime)
Tags: runtime, optimization, profiling, foi, spatial, binding-cache

## Context / question

What is the highest-ROI *compiler-side* change for **runtime** speed (forward
simulation and, downstream, particle-filter inference) on a spatially-coupled
model? Compile time is already addressed; the question here is the per-step
evaluation cost that every simulate/filter pays.

The answer turned out to be a Rust runtime change, not an OCaml one — but the
*evidence* that pointed at it came from a new compiler inspection tool.

## The benchmark model (synthetic, Kano-free)

`scripts/gen_scaling_models.py` emits a parametric SEIR × patch × age model whose
force-of-infection has the same *shape* as a dense spatially-coupled measles
model: per-patch aggregates `N[l]`, `I_agg[l]` and a coupling sum **inside** the
infection rate, `kappa * sum(q, W[l,q] * I_agg[q] / N[q])`. That sum is what the
compiler flat-inlines into every `(patch, age)` infection rate tree.

```
python3 scripts/gen_scaling_models.py -P 44 -A 21 --coupling on --observe \
    > /tmp/gen_P44.camdl
# headline horizon: set `to = 3650 'days` (10× the default 365) so steady-state
# per-step eval dominates over fixed setup/IO. → /tmp/gen_P44_long.camdl
```

P=44, A=21, dense all-to-all coupling — the same O(P²) FOI structure a national
model has, with none of the private data.

## Finding 1 — binding reuse is the signal (cost report)

`camdlc inspect --cost-report` on the model:

```
$ camdlc inspect --cost-report /tmp/gen_P44_long.camdl
  transitions   2,772
  bindings      90
  rate nodes    263,340 total, 279 max (infection_p0_a0)
  Reduce terms  40,656 before fold → 40,656 after (0% collapsed)

  top bindings by reuse
    seas      time   size=5  refs=924  ~saved=4,615
    I_agg_p0  state  size=1  refs=945  ~saved=944
    N_p0      state  size=1  refs=945  ~saved=944
    I_agg_p1  state  size=1  refs=945  ~saved=944
    …

  duplicated subexprs (≥3)   6,029
```

Each per-patch aggregate `N[l]` / `I_agg[l]` is referenced **945×** (once per
destination stratum in the FOI sum) and — crucially — recomputed on every
reference, because `BindingRef` evaluated on demand with no memoization:

```rust
// resolved_expr.rs, before:
ResolvedExpr::BindingRef(slot) => eval_resolved(&ctx.model.resolved.bindings[*slot], ctx),
```

The `~saved` column is gated on caching: it counts evals a per-step cache would
collapse (945 → 1). Two observations from the same report:

- **The sparse-coupling fold does not fire here** (0% Reduce collapse): dense
  coupling has no zero `W` cells to drop. The fold is a no-op for dense
  (and, separately, for any `read()`-loaded matrix). Different lever, different
  model class — out of scope for the runtime question.
- Hoisting *more* bindings does not help on its own. A `BindingRef` is recomputed
  on every reference, so the saving only exists once a **cache** does. This is
  why the lever is a runtime cache, not an OCaml hoist.

## Finding 2 — the runtime profile confirms it

samply on a forward `simulate` (chain_binomial, 3650 d), busy-thread leaf
attribution: **`sim::resolved_expr::eval_resolved` is 78.9% of compute**, the rest
being RNG, output hashing (`sha256::compress256`), and allocation. The cost
report's prediction is the measured cost.

## The fix — per-step binding cache

Memoize each binding's value for the lifetime of one propensity-vector
evaluation (one state snapshot). All rates for that state share the cache; the
next state bumps a generation counter to invalidate in O(1). Implemented as a
thread-local entered via an RAII `CacheScope` in `eval_propensities`
(`resolved_expr.rs` + `propensity.rs`); design and the thread-local-vs-EvalCtx
deviation are in `docs/dev/proposals/2026-06-07-runtime-binding-cache.md`.

Correctness is gated by a **byte-identical A/B**
(`rust/crates/sim/tests/gate_binding_cache_ab.rs`): same model, cache on vs off,
identical trajectory hash under every backend, with a runtime hit-count
non-vacuity check (the cache must actually serve hits or the gate proves
nothing):

```
gillespie:       byte-identical, cache hits = 248
tau_leap:        byte-identical, cache hits = 45,260
chain_binomial:  byte-identical, cache hits = 22,630
ode:             byte-identical, cache hits = 22,630
```

The full `cargo test -p sim` suite — including the PGAS/IF2/PF inference tests
and all trajectory baselines — is byte-identical with the cache on, so the change
is invisible to every existing result.

## Before / after — estimate vs realized

Same release binary, cache toggled by `CAMDL_NO_BINDING_CACHE`; wall = best-of-3,
profile = samply busy-thread leaf attribution.

```
                   wall (s)   speedup   eval_resolved (% busy)   eval_resolved samples
  before (off)       9.06       1.0×          78.9%                   7443
  after  (cache)     3.31       2.74×         36.3%                   1194  (6.2× fewer)

  estimate            —        ~1.5×           —                       —    (short-run anchor)
```

The pre-implementation estimate (~1.5×) was anchored on a 365-day run, where
setup/IO dilutes `eval_resolved` to ~46% and Amdahl gives ~1.5×. On the long
horizon `eval_resolved` is ~79%; with the cache cutting eval work 6.2×, Amdahl
predicts `1/((1-0.79) + 0.79/6.2) = 2.96×`. **Realized 2.74× lands at the
long-run end.** The busy-sample ratio (9429/3291 = 2.87×) corroborates the wall.

## A surprise the after-profile surfaced — the cache's own cost

The residual gap between realized 2.74× and the 2.96× Amdahl prediction is
visible in the after profile: `std::thread::local::LocalKey::with` is **12.4%**
of the busy thread, `FnOnce::call_once` 9.0%, `_tlv_get_addr` 4.3% — **25.7%** of
the busy thread is thread-local closure machinery, the indirection paid on every
`BindingRef` access. The hypothesis: reach the cache by reference instead —
resolve the thread-local *once* per propensity vector, thread a `&BindingCacheCells`
through the loop, and each `BindingRef` becomes a slice index. Predicted ~1.2×.

## The EvalCtx-by-reference experiment — a measured negative result

We built it and measured it. The result refuted the prediction: **no speedup.**

Implementation (since reverted): an `EvalCtx::bind_cache: Option<&BindingCacheCells>`
field; `with_binding_cache` resolves the thread-local once (a `&mut` resize/gen-bump
prelude, then a shared `Cell`-interior-mutable borrow for the whole loop); each
`BindingRef` reads `ctx.bind_cache` and indexes. The cache is `Some` only on the
`eval_propensities` path (and `ode_derivs`); `None` everywhere else. Byte-identical
— the A/B gate produced the **same trajectory hashes** as the thread-local design,
and the full `cargo test -p sim` (493 tests) passed.

Controlled A/B — both release binaries built from the same tree (thread-local at
HEAD vs by-ref working tree), timed **interleaved** under identical load, 9 runs:

```
                   wall min (s)   wall median (s)
  thread-local        3.301           3.495
  by-reference        3.317           3.369
  ratio (by-ref/TL)   1.005           0.964      ← brackets 1.0 = no difference
```

The min and median straddle 1.0 in opposite directions: the designs are
runtime-equivalent within noise. The profile shows *why* — the by-ref version did
exactly what it promised at the symbol level, yet wall-clock was flat:

```
                          thread-local      by-reference
  TLS closure machinery       25.7%             0.0%      ← eliminated, as predicted
  eval_resolved               36.3%            63.2%      ← absorbed the same work
```

The 25.7% "thread-local overhead" was **not reclaimable work**. Inlining the
cache access into `eval_resolved` (slice index + `Cell` get/set + generation
compare) does the same arithmetic the `LocalKey::with` closure did; the samples
re-attribute from the closure to `eval_resolved` and the total is unchanged.
macOS `_tlv_get_addr` is cheap enough that resolving the thread-local per
`BindingRef` was never the bottleneck — the leaf-profile made it *look* like a
lever because the work landed on a distinct, suggestively-named symbol. The real
floor is `eval_resolved`'s actual arithmetic (the irreducible FOI sum and the
miss-path `PopSum` evals), which neither design touches.

**Decision: reverted.** A 20-file change that threads a field through every
`EvalCtx` site, leaks a `pub BindingCacheCells`, and touches four inference
modules — for zero measured runtime gain — does not earn its place. The
thread-local design is simpler *and* equal-performance. Evidence profile:
`docs/dev/notes/assets/2026-06-08-binding-cache-byref-experiment.json.gz`.

Lesson: a leaf-profile symbol at 25% is a *hypothesis*, not a lever. Confirm a
predicted speedup with a controlled before/after binary A/B before banking it —
re-attributable work (a closure's body that inlines into its caller) shows up as
removable but isn't.

Incidental finding (kept as a note, not acted on): the thread-local design does
**not** cache the ODE backend — `ode_derivs` evaluates its rate/derivative vector
outside `eval_propensities`, so it never enters the cache scope. The by-ref
experiment extended caching there (113k hits, byte-identical). That is a
separable ~1-line `CacheScope` addition if ODE-backend throughput ever matters;
not pursued — ODE is deterministic and not the spatial-inference workload.

## ROI ceiling

`eval_resolved` was ~79% of the busy thread and is now ~36% (under the cache,
thread-local form). Even driving it to zero caps at `1/0.21 ≈ 4.8×` over the
original, of which **2.74× is banked**. The rest of the budget (RNG, CAS output
hashing, allocation) is not compiler-addressable, and the by-ref experiment
showed the cache-access mechanism itself is *not* a further lever. This is a
clean ~2.7×, not an order of magnitude, and the obvious micro-optimization of the
cache path has been tried and refuted.

## Next

- The cache-access mechanism is settled (thread-local; by-ref tried and refuted).
- The sparse-coupling fold gap (0% collapse on dense / `read()`-loaded W) is a
  separate lever for a different model class; not pursued here.
- ODE-backend caching is available (~1-line) if that path ever becomes hot.

## Repro

```bash
# model
python3 scripts/gen_scaling_models.py -P 44 -A 21 --coupling on --observe \
    > /tmp/gen_P44.camdl   # then set `to = 3650 'days`
camdlc /tmp/gen_P44_long.camdl -o /tmp/gen_P44_long.ir.json

# A/B wall (best-of-3), same binary:
CAMDL_NO_BINDING_CACHE=1 camdl simulate /tmp/gen_P44_long.ir.json \
    --backend chain_binomial --dt 1 --seed 1 --obs-only /tmp/off.tsv   # before
camdl simulate /tmp/gen_P44_long.ir.json \
    --backend chain_binomial --dt 1 --seed 1 --obs-only /tmp/on.tsv    # after

# profiles (committed under docs/dev/notes/assets/):
samply record --save-only --unstable-presymbolicate \
    -o docs/dev/notes/assets/2026-06-07-binding-cache-{before|after}.json.gz \
    -- [CAMDL_NO_BINDING_CACHE=1] camdl simulate /tmp/gen_P44_long.ir.json …
# symbolicate by joining busy-thread leaf addresses (frameTable.address) against
# the .syms.json per-module symbol_table rva intervals.
```

Profile artifacts: `docs/dev/notes/assets/2026-06-07-binding-cache-{before,after}.json.gz`
(+ `.syms.json` sidecars).
