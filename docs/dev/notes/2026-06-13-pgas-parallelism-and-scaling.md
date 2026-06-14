# PGAS particle-parallelism and the national-scale wall

Date: 2026-06-13
Project: camdl
Tags: inference, pgas, particle-filter, parallelism, scaling, profiling, high-dimensional

## Context / question

Daniel Klein is fitting a Sokoto cVDPV2 polio metapopulation — 244 wards × 3 age
× 2 vaccine-source = 4,393 compartments, ~18k transitions — with PGAS, and hit a
wall. Three issues capture it: #209 (PGAS particle filter is single-threaded),
#207 (9.3 GB/sim, memory-bound), #185/#202 (sparse coupling). This note settles
what the actual levers are, separates the engineering walls from the statistical
one, and scopes the parallelization lift.

## Finding 1 — PGAS does not parallelize particles (verified in code)

The production Bayesian engine propagates and weights particles **serially**;
only whole chains and parallel-tempering rungs run on separate cores. The other
three filters are particle-parallel:

| engine | particle propagation | evidence |
| --- | --- | --- |
| bootstrap PF | parallel | `particle_filter.rs:239` `par_iter_mut` |
| PMMH (correlated PF) | parallel | `correlated_pf.rs:381` `par_iter_mut` |
| IF2 | parallel | `if2.rs:413` `par_iter_mut` |
| **PGAS (`csmc_as`)** | **serial** | `pgas.rs` `for j in 0..n_particles` at 1134, 1194, 1245 |

The 2026-05-29 roadmap assumed "the particle filter is embarrassingly parallel
across particles (already rayon on CPU)" — true for PF/IF2/PMMH, **false for
PGAS-CSMC**. Daniel found the gap.

Consequence at his scale: a 5000-particle sweep ≈ 5000 × 67 s/sim ≈ 90+ hours on
one core; particle-parallel across 120 cores ≈ 45 min. Up to a ~120× wall-clock
factor — bounded by core count.

## Finding 2 — the profile measured the wrong engine; regime is unsettled

A fresh leaf profile of the *PMMH* inner loop (P=16, A=7, dense coupling; raw in
`assets/2026-06-13-pgas-scaling/`) shows the busy thread is **62.7% `eval_resolved`**
(FOI rate trees), **24.4% binding-cache thread-local protocol**, RNG 4.3%, engine
step 5.6%, obs likelihood ~0%, CAS hashing ~0%. 31.8% of all thread-samples are
parked rayon workers (small-scale under-utilization of the *parallel* path).

But this is P=16 (fits in cache) and the *parallel* engine — not Daniel's
regime. His 8-particle PGAS probe used **~44% of one core**, not ~100%. A serial
CPU-bound loop saturates a core; 44% says it is **stalling** — almost certainly
memory-bandwidth-bound (4,393 compartments × large per-substep buffers thrashing
cache at 19 GB RSS). So the per-particle-step regime at P=244 is likely
*different* from the P=16 profile, and that difference decides everything:

- **CPU-bound at scale** → #209 gives ~linear speedup (≈120×); the headline move.
- **Memory-bandwidth-bound** → #209 gives *sublinear* speedup (cores contend for
  one memory bus); #207 (leaner per-particle state) becomes the real lever.

### Measured (P=244 synthetic, `--shape polio` matching his structure)

Reconstructed his model with `gen_scaling_models.py --shape polio` — faithful to
his exp-07 codegen: per (patch,age) stratum **S, I_v (vaccine-derived), I_c
(circulating), R** (I_v reverts to I_c at `mu_rev`) + a single un-stratified AFP
accumulator; within-patch FOI mixes ages through a contact matrix `C_age`;
cross-patch spread via gravity import transitions (`imp_I_v`/`imp_I_c`, the
guarded Reduce the fold collapses); aging, age-split waning, births, per-(comp,age)
mortality; `incidence(afp_influx)` obs. At P=244,A=3: **2,929 compartments**
(4/stratum + AFP) / **11,713 transitions** / 1.63 s compile (raw in
`assets/.../polio244_measurements.tsv`). His headline **4,393 = 6/stratum**
(I_v/I_c each ×2 vaccine-source) — an optional knob closes it; the FOI/transition
*shape* (age-mixing + gravity fold + demography) is what matters and is matched.
Sparse top-12 fold fires (95% Reduce collapse). Forward sim (chain_binomial,
dt=2, 12 yr, grad-full IR): **7.8 s, 2.81 GB RSS, user/real = 0.95
(CPU-saturated)**. Grad trees are ~90% of the IR (157 MB full vs 14.8 MB minimal)
— a major PGAS RSS driver (#207).

**Thread-scaling of the parallel bootstrap PF** (the same per-particle work
#209 would parallelize in PGAS; `RAYON_NUM_THREADS` sweep, 64 particles, 2 yr;
`pf_thread_scaling.{png,tsv}`):

| threads | wall (s) | speedup | efficiency |
| --- | --- | --- | --- |
| 1 | 30.08 | 1.00× | 100% |
| 2 | 15.05 | 2.00× | 100% |
| 4 | 7.85 | 3.83× | 96% |
| 8 | 4.00 | 7.52× | 94% |
| 16 | 2.83 | 10.65× | 67% |

**User-CPU stays flat at ~30 s through 8 threads** (rising to 35 s only at 16) —
the signature of **CPU-bound, not memory-bandwidth-bound** work. Bandwidth
contention would inflate user time and plateau the speedup early; instead it is
near-linear to 8 cores. The 67% at 16 is **load imbalance + the serial
resampling barrier** at 64 particles / 16 threads (4/thread), not a memory wall —
Daniel's 5000 particles / 120 cores (≈42/core) balances far better, so his
high-core efficiency should *exceed* this. Caveat: measured at 2.81 GB (fits the
M4 Max caches); at his 9.3 GB grad-full footprint, shared read-only rate-tree
traffic across 120 cores could introduce bandwidth contention this small test
cannot see — confirm at his real RSS.

**Verdict: the gating question resolves in #209's favor.** The per-particle
work is CPU-bound and parallelizes near-linearly, so parallelizing PGAS's serial
CSMC is a real ~linear lever (not bandwidth-capped) up to the load-balance limit.
`--parallel` is a no-op for a single `camdl pfilter` run (only honored in
`--replicates` mode, `pfilter.rs:419`); the inner particle `par_iter` uses the
global pool — `RAYON_NUM_THREADS` is the real control. PGAS's CSMC has neither
(serial loops), which is exactly #209.

## Finding 3 — the statistical wall: a constant factor cannot beat an exponential

Even granting #209, particle filters degenerate as the *effective observation
dimension* grows. Snyder, Bengtsson, Bickel & Anderson (2008), *Obstacles to
High-Dimensional Particle Filtering*, MWR 136:4629–4640: the filter collapses
(max normalized weight → 1) unless

    log N  ≳  τ²/2,     i.e.   N ≈ exp(τ²/2),

where τ² = Var(log w) is the variance of the log importance weights at an
assimilation step. For D independently-observed dimensions each contributing
per-observation log-likelihood variance ν, the variances add: τ² ≈ D·ν, so

    N ≈ exp(D·ν / 2),   exponential in the number of independently-observed dims.

The driver is the **observation** dimension, not the 4,393-dim state. Daniel
already saw this empirically: an aggregate observable (D≈1) sampled finite
log-likelihood fine; per-patch observables (D≈244) degenerated.

Worked estimate for D=244 (ν = per-patch weekly-count log-lik variance, the
empirical unknown — measure it, don't assume):

| ν (per patch) | τ² = 244·ν | N ≈ exp(τ²/2) | memory @ 1 GB/particle |
| --- | --- | --- | --- |
| 0.05 | 12.2 | ~450 | ~450 GB (fits one big box) |
| 0.10 | 24.4 | ~2×10⁵ | ~200 TB |
| 0.25 | 61 | ~2×10¹³ | absurd |
| 0.50 | 122 | ~3×10²⁶ | impossible |
| 1.00 | 244 | ~10⁵³ | impossible |

So required N spans *feasible* to *physically impossible* across a plausible ν
range. **τ² is directly measurable** from a pilot run — it is the spread of
`log_weights[j]` at an observation substep (`pgas.rs:1246`). That single number
tells us which regime Daniel is in. A constant-factor speedup (#209: ≤120×)
cannot rescue the high-ν regime; that needs a *block/local* particle filter that
localizes the weight update to break D into independent low-dim blocks
(Rebeschini & van Handel 2015, *Can local particle filters beat the curse of
dimensionality?*, Ann. Appl. Probab. 25:2809–2866) — a research project, not a
`par_iter`.

## Memory does not scale with cores

All N particles must be resident simultaneously (resampling reads the whole
swarm; you cannot stream it). Cores process the resident swarm concurrently —
they neither multiply nor divide particle memory. Per-core overhead is only
thread-local scratch (the binding cache: ~90 f64 × cores ≈ negligible; the
`StepScratch`/RNG are already per-particle, not per-core). So **#209 is
memory-neutral**: it buys wall-clock, not headroom. The memory wall is set by
N (Finding 3) × per-particle state (#207), independent of core count. You cannot
trade cores for memory here. PGAS's ~1 GB/particle is dominated by the
full-trajectory history stored for ancestor-sampling traceback
(`history_counts_{before,after}`, `history_flows`, `history_gammas` pushed every
substep, `pgas.rs:1257`) — absent in the other engines, which is why only PGAS
carries it.

## The parallelization lift (#209) — medium, well-scoped

`csmc_as` (`pgas.rs:958`) has three expensive per-particle loops, all
embarrassingly parallel (each writes only its own slot, reads shared
`&model`/`params`):

1. **Propagation** (`step_one`, line 1134) — the dominant cost. Skip `j_ref`.
2. **Ancestor-sampling density** (`log_transition_density_substep`, line 1194) —
   comparable cost; writes `ancestor_log_w[j]`.
3. **Weighting** (`log_likelihood_from_flows_and_counts`, line 1245).

The sync points (systematic resample 1110, categorical ancestor sample 1225,
history push 1257) are cheap or inherently serial and stay so.

Why it's tractable:
- **RNG is already per-particle** (`rngs[j]`, own stream) → parallel is
  **byte-identical** to serial, the same guarantee the other three filters rely
  on. The expensive loops write disjoint slots (no reduction) so order is
  irrelevant; the serial reductions read after a barrier.
- The **binding cache is already thread-local** → per-worker caches fall out for
  free (`CacheScope` is entered inside `step_one`).
- The `par_iter_mut().zip(...).collect::<Vec<Result<…>>>()` error-collection
  pattern already exists three times in the crate to copy.

Wrinkles: the `j_ref` skip in the parallel propagation (enumerate + early-return,
or split off the reference); error propagation through the closure (collect
Results); and the **ancestor-sampling step is subtle, audited code** (gh#audit-H8,
IM6 — pre/post-resample state pairing) so it needs care + the existing PGAS
determinism/golden tests, plus a new serial-vs-parallel A/B gate mirroring
`tests/gate_binding_cache_ab.rs`.

Estimate: ~1–2 focused days. Low architectural risk (pattern established,
determinism free from per-particle RNG); main cost is byte-identity verification
on the ancestor-sampling path.

## Next

1. **Profile PGAS at/near P=244** — settle CPU-bound vs memory-bandwidth-bound
   (Finding 2). Decides whether #209 is ~120× or sublinear. Use `hyperfine` for
   wall-clock A/B; `samply` for leaf attribution. Get Daniel's model (offered) or
   reconstruct a comparable metapop with `gen_scaling_models.py`.
2. **Measure τ²** (log-weight variance at an obs substep) on his model — the one
   number that says whether large-N PGAS is viable at all (Finding 3).
3. Then sequence: #209 (parallel CSMC) if CPU-bound and τ² modest; #207 (leaner
   per-particle history) if memory-bound; block/local PF if τ² is large.

Raw data: `assets/2026-06-13-pgas-scaling/` (`pmmh_inner_loop_leaves.tsv`,
`particle_parallelism_audit.tsv`, `pmmh_breakdown.png`).
