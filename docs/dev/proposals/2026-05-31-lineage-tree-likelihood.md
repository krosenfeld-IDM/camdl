# Proposal: lineage tree likelihood and the native joint-tree-inference path

Date: 2026-05-31 Status: Proposal **Split from:**
`archive/post-alpha/2026-05-20-lineage-resampling-and-likelihood.md` (the
three-layer lineage RFC). Layers 1–2 — lineage resampling and tree _realization_
(count-level tracking, stratified attribution, the three backends, projections,
validation) — shipped. This proposal carries only the reserved Layer-3 remainder
so the live set describes open work; see the archived RFC §4c and §7 for the
full derivation and the structured- coalescent references.

**Motivation:** Layers 1–2 produce sampled trees but no _likelihood_ over them —
there is currently no observation channel that scores a realized genealogy
against the dynamical trajectory. That channel (and the native joint inference
it enables) is what turns lineage realization into a tool for
genomic/phylodynamic inference.

## Scope (the reserved Layer 3)

- **Sampled-tree likelihood.** Score a realized tree given the trajectory and
  per-deme lineage assignments. The exact marginal over latent genealogies is
  combinatorial; the forward-MC sampled-tree estimator is tractable only in
  restricted regimes (archived RFC §4c). Define the estimator, its
  bias/variance, and the regime where it is honest.
- **Structured-coalescent approximation.** The analytic route (Volz 2009;
  structured-coalescent approximation theory). Decide whether camdl offers it as
  the production likelihood and where the forward-MC estimator deviates, on a
  _specific dynamical measure_.
- **Coalescent observation channel.** Wire it as an observation channel
  (`coalescent_loglik(interval | trajectory, lineage_demes)` over a preprocessed
  `CoalescentTimeline`) that composes with the existing multi-stream observation
  surface — the RFC's Layer-2 interface was built so this plugs in without
  touching tree realization.
- **Native joint-tree-inference path** (RFC §7). The coalescent / birth-death
  joint inference over (parameters, tree) — reserved future work; scope the
  boundary so the PF cannot read ground truth.
- **[FUTURE] mutations + substitution model** — sequence evolution / within-host
  coalescent — noted in the RFC as a later channel; out of scope here beyond
  leaving room for it.

## Open design questions

- Forward-MC sampled-tree estimator vs structured-coalescent analytic
  approximation as the production path (tractability vs accuracy).
- Where the coalescent channel sits relative to `multi_stream_obs` and the
  unified-observation work (`2026-05-30-unified-observation-data.md`).
- The inference boundary: enforce that the likelihood path structurally cannot
  read ground-truth lineage assignments.

Design-only; nothing here is implemented.
