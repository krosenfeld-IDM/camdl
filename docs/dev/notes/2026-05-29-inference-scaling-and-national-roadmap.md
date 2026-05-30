# Inference scaling, and the path to national-scale fits

Date: 2026-05-29
Project: camdl
Tags: inference, pmmh, pgas, if2, scaling, spatial-coupling, profiling, roadmap

## Context / question

Forward-`simulate` is now fast and memory-safe at Kano scale after Fix E/D/B
([`2026-05-29-foi-scaling-bench.md`](2026-05-29-foi-scaling-bench.md)). The open
question is **inference**: can we get *fits* (PMMH / IF2 / PGAS) to run at
**national scale (P ≈ 774 LGAs)** in a usable timeframe — ideally **< 5 days**,
the rough envelope a Kano-size fit occupies today? This note measures how
inference scales, identifies the ROI-ranked levers from profiling, and lays out
a path. It is the umbrella investigation; the load-bearing pieces (sparse
coupling DSL, binding cache) get their own proposals.

## Measured scaling (this is data, not a model)

Harness: `scripts/gen_scaling_models.py --observe` → `camdl profile --algorithm
pmmh` under a profiler / timer. Figures: `assets/scaling/flamegraph_pmmh.svg`,
`assets/scaling/pmmh_scale.png`.

- **Per-step-eval-bound — verified.** Flamegraph of a PMMH run (P=16, A=7):
  **~72% in `sim::resolved_expr::eval_resolved`** (rate-tree walking), ~9%
  chain-binomial step, ~7% rayon mutex/scheduler, obs-likelihood/resample ≈ 0%.
  The whole cost *is* the propensity evaluation.
- **Patches: ~O(P²)** — measured log-log slope 1.7 over P=16–44, *steepening*
  with P and with particle count (the small-P points are deflated by fixed
  per-run overhead; the cleaner high-N line slopes 1.4 and rising). The O(P²) is
  the dense spatial-coupling sum.
- **Ages: ~linear** (slope ≈ 1.1–1.3) — ages multiply cells but don't enter the
  coupling.
- **Particles: linear once compute dominates** — `wall(400)/wall(100)` climbs
  2.2× (P=4, overhead-bound) → 3.5× (P=32, compute-bound; 4.0 = pure linear). At
  national scale compute utterly dominates, so particles are firmly linear.
- **Iterations / MCMC steps: linear** by construction (each is one more PF sweep).
- **CPU parallelism is already exhausted — verified.** `particle_filter.rs:200`
  and `if2.rs:376` already `par_iter_mut()` across particles. More cores is *not*
  an available lever; the ~7% mutex in the flamegraph is that coordination.

**So: `wall ≈ (iterations) × (particles) × O(P²·A)`, on a fixed core count.**

## Big-run estimates (projected — order of magnitude, not 3 sig figs)

Extrapolated from the tiny-config sweep, scaled linearly in particles/iters
(well-founded) and ×3 for A=7→21 (uncertain), single chain, 1000 particles:

| scale | IF2 (~100 iters) | PMMH (~10k steps) |
|---|---|---|
| Kano (P=44, A=21) | ~50 min | ~3.5 days |
| **national (P=774, A=21)** | **~5 days** | **~1 year** |

National PMMH ≈ **310× Kano** (the (774/44)² patch factor). **No stack of
constant-factor wins reaches 200× — the quadratic must be beaten.** Caveat
beyond the extrapolation noise: national models have ~18× more compartments, and
a bootstrap filter likely needs *more particles* to avoid degeneracy as state
dimension grows — a real factor that could make national worse than the pure-P²
projection (statistical, not compute; flagged for follow-up).

## ROI-ranked levers

| # | lever | type | est. gain | cost | status |
|---|---|---|---|---|---|
| 1 | **Sparse coupling** (dense `W` → neighbour/gravity/radiation + long-range): O(P²)→O(P·k) | algorithmic | **~50×** @ national | DSL + IR + eval | deferred non-goal of the bindings proposal — **now promoted** |
| 2 | **Per-step binding cache** (compute `N[l]`,`I_agg[l]` once/step) | constant, byte-identical | ~2–4× | small | the preamble Fix B skipped; scoped |
| 3 | **SIMD / flattened eval** (vectorise the propensity walk) | constant | ~2–8× | medium | `eval_resolved` is an interpreted tree-walk |
| 4 | **Right method per task** (IF2 MLE, PGAS posteriors, PMMH fallback) | statistical | ~10× fewer iters | medium | PGAS mixes far better than gradient-free RW PMMH |
| 5 | **GPU particle batching** | hardware | ~100–1000× | large | future, costed (below) |
| — | ~~more CPU threads~~ | — | already spent | — | rayon per-particle is live |

### Minimal viable stack for < 5-day national fits

**Sparse coupling (~50×) × binding cache (~3×) ≈ 150×** → national PMMH from
~1 year to **~2–3 days**; IF2 to hours; PGAS posteriors comfortably inside a day.
Sparse coupling *also* shrinks the IR (fewer FOI terms), so it fixes the
national-scale **compile** (which otherwise won't fit in memory). Levers 1+2 are
sufficient; 3/4/5 are headroom, not prerequisites.

## Coupling structure as a first-class, comparable model

The key reframing (per maintainer): the spatial coupling is a **scientific
hypothesis**, not just a perf knob. The dense all-to-all `W` is *one* model
(sum-everything). The national-scale program wants **all of**:

- **local neighbour mixing** — kNN / gravity / radiation kernels (the O(P·k) bulk),
- **a few long-range links** — aviation, transport corridors (a sparse overlay, +m),
- and **model comparison across them** — which coupling structure the data
  actually supports, via the existing `camdl compare` (elpd / CRPS / PIT).

So this is "the middle layer is the program": make coupling a swappable
component. Concretely, **carefully-designed DSL helpers** that *generate* a
sparse `W` — e.g. `gravity_kernel(pop, dist, …)`, `radiation_kernel(…)`,
`knn(coords, k)`, `corridors([(a,b,w), …])`, composable into one coupling matrix
— keep the surface human-first (a health-ministry modeller reads `knn(coords, 8)
+ corridors(air_links)` and knows exactly the mixing assumed). The compiler emits
a sparse FOI sum (over each row's nonzeros only) instead of the dense `sum(q in
patch, …)`. This needs its **own proposal** (DSL grammar, sparse `W` IR
representation, sparse-FOI eval, dimcheck); it is the single highest-leverage
lift and the one that makes national scale tractable.

## Method: right tool per task

- **IF2** — fast MLE / point estimates (~100 iters). Cheapest path to a national
  point fit.
- **PGAS** — production posteriors; gradient-based, mixes far better than PMMH,
  so ~10× fewer iterations for an equivalent ESS. The right national-scale
  *posterior* engine.
- **PMMH** — robust, gradient-free fallback (no `rate_grad` needed); valuable
  where gradients are unavailable/unreliable, but the most iteration-hungry — so
  *not* the method to push to national scale if a posterior is the goal.

The roadmap should recommend per-task, not center PMMH. "Fewer iterations" (lever
4) is as valuable as a compute win and is partly free via method choice.

## GPU (future, costed)

The particle filter is embarrassingly parallel across particles (already rayon on
CPU; GPU is the natural next substrate). Batching particles on GPU is ~100–1000×
and the only lever buying headroom *beyond* national scale (continental,
fine-grained age × space, large ensembles). Cost: a parallel eval path (the
`ResolvedExpr` interpreter → a GPU kernel or a flattened bytecode the GPU runs),
RNG-on-device, and resampling-on-device — a multi-month project touching the eval
core. Not a near-term prerequisite; revisit once 1+2 land and the profiler says
the remaining bottleneck is raw eval throughput.

## Profiling-driven sequencing

1. **Land the binding cache** (lever 2) — cheap, byte-identical, immediate.
   Re-run `make profile-pmmh`; confirm `eval_resolved` share drops.
2. **Prototype sparse coupling** (lever 1) on a national-scale synthetic
   (kNN `W`); measure the actual P-slope flip (O(P²)→O(P·k)).
3. **Re-profile** — the bottleneck *will move*. Likely candidates next: RNG
   draws, resample, obs-likelihood, or memory bandwidth. Let the flamegraph pick
   the next target rather than guessing.
4. **SIMD** the eval (lever 3) if it still dominates.
5. **GPU** (lever 5) as the beyond-national bet.

Each step is gated by a re-profile — no speculative optimisation.

## Next

- Confirm particle linearity — **done** (`pmmh_scale.png`).
- A proposal for the **sparse-coupling DSL + IR + eval** (the big lift).
- Land the **binding cache** (small, scoped) and re-profile.
- Settle the **particles-vs-state-dimension** question (does national need
  proportionally more particles? — re-run with a degeneracy diagnostic).
- A clean, committed `bench-pmmh-scale` harness (the sweeps here were ad-hoc in
  `/tmp`) so the curve is reproducible and trackable as levers land.
