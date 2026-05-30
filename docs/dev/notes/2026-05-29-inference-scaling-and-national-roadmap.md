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
pmmh` (flamegraph) and `camdl fit run` per method (cross-method timing). Figures:
`assets/scaling/flamegraph_pmmh.svg`, `assets/scaling/pmmh_scale.png`,
`assets/scaling/method_scale.png`. The per-step-eval finding is method-
independent — every sampler runs the same particle filter / propensity eval.

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

## Cross-method bench + big-run estimates

A direct IF2-vs-PGAS sweep (`assets/scaling/method_scale.png`; `fit run` per
method, A=7, 50 particles, 10 iters/sweeps, P=4–32) **corrected a wrong
assumption in an earlier draft of this note** — that PGAS would be *heavier* per
iteration because of gradients. It is the opposite:

| P | PGAS (per sweep) | IF2-refine (per iter) |
|---|---|---|
| 4 | 0.4 s | 4.5 s |
| 16 | 2.9 s | 39 s |
| 32 | 9.3 s | 134 s |

- **PGAS benches cleanly** (fixed sweep count → deterministic), slope **1.51**,
  and is *cheap* per sweep. Particle-Gibbs + one NUTS-on-θ step is lighter than a
  bare perturb-filter pass, and gradient-informed proposals mix better → fewer
  sweeps. It wins on *both* axes — matching the tool's own verdict (`fit
  methods`: PGAS is `[stable]` "production Bayesian path"; PMMH is
  `[experimental]`, "degrades for T > 500 observations").
- **The IF2 number here is confounded** — run via a `refine` stage whose
  dt-convergence machinery does a *data-dependent* number of extra filter passes
  (a near-identical P=4 config took 1.0 s in one run and 45 s here). Discard the
  IF2 *absolutes*; a bare-IF2 bench is a TODO.
- **Both scale ~O(P^1.5→2)** — model-size scaling is method-independent (the
  whole point; see the lever ranking).

National posterior estimate, from the *measured* PGAS per-sweep (×(774/32)^1.5 in
P, ×3 for A=7→21; 50 particles, ~500 post-burn-in sweeps):

| coupling | national PGAS per-sweep | national posterior |
|---|---|---|
| dense (current default) | ~3400 s | **~20 days** — over the <5-day bar |
| sparse (÷~50) | ~70 s | **~hours–a few days** — feasible |

So **national PGAS in <5 days is reachable, but only with sparse coupling** — the
same conclusion the PMMH analysis reached, now confirmed on the production
method. Choosing PGAS buys a healthy constant factor and is the right engine; it
does **not** beat the quadratic. (For reference, the PMMH extrapolation that
opened this investigation put national PMMH at ~1 year — `pmmh_scale.png`.)

Caveats so "days" isn't over-trusted: the sweep *count* to a converged national
posterior is unmeasured (PGAS default burn-in is 2000), 50 particles is
optimistic (bigger national state → more particles to avoid filter degeneracy),
and the P-slope ~1.5 is overhead-deflated toward 2 at scale. The robust claim is
the *direction* — sparse coupling moves national PGAS from weeks-to-months down
to days — not the exact figure. A fixed-ESS study (below) is needed to rank
methods on *total* fit time.

## ROI-ranked levers

| # | lever | type | est. gain | cost | status |
|---|---|---|---|---|---|
| 1 | **Sparse coupling** (dense `W` → neighbour/gravity/radiation + long-range): O(P²)→O(P·k) | algorithmic | **~50×** @ national | DSL + IR + eval | deferred non-goal of the bindings proposal — **now promoted** |
| 2 | **Per-step binding cache** (compute `N[l]`,`I_agg[l]` once/step) | constant, byte-identical | ~2–4× | small | the preamble Fix B skipped; scoped |
| 3 | **SIMD / flattened eval** (vectorise the propensity walk) | constant | ~2–8× | medium | `eval_resolved` is an interpreted tree-walk |
| 4 | **Right method = PGAS** (cheap per-sweep + iteration-efficient; IF2 for MLE, PMMH only as fallback) | statistical + constant | fewer iters **and** cheaper per-sweep (measured) | low (PGAS is already production) | bench: PGAS ≪ IF2-refine per iter, both O(P^1.5) |
| 5 | **GPU particle batching** | hardware | ~100–1000× | large | future, costed (below) |
| — | ~~more CPU threads~~ | — | already spent | — | rayon per-particle is live |

### Minimal viable stack for < 5-day national fits

**PGAS (the right method) on sparse coupling** is the path: measured PGAS at
national scale is ~20 days dense → **~hours–days with sparse coupling (÷~50)**.
The binding cache (~3×) and SIMD (lever 3) are headroom on top. Sparse coupling
*also* shrinks the IR (fewer FOI terms), fixing the national-scale **compile**
(which otherwise won't fit in memory). **Sparse coupling is the one
non-negotiable; PGAS is the vehicle; binding cache / SIMD / GPU are headroom.**

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
representation, sparse-FOI eval, dimcheck). The cross-method bench confirms this
is **method-independent** — IF2, PGAS and PMMH all inherit the O(P²) coupling
cost — so it is the single highest-leverage lift *regardless of sampler*, and the
one that makes national scale tractable.

## Method: PGAS is the vehicle (measured, corrected)

The cross-method bench corrected an earlier assumption that PGAS would be
*heavier* (gradients). It is the opposite:

- **PGAS** — the production posterior engine, and *measured cheapest*: ~9 s/sweep
  at P=32 (A=7, 50p), cleanly O(P^1.5). Particle-Gibbs + one NUTS-on-θ step is
  lighter than a perturb-filter pass, and gradient-informed proposals mix better
  → fewer sweeps. Wins on per-sweep cost *and* iterations. Push **this** to
  national scale.
- **IF2** — fast MLE / point estimates; the gradient-free perturb-filter loop.
  Its cost here was confounded by the `refine` stage's convergence machinery (see
  above); a bare-IF2 bench is needed before ranking it against PGAS.
- **PMMH** — robust gradient-free fallback, but `[experimental]` and degrades for
  T > 500 observations (`fit methods`) — *not* the method for national
  posteriors. It was the right place to *start* the scaling investigation (gradient-
  free, simplest), but not the destination.

So: center the national-scale path on **PGAS**, use **IF2** for cheap point
estimates, keep **PMMH** as a fallback. The remaining unmeasured axis is
*iterations-to-converge* per method (per-iter cost × iters = total fit time); the
fixed-ESS study (below) is the disciplined way to settle the ranking.

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

- Particle linearity — **done** (`pmmh_scale.png`); IF2-vs-PGAS bench — **done**
  (`method_scale.png`).
- A **fixed-ESS cross-method study**: run IF2/PGAS/PMMH to the same effective
  sample size and compare *total* wall — the only fair ranking (the
  iterations-to-converge axis, currently unmeasured). Includes a **bare-IF2
  bench** (the refine-stage number here is confounded).
- A proposal for the **sparse-coupling DSL + IR + eval** — the big lift, the one
  thing that makes national scale tractable on *any* method.
- Land the **binding cache** (small, scoped) and re-profile.
- Settle **particles-vs-state-dimension** (does national need proportionally more
  particles? — re-run with a degeneracy diagnostic).
- A clean, committed `bench-inference-scale` harness (the sweeps here were ad-hoc
  in `/tmp`) so the curves are reproducible and trackable as levers land.
