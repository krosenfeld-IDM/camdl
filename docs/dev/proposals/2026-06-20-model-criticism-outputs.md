---
status: proposal
date: 2026-06-20
---

# Model-criticism outputs: posterior-predictive emission and held-out evaluation

**Status:** Proposal **Date:** 2026-06-20

A spatial-model analyst's figure pipeline today is three camdl outputs stitched
together in numpy: conditioned latent trajectories (globbed from per-sweep PGAS
TSVs), a posterior-predictive ensemble (forward-simulated per draw, with the
observation noise hand-rolled), and a free-forward spread. The fragile seam is
the hand-rolled observation model — assembling the negative-binomial draw
outside camdl is what let a smoother mean get silently substituted for a forward
predictive without the analyst noticing. The two objects looked alike on a plot;
nothing in the pipeline made them structurally distinct.

The fix is to make every model-criticism artifact a **named, labelled camdl
output** so the wrong objects cannot be confused for each other, and so the
observation model lives in one place. Much of this is already built or already
designed; this document is the map, and specifies the two pieces that are
neither.

## What already exists (and was being reimplemented by hand)

`camdl simulate --draws <draws.tsv> --obs` already draws parameter vectors from
a file, forward-simulates one trajectory per draw, **applies the observation
model, and samples `y_rep ~ p(y | x)`** with the correct noise —
negative-binomial via the Gamma–Poisson mixture, Poisson, binomial,
beta-binomial, normal. The output is a wide TSV — `time` plus one value column
per stream, with `replicate`/`scenario`/`draw` id columns prepended when the run
is an ensemble.

Verified on the release binary against a NegBinomial golden:

```
$ camdl simulate cholera_siwr.ir.json --backend ode --draws draws.tsv --obs-only pp.tsv
replicate  draw  time  reported_cases
1          1     7     388        # NB draw around the ODE mean, not the mean
1          1     14    158
1          2     7     ...        # second draw → its own y_rep
```

Two seeds give different counts (388 vs 1093 at t=7), confirming the noise is
sampled, not the deterministic projection emitted. The sampler is
`sample_obs_resolved` (`sim/src/inference/obs_model.rs`), the same routine the
particle filter and reactive triggers call — there is one observation-model
implementation, and the hand-rolled numpy step was duplicating it.

The gaps are ergonomic, not capability:

1. There is **no `--draws posterior`** — the analyst must hand-type the
   content-addressed path to the fit's `draws.tsv`. Accepted `--draws` values
   are a file path, `uniform`, or `prior`; `--fit` only supplies priors for
   `--draws
   prior`.
2. `y_rep` (the noisy observation) and the **latent incidence it is built from**
   land in separate files (`--obs` writes trajectory and obs separately;
   `--obs-only` drops the trajectory). The analyst's ideal artifact — one long
   frame carrying both `(draw, time, stream, y_rep, latent_incidence)` — needs a
   join today.

## The adjacent pieces (already designed or already filed)

This proposal does **not** re-specify these; it sequences against them.

- **Conditioned latent-trajectory output →
  `2026-06-09-latent-trajectory-output-consolidation.md`.** Replaces the ~200
  wide per-sweep PGAS TSVs with one tidy long `stage/trajectories.tsv`
  (`chain  draw  time  S E I R … flow_<t> …
  inc_<stream>`), one shared writer
  across `simulate`/`pfilter`/`fit`, a version header and manifest, and an
  optional posterior-band summary. Crucially it adds **`inc_<stream>` projection
  columns** so the analyst reads model-predicted incidence directly instead of
  finite-differencing compartment counts (unsafe under events/balance) — that is
  the glob→concat→melt→conservation-audit ritual, designed away. Draft v1, ~3–4
  eng-days, no schema change; 7 open questions pending the maintainer.

- **Posterior-integrated forecast scoring →
  `2026-05-31-prequential-bayesian-lfo.md`.** The existing prequential is
  plug-in (point-estimate predictive, `Provenance::PlugIn` only). This carries
  the deferred Part II: fully Bayesian leave-future-out via PSIS, the k̂
  tail-reliability gate, randomized PIT for count data, and the rolling-origin
  k-step-ahead (forecast-horizon) variant. Design-only.

- **Trajectory coherence + typing → gh#267.** The typed directionality assertion
  (source-only compartments non-increasing) is "natural to land with the
  trajectory type redesign" — i.e. inside the consolidation proposal's adapter.
  Item 2 (inference-path SBC) is independent verification work, not output.

- **Prequential `y_obs` correctness → gh#268 (confirmed bug).**
  `--save-prequential` records `y_obs = 0` at every step and scores predictive
  samples against those zeros — silent garbage `elpd`/`CRPS`/`PIT`, inherited by
  `camdl compare`. **Code-confirmed:** `pfilter.rs:259-266` builds the
  prequential time axis as `Observation { time, value: 0.0 }` (value hardcoded),
  and `pfilter.rs:764` reads `.value` from it for the trace (the
  `--trace
  observed` column at `:672`/`:679` shares the defect); the real
  observed values live in `per_stream_cells` and never reach `build_trace`. This
  is a **regression from PR #218** (`4fbc67ab`, sparse/multi-cadence union
  axis): before it, `observations` carried `per_stream_obs[0]`'s real value, so
  the bug is **not multi-stream-only — single-stream is also broken**. The fix
  is narrow: a `MultiStreamObsModel::joint_observed()` accessor that sums the
  bound per-stream values onto the union axis (mirroring the NaN-filtered joint
  predictive sum), swapped in at the two read-sites. It **cannot change any
  loglik** — the filter loglik comes from `swarm.log_weights`, independent of
  the read-only prequential recording.

- **Per-stream prequential output → gh#269.** Today `--save-prequential` emits a
  single joint scalar per step on a multi-stream model. For a metapopulation the
  **per-stratum** predictive (per-district observed-vs-predicted, PIT, coverage,
  CRPS) is the scientific artifact, and it is only obtainable inside the filter,
  where the per-stream predictive density is computed before being summed. This
  is **strictly more than the gh#268 fix, not the same change** (the earlier
  framing that one fix satisfies both was wrong): the per-stream signal is
  collapsed by the `.sum()` at `particle_filter.rs:367-370` and by the scalar
  fields of `PrequentialStep`/`PrequentialTrace`. The fix is to _stop
  collapsing_ — extend the **existing** per-stream seam (`score_streams` /
  `at_union` / the per-stream `Vec` already returned by `sample()`/`mean()` in
  `multi_stream_obs.rs:986`/`:1148`) up through the recorder and the trace
  schema, keeping the joint score as the summary. Because `camdl compare`
  deserializes `PrequentialTrace`, this is a `compare`-visible schema change
  (bump `schema_version`; new fields default-empty so v1 JSON still reads).

A note on diagnosis: an analyst seeing this reported the bootstrap PF as
"degenerate on 14 streams, prequential unusable." gh#268 shows the filter is in
fact fine — same model, real vs zeroed observations give `loglik = -1822.98` vs
`-482.09`, so the filter responds strongly to data, and the saved predictive
samples re-scored against the real onsets give 91% coverage. The "degeneracy" is
the recording bug producing plausible-but-wrong scores. Fixing gh#268 likely
makes plug-in prequential usable on the spatial model immediately, independent
of the Bayesian Part II.

## New surface A — posterior-predictive emission as a first-class artifact

The capability exists; promote it to the happy path and compose it with the
trajectory writer rather than forking a parallel one.

**A1. `--draws posterior --fit <fit-id-or-path>` (independent, ship now).**
Resolve `posterior` to the fit's `draws.tsv` (the column-per-param format
`--draws PATH` already reads back round-trip), with `-n N` subsampling the first
N (or a thinned N) draws.

```
camdl simulate model.camdl --backend ode --draws posterior --fit <fit> -n 200 \
    --obs postpred.tsv
```

This is a fourth branch in the `--draws` dispatch (`main.rs:897-947`, alongside
`uniform`/`prior`/file-path) that resolves the path and falls into the existing
file-path loader — no new simulation or sampling code. There is a precedent for
the resolver: `fit --init from_posterior --posterior <path>` already does
`PosteriorSource::DrawsTsv` (`args/mod.rs`). A1 has no dependency on the
trajectory writer and is shippable on its own.

**A2. Co-emit `y_rep` and `latent_incidence` in one long frame — gated on a seam
decision.** The natural home is the consolidation proposal's
`write_trajectories_tsv`, which already plans an `inc_<stream>` column (the
projected/expected observable, i.e. `latent_incidence`); add a sibling
**`y_<stream>` sampled column** (the noisy draw from `sample_obs_resolved`).
Done this way, the smoother-vs-predictive confusion becomes unrepresentable: a
conditioned latent path (`fit` output) and a posterior predictive
(`simulate --draws posterior`) are different files with different column sets
and a `method` header, not two numpy arrays of the same shape.

But there is a seam hazard to resolve **first**, or A2 becomes the fork it
claims to prevent. `simulate --obs` today runs a _fully parallel_ obs-output
path — `compile_obs_sample_pf` looped per stream (`main.rs:1719-1743`), its own
`ObsRow`, its own **wide** writer `write_obs_output` (`main.rs:1753-1845`) —
which shares only the `sample_obs_resolved` leaf with the inference path, not
the projection/scheduling/hole-handling/assembly. Adding `y_<stream>` to the new
**long** consolidation writer while the wide `simulate --obs` path stays would
create a _third_ obs-output format. So A2 must be gated on an explicit decision
(shared with the consolidation proposal's own open question on routing
`simulate`'s posterior-predictive draws through the unified writer): does
`simulate --draws posterior --obs` emit through `write_trajectories_tsv`,
retiring/unifying the wide path — or is the wide path a deliberate second format
(which then needs the one-sentence justification the "reach for the existing
seam" rule demands)? A2 depends on the consolidation writer (Cluster B) landing.

Note the headline payoff — "the wrong objects cannot be confused" — is delivered
by **B + A2**, not by A1 alone: the labelling (`method` header, distinct
columns) lives in the consolidation writer. Approving A1 buys the ergonomic
one-liner; it does not yet buy the anti-confusion property.

## New surface B — held-out evaluation harness

`camdl data split --holdout` (temporal) exists; the fit-time `holdout_after` /
`[data.holdout]` config is parsed and CAS-digested but **not yet applied to
filter observations**. There is no `--holdout-strata`. For a spatial model the
natural model-criticism test is leave-district-out.

**B1. Activate temporal holdout (near-term wiring).** `holdout_after` is parsed,
validated, and CAS-digested but **never consumed** to withhold observations
(`holdout` appears only in `config_v2.rs` parse/validate and `fit/cas.rs`
digest, never in the runner/methods/pgas/pmmh). A config field that silently
does nothing is its own silent-gap bug. Make `holdout_after` actually withhold
observations at `t > threshold` during the fit, then score the predictive on the
withheld tail. This is pure wiring, shippable ahead of the research tail, and is
the temporal cousin of B2.

**B2. `fit --holdout-strata <names>`.** Mask the named strata's observation
streams during the fit (their data binding is withheld, the streams become
unobserved), fit on the remainder, then score the predictive on the held-out
streams.

```
camdl fit run fit.toml --holdout-strata Kambia,Port_Loko
```

The honest statistical content, stated in the output: holding out a whole
stratum is **not** one-step-ahead temporal scoring — it scores a _marginal_
predictive for a never-observed unit, driven entirely by the spatial coupling
from observed units. That is exactly the test that interrogates whether the
coupling kernel carries signal, but it is a different (and harder) predictive
object than the temporal tail, and the report must label it as such. B2 composes
the per-stream predictive scoring (gh#269) with a masking front-end; it is
downstream of gh#268/269.

## Execution sequence (to land all of this on main)

Ordered by dependency and by silent-bug priority. gh#268 and gh#269 are split
because they are different change classes (correctness fix vs `compare`-visible
schema change), not "one fix."

1. **A0 — gh#268 prequential `y_obs` correctness (ship now).** Add
   `MultiStreamObsModel::joint_observed()`, swap the two read-sites
   (`pfilter.rs:764` and the `--trace observed` column at `:672`/`:679`), leave
   the holes-reject guard (`check_holes_output_compat`) intact. TDD: a
   **single-stream** _and_ a **≥2-stream** red test asserting `y_obs` equals the
   bound data (both fail today against the hardcoded `0.0`), then green; plus a
   `joint_observed()` unit test. **Highest priority — silent wrong answers
   `camdl compare` inherits; touches no loglik.**

2. **A1 — gh#269 per-stream prequential output.** Extend the existing per-stream
   seam up through the recorder and the trace schema (per-stream `y_obs`,
   samples, `log_score`, `crps`, `pit`; joint stays as the summary; `stream`
   column in `{STEM}.tsv`, per-stream block in `{STEM}.json`). Guard the
   per-stream/joint relationship with a `sum(per_stream) == joint` test. Built
   on A0. **Format-lock-in flag:** this per-stream `prequential.json` shape is
   also read by `camdl compare` and will be _extended_ by the deferred LFO work
   (rolling-origin adds a per-horizon axis) — design the per-stream axis so a
   later per-horizon axis composes rather than re-keys the artifact.

3. **B — latent-trajectory consolidation (P-traj + gh#267 item 1).** The tidy
   long trajectory writer, the `inc_<stream>` columns, the shared
   `--save-paths
   N`, and the typed directionality assertion landing with the
   adapter. Needs the maintainer's call on P-traj's 7 open questions first.

4. **C — posterior-predictive emitter (new surface A).** **A1-resolver** (the
   `--draws posterior --fit` branch) is independent of B and can ship alongside
   A0/gh#269. **A2** (the `y_<stream>` column) rides on B and is gated on the
   parallel-obs-path decision above.

5. **D — held-out evaluation + Bayesian Part II.** Split by risk tier: **B1**
   (activate temporal holdout — near-term wiring of a parsed-but-inert config
   field) ahead of **B2** (`--holdout-strata`, on top of gh#269) and
   posterior-integrated LFO scoring (research tail).

A0 is immediately actionable and is the urgent fix. gh#269 + the A1 resolver
remove most of the analyst's bespoke glue; B unblocks the conditioned-trajectory
ritual. A2, B2, and the LFO work are the spatial model-criticism layer proper.

## Open questions for the maintainer

1. **CLI naming for posterior predictive.** `simulate --draws posterior --fit`
   (reuses the simulate ensemble path) vs a dedicated `camdl postpred` verb. The
   former is less surface and composes with existing `--scenario`/`--backend`;
   recommend it.
2. **`y_<stream>` vs `inc_<stream>` in one file vs two.** Co-emit both columns
   in the consolidation writer (recommended — keeps expected and replicate
   adjacent), or keep `simulate --obs` separate and document the join?
3. **Holdout-strata reporting.** Does the marginal-predictive caveat live in the
   output header, a manifest field, or both?
4. **Sequencing A2 vs B.** Ship `y_<stream>` as part of the consolidation writer
   (one writer, one PR), or land A1 standalone now and A2 after B?
