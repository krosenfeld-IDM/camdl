# Consolidated posterior latent-trajectory output across the PF inference methods

- **Status:** Draft **v1**. A design proposal reasoned in types: what
  representations of "a latent state path" exist today, which are duplicates,
  and what single output type all particle-based methods should converge on.
- **Motivation:** today **only PGAS** writes posterior latent trajectories
  (`X_{0:T}` draws), via an ad-hoc inline TSV writer. **PMMH and the bootstrap
  PF discard a path they already have the machinery to produce; IF2 has none**
  (it is MLE). There is **no shared path type** — the forward simulator, PGAS,
  and the PF each invented their own.
- **Required reading:** the inference modules cited inline; this doc quotes the
  actual type definitions so it is self-contained.
- **Scope question this answers:** "can we get complete consolidation across
  PGAS / PMMH / PF?" — **yes at the _output/type/IO_ boundary** (one path type,
  one writer, one flag, shared with `simulate`); **no at the _path-production_
  boundary** (two irreducible mechanisms — CSMC-native vs ancestor-traceback),
  and that's correct, not a wart.

## 1. The types today (the inputs to this consolidation)

There are **three** representations of "a state path over time," none shared:

**(A) Forward-sim `Trajectory`** — what `camdl simulate` produces and writes
(`sim/src/state.rs`):

```rust
pub struct IntState  { pub counts: Vec<i64> }       // integer compartments
pub struct RealState { pub values: Vec<f64> }       // real (ODE) compartments
pub struct FlowVec   { pub counts: Vec<u64> }        // per-transition cumulative flows
pub struct Snapshot  { pub t: f64, pub int_state: IntState,
                       pub real_state: RealState, pub flows: FlowVec }
pub struct Trajectory { pub snapshots: Vec<Snapshot>, /* + transition_diagnostics */ }
```

Granularity: the **output schedule** (sparse, user-chosen). Splits int vs real
compartments. Has a TSV writer + downstream tooling already.

**(B) PGAS `PGASTrajectory`** — the CSMC-AS reference path
(`sim/src/inference/pgas.rs`):

```rust
pub struct SubstepRecord {
    pub counts_before: Vec<i64>,  // for the density (clamping correctness)
    pub counts_after:  Vec<i64>,  // the state path
    pub flows:         Vec<u64>,  // per-substep flows
    pub gammas:        Vec<f64>,  // overdispersion multipliers (density)
    pub t0:            f64,       // realized substep start time
    pub dt_substep:    f64,       // realized substep length (density)
}
pub struct PGASTrajectory { pub initial_counts: Vec<i64>,
                            pub substeps: Vec<SubstepRecord> }
```

Granularity: **per substep** (dense). Carries **density-evaluation internals**
(`counts_before`, `gammas`, `dt_substep`) that exist for computing the
complete-data likelihood, _not_ for output.
`PGASResult.final_trajectory:
PGASTrajectory` returns one; the per-sweep
callback exposes each.

**(C) PF `AncestorTrace` → `SampledPath`** — the smoother
(`sim/src/inference/ancestor_trace.rs`):

```rust
pub struct AncestorTrace {                 // recorded when SMCConfig.record_ancestry
    pub states:      Vec<Vec<Vec<f64>>>,   // [obs_step][particle][compartment], pre-resample
    pub log_weights: Vec<Vec<f64>>,
    pub ancestors:   Vec<Vec<usize>>,      // resampling indices for traceback
    pub obs_times:   Vec<f64>,
    pub projections: Vec<Vec<Vec<f64>>>,   // gh#48 model-predicted obs per stream
    pub stream_names: Vec<String>,
    pub n_compartments: usize,
}
pub struct SampledPath {                   // one back-sampled smoothing draw
    pub states:       Vec<Vec<f64>>,       // [obs_step][compartment]
    pub obs_times:    Vec<f64>,
    pub projections:  Vec<Vec<f64>>,       // per-stream incidence along the path
    pub stream_names: Vec<String>,
}
pub fn sample_paths(trace: &AncestorTrace, n_paths: usize, seed: u64) -> Vec<SampledPath>;
```

Granularity: **observation step** (coarse). Already a back-sampling _smoother_
with a _path output type_ and projections.
`PFilterResult.ancestry:
Option<AncestorTrace>` carries it out of the filter.

**The bottleneck — PMMH never sees it.** PMMH calls the PF through a closure
typed `eval_loglik: dyn Fn(&[f64], u64) -> f64` (`pmmh.rs:233`), and the CLI
builds it from `run_quick_pfilter(..) ->
run_quick_pfilter_full(..).0` — **it
takes `.0`, dropping the `PFilterResult` (and its `ancestry`) on the floor.**
`PMMHResult`/`PMMHStep` carry only `params`

- scalars. IF2 (`IF2Result`) carries only `mle: Vec<f64>` — by design.

### The duplication, stated plainly

`Trajectory` (A) and `SampledPath` (C) are **near-duplicate** "state path +
flows/projections over time" types. `PGASTrajectory` (B) is a **third**, richer
at substep granularity, but most of its richness (`counts_before`, `gammas`,
`dt_substep`) is **density-internal, not output**. So the real situation is: two
output-shaped path types that should be one, plus one density-computation type
whose _output projection_ is just (A) again.

## 2. The design: separate the computation rep from the output type

The mistake to avoid is unifying (B)'s `SubstepRecord` with (A)/(C). Those
density internals (`gammas`, `counts_before`) serve PGAS's likelihood math and
have no business in a posterior-output type. **Consolidate at the output
boundary, not the computation boundary.**

**Target type — a posterior draw of the latent path _is_ a `Trajectory`.** Reuse
the forward-sim type so inference output and `simulate` output share one format,
one writer, and one downstream toolchain:

```rust
/// One posterior draw: the parameter vector AND the latent path it implies.
pub struct PosteriorDraw {
    pub chain: usize,
    pub draw:  usize,            // sweep (PGAS) / kept step (PMMH) / path index (PF)
    pub params: Vec<f64>,        // the θ this path is conditioned on
    pub path:   Trajectory,      // <-- the SAME type `simulate` produces
    pub granularity: Granularity,// Substep | Observation  (honesty about resolution)
}
pub enum Granularity { Substep, Observation }
```

(`Trajectory`/`Snapshot` gains an optional per-stream `projections` field, or it
rides as a sidecar — see open questions; `SampledPath.projections` already
computes it, so folding it in unifies (C) into (A) outright.)

**Each method supplies one adapter `internal_rep -> Trajectory`:**

```
                       ┌───────────────────────────────┐
PGAS  PGASTrajectory ──┤ pgas_to_traj: SubstepRecord →  │
     (substep, native) │   Snapshot{t0+dt, counts_after,│
                       │   flows}  (drop gammas/before) │──┐
                       └───────────────────────────────┘  │
PF /  AncestorTrace ───► sample_paths → SampledPath ──► sampled_to_traj  │
PMMH  (obs, traceback)   (ALREADY EXISTS)   (states→Int/Real, projections)│
                                                                          ▼
IF2   (MLE θ̂) ──► final PF pass w/ record_ancestry ──► sample_paths ──► PosteriorDraw
     (optional, pomp `filter.traj` style)                                 │
                                                                          ▼
                                                        one writer  ──►  on disk
                                                     (same TSV schema as `simulate`)
```

- **PGAS** already produces (B) natively; the adapter is the projection
  `SubstepRecord → Snapshot` (take `counts_after`+`flows`, time = `t0`+
  `dt_substep`; **drop** the density internals). Replaces the ad-hoc inline
  writer in `cli/fit/pgas.rs:559-576`.
- **PF / PMMH** reuse the **existing** `sample_paths` smoother — the only new
  code is _plumbing_: let the PF call hand back `ancestry` (not just `f64`) on
  the draws we want to keep, then `sample_paths` → `Trajectory`.
- **IF2** is MLE, so it has no posterior path — but a single final PF pass at
  the MLE with `record_ancestry`, back-sampled, yields a trajectory band at θ̂
  (the pomp `pfilter`→`filter.traj` idiom). Optional, opt-in.

The internal reps stay exactly as they are; only a thin per-method projection
into `Trajectory` is added. That is the whole consolidation: **N internal
representations → 1 adapter each → 1 output type → 1 writer → 1 format.**

## 3. The PMMH plumbing decision (the `f64` closure)

PMMH's `eval_loglik: Fn(&[f64], u64) -> f64` is where the path dies. Two ways to
recover it:

- **(a) Post-hoc re-run (recommended for v1).** Leave the MH hot loop and the
  correlated-PF path untouched. After PMMH returns, for each kept θ draw, re-run
  the PF once with `record_ancestry=true` and `sample_paths(.., 1)`. Cost: one
  extra PF per _kept_ draw (thinned), not per step. Clean, isolated, no
  closure-signature churn. Statistical note: this yields a _fresh_ `X | θ, y`
  smoothing draw rather than the exact path the chain visited — still a valid
  posterior trajectory draw (θ is the posterior sample; the path is its
  conditional), just not the in-loop one.
- **(b) In-loop capture.** Widen the closure to return
  `(f64,
  Option<AncestorTrace>)` (or a `PFilterResult`) and back-sample on
  accept. Gives the _exact_ visited (θ, X) joint draw, but touches the hot loop
  and the correlated-PF `CorrelatedEvalFn` — more surface, more risk.

Recommend **(a)**; note **(b)** as the upgrade if the exact joint draw is
wanted.

## 4. Honest statistical caveats (these are not just engineering)

The consolidation gives every method a _path output_; it does **not** make the
paths equal quality. State this in the output and the docs:

- **Ancestral path degeneracy.** `sample_paths` back-samples from a bootstrap
  filter's ancestry. The well-known filter-smoother degeneracy collapses
  early-time states onto a few (often one) ancestor, so **PF/PMMH paths are
  unreliable at early times** for long series. **PGAS's ancestor sampling
  specifically mitigates this** — its paths do not degenerate the same way. So a
  PGAS trajectory and a PMMH trajectory are _not_ interchangeable in quality;
  the output should carry the method + a degeneracy caveat, not present them as
  equivalent.
- **Granularity differs and must be labelled.** PGAS paths are substep
  resolution; PF/PMMH paths are observation-step resolution (the PF only records
  states at obs times). The `Granularity` tag makes this explicit rather than
  silently emitting paths of different time resolution under one schema.
- **int vs real compartments.** `AncestorTrace.states` is flattened to `f64`;
  `Trajectory` splits `IntState(i64)`/`RealState(f64)`. The `sampled_to_traj`
  adapter must re-split using the model's compartment layout (and round/guard
  the int part) — a small but real correctness step, not a bit-cast.

## 5. Scope & lift

- **PF/PMMH output (the headline gap): small.** The smoother (`sample_paths`,
  `AncestorTrace`, `SampledPath`) and the recording flag (`record_ancestry`)
  exist. New code is the adapter `SampledPath → Trajectory`, the post-hoc PF
  re-run for kept PMMH draws, and the shared writer + CLI flag. ~1–2 eng-days.
- **PGAS migration: small.** Swap the ad-hoc inline TSV for the shared writer
  via the `SubstepRecord → Snapshot` adapter; preserve the current columns. ~0.5
  day.
- **Shared writer + `--save-paths` flag + format unification with `simulate`:**
  ~1 day (and retire PGAS's undiscoverable `n_trajectories`-only, no-CLI-flag
  surface).
- **IF2-at-MLE final-smooth pass:** optional, ~0.5 day.
- **No IR/schema change, no golden re-bless** — this is runtime output only.

Total ≈ **3–4 eng-days** for full PGAS+PMMH+PF consolidation; the IF2 pass is a
small add-on.

## 6. Open questions for the maintainer

1. **Output type:** reuse `Trajectory` for the path (max consolidation with
   `simulate`; add an optional `projections`), or define a distinct
   `LatentTrajectory`? Recommend reuse — a posterior draw of `X` _is_ a
   trajectory, and sharing the writer/format/tooling is the win.
2. **`SampledPath` fate:** fold it into the `Trajectory`-based output (it is a
   near-duplicate + projections), or keep it as the PF-internal smoothing type
   and adapt at the boundary? Recommend folding the projections into the output
   type and keeping `SampledPath` only as `sample_paths`'s transient return.
3. **PMMH path capture:** post-hoc re-run (a) vs in-loop (b)? Recommend (a).
4. **Degeneracy surfacing:** a hard warning / a `--save-paths` note for PF/PMMH
   that early-time paths are degeneracy-prone, steering users to PGAS for
   trajectory inference? (Error-quality lever — don't let a coarse PF path read
   as a clean PGAS one.)
5. **Posterior summaries:** ship raw per-draw paths only (as PGAS does today),
   or also a summarized band (mean + quantiles over draws) — the thing users
   actually plot? The summary is method-agnostic once paths share one type.
