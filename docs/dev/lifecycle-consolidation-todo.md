# Lifecycle / effect / timeline consolidation — tiered TODO

Living tracker for the simulation-engine consolidation. Design map:
[`proposals/2026-06-06-scheduling-effect-topology.md`](proposals/2026-06-06-scheduling-effect-topology.md)
(v1; a v2 superseding proposal lands at Tier 3). External review:
[`reviews/2026-06-07-lifecycle-design-review.md`](reviews/2026-06-07-lifecycle-design-review.md).

Tiers are ordered: **finish a tier before the next**, except where a Tier-1 item
blocks a later one (noted). `[ ]` todo · `[~]` in progress · `[x]` done.

## Decisions locked (do not relitigate)

- **Lifecycle shape:** a closure-taking `fixed_step_substep` driver owns the
  canonical order; the per-backend kernel is a closure. **No `FixedStepLifecycle`
  trait** (Gillespie can't honor it; the only shared content is the order). Remove
  the `// → FixedStepLifecycle` markers. Gillespie uses a separate
  `apply_boundary_effects` helper.
- **i64/f64 seam:** ~~`CountStoreMut` enum-with-rounding-methods~~ **SUPERSEDED** by the
  effect-purity seam (`8b9111d`…`01f92f1`, Tier 2 below). Two adversarial reviews found
  `CountStoreMut` conflated two orthogonal axes (representation × purity); the shipped
  design splits them: a pure `resolve_*(StateRef) → typed Int/Real deltas` (all rounding
  here) + a trivial `apply_effects` (no branch). Representation rides the delta type, not
  an enum. ODE uses a continuous f64 effect path; discrete stays byte-identical.
- **tau-leap:** ~~drop via extracting ONE shared kernel + a 14-case equivalence gate~~
  **DONE as a pure delete** (D3 below). The kernel-extraction/equivalence approach was
  superseded: tau and chain already share `step_one`, and at alpha (backward-compat a
  non-goal) no equivalence is owed. Deleted `TauLeapSim` + its golden rows — byte-identical
  for the other backends; the Exact policy survives in the inference filters. The
  `RATE_EPSILON`/`cfg.dt` divergences were tau's (arguably bugs), now gone with it.
- **Timeline vocabulary (going forward):** `substep`/`interval` = `[t0,t1]`;
  `TimelineStop` = `t1`; `StopReason` = why it matters (`Output|ScheduledEffect|
  Observation|End`); `scheduled effect` = the action due there. New types use this;
  existing "boundary" prose stays (no churn).
- **Capability-matrix honesty:** three forward kernels (chain / ODE / Gillespie);
  **inference stays chain-binomial-centred.** Do NOT make ODE/Gillespie inference
  kernels or pretend all backends share one cadence. The `ProcessModel` /
  `DensityProcess` capability split is correct; keep it. No fake genericity until a
  real second inference backend demands it.

## Invariants every change must preserve (the silent-wrong-answer guardrails)

- **RNG draw order / paired-seed CRN** — any reorder of draws breaks enable/disable
  byte-identity and the PGAS density. Firing-path or fusion reshapes need a
  byte-identical A/B gate.
- **PGAS complete-data density / gradient** — `shape = dt/σ²` is dt-sensitive; the
  producer draws in source-group order with `scratch.gamma_used` recorded.
- **i64 byte-identity** — the discrete backends must stay byte-identical through the
  f64-seam rework; only ODE's numbers may move (and must be *verified* correct, not
  re-blessed).
- **Golden gates** — `gate_trajectory_baseline`, `gate_corner_case_baseline`,
  `gate_pgas_density_baseline`, `gate_inference_baseline`.

---

## Tier 0 — landed ✅

- [x] **M2** `6c80e62` — sub-`dt` obs-collision guard (silent likelihood drop).
- [x] **M1** `ec4e7d1` — canonical event→intervention order in tau/ode/gillespie.
- [x] **M6** `370f8e2` — CPM non-uniform-window guard + hard-error noise overrun.
- [x] **2a** `9988415` — shared `lifecycle.rs` seam + tau event read-source fusion.

## Tier 1 — critical correctness bugs (NOW; red-first; some block Tier 2/3)

- [x] **#3 — chain-binomial stale real-state** *(BLOCKS the tau fold)* — `5c7585c`
  (+ fixed a second bug: real-compartment interventions were dropped). Still blocks
  the tau fold until the chain≡tau proof includes real-coupled models.
  `step_one` evaluates propensities/events against `scratch.real_s`, never synced
  from the run's RK4-advanced `real_s` → integer transition rates that couple to a
  real (ODE-style) compartment use a stale (zero) value. Hits chain-binomial forward
  **and inference** (the only inference kernel). `cholera_siwr` (W→infection) is the
  in-tree reproduction. → incident report + red-first real-coupling fixture + fix
  (sync `real_s` into `step_one`) + **verify** the corrected `cholera_siwr` golden
  (cross-check vs ODE/gillespie, don't re-bless) + **scope whether inference advances
  `real_s` at all** (may be a bigger gap). Fix on this branch (least friction).
- [x] **D-finite + D-negative** — `6b07627`. `finite_action_value` rejects non-finite
  resolved values at both effect sites (events + scheduled) before the cast; centralized
  post-INTERVENE/BALANCE negative scan in `apply_post_advance` (covers all 4 backends —
  they all route INTERVENE through it) with new `InterventionNegative` cause (hard
  error). Fixed the `Add` error's hardcoded `t:0.0`. Deduped the error-path name lookup
  into `CompiledModel::int_compartment_name`. Red→green proof in the commit; key finding:
  pre-guard a mid-run `set`-negative is mis-caught one substep later as the *recoverable*
  `BinomialOvershoot` (silently swallowed in inference), at the final step not at all.
  sim suite 541/0, no golden moves.
- [x] **#1-interim — PF/IF2/correlated-PF event-misfire guard** — `e90c217`.
  `Schedule::reject_event_misfire(has_events, t_start)` + `ProcessModel::
  has_always_active_events()` (required, no default), wired into all three
  bootstrap filters. Scoped tighter than PGAS's blanket Exact+events refusal:
  these filters use Exact *unconditionally*, so the trigger is an OFF-grid obs
  (or t_start) — on-grid importation/seeding still fits (over-rejection control
  test). Red→green: pre-guard the PF returned a finite loglik (silent misfire);
  post-guard it errors naming the off-grid time. The full fix is `StepClock`
  (Tier 3).
- [x] **#3-inference-gate — reject real-coupled models in the fit path** *(surfaced
  by #3).* — `2684b0d` (gh#191). Dropped `REAL_COMPARTMENTS` from chain-binomial's
  *inference* capabilities (the flag was false in fact); fits of real-coupled models
  now hard-error at dispatch with the frozen-reservoir reason + a pointer to
  `backend = "ode"` / `camdl simulate`. A one-line re-grant lifts the gate once
  inference advances real state (the full fix — Tier 2). Red-first test
  `chain_binomial_inference_rejects_real_compartments` (chain rejects, ode accepts).

## Tier 2 — the consolidation (designed; resume after Tier 1)

- [x] **3a + 3.x — the effect-purity seam** (supersedes `CountStoreMut`) — `8b9111d`
  `d8a9259` `973cc2e` `01f92f1`. Proposal
  [`proposals/2026-06-07-effect-purity-seam.md`](proposals/2026-06-07-effect-purity-seam.md).
  Pure `resolve_*(StateRef) → typed Int/Real deltas` + trivial `apply_effects`; one home
  for the round/floor/clamp/arena arithmetic. Interventions + events route through it
  (dedup complete — `apply_intervention`/`inject_event_deltas`/`propose_event_deltas`
  deleted). Fixes the events-on-real silent drop (red→green on ODE + chain) and ODE's
  `to_states` quantization (continuous f64 effect path; 3 ODE corner-case baselines
  re-derived, verified by the continuous unit oracle). Discrete backends byte-identical;
  CRN / PGAS-density / gradient untouched. `CountStoreMut` not introduced — representation
  is carried by the delta type, not an enum-with-rounding-methods.
- [ ] **D1 — closure driver.** Extract `fixed_step_substep(state, .., advance)`;
  route chain/tau/ode through it; Gillespie via `apply_boundary_effects`. Remove the
  `// → FixedStepLifecycle` markers. Byte-identical.
- [x] **D3 — drop tau-leap** — `<SHA>` (scheduling-spine-v2 §D / Step 4). Done as a
  **pure delete**, not a kernel-extraction. tau-leap's niche (approximate
  fixed-step stochastic forward, multiple firings per substep against frozen
  rates) is already covered by chain-binomial on the production path — its
  `run_tau_leap` loop only ever *mirrored* chain's within-substep lifecycle
  (its own comment: "matches chain_binomial"), never shared code with it. No
  capability is lost: chain covers approximate-stochastic forward (incl.
  overdispersion), gillespie covers exact-stochastic forward, ode covers
  deterministic, and the Exact step *policy* survives in the inference filters
  independent of the tau backend. So no equivalence proof and no chain≡tau edge
  matrix were required (the bar is "the surviving backends are correct," not
  "chain+Exact == tau"). Deleted `TauLeapSim` / `TauLeapConfig` /
  `SimConfig::TauLeap` / the CLI `tau_leap` arm + the tau golden rows and
  tau-specific tests; the other backends' baselines are byte-identical.
- [ ] **Target=Parameter (NPI axis).** `Action` gains a `{Compartment|Parameter}`
  target via the same `CountStoreMut`/Action rework; option-2 (compile-error guard on
  forcing-consumed params; `gh#186` deferred).
- [ ] **Inference real-state support (the full fix for the #3-inference-gate).** Carry
  the real reservoir in `ParticleState` and RK4-advance it each substep in the filter
  loops (PF/IF2/PMMH/PGAS), so real-coupled models can actually be fit. CRN-sensitive
  (the real state joins the particle); PGAS density may need to account for it. Lifts
  the Tier-1 inference gate when done.

### tau-fold deletion gate (the 14 cases — obviated by the pure-delete decision)
This edge matrix was the gate for the *kernel-extraction* approach, which proved
chain+Exact ≡ tau before deleting tau. The v2 proposal (§D) replaced that with a pure
delete — no equivalence proof is owed (alpha; the bar is "chain+Exact is correct"). The
cases are retained as the rationale for why the fold was safe:
integer-only · off-grid interventions · always-active events reading the source
compartment · simultaneous event+intervention+output · overdispersed · deterministic
draws · competing exits · ungrouped inflows · tiny rates near `RATE_EPSILON` · models
whose expressions reference `dt` · real compartments coupled into rates · lineage
observer on · balance under Exact (support or reject) · inference stepping to off-grid obs.

## Tier 3 — timeline tightening (gated reshapes; **write the v2 proposal here**)

Each behind a byte-identical A/B gate (these touch CRN / PGAS-density invariants).

- [x] **v2 superseding proposal written** — `2db81ba`,
  [`proposals/2026-06-07-scheduling-spine-v2.md`](proposals/2026-06-07-scheduling-spine-v2.md).
  Two-dt model (`dt_actual` physics+eval vs `grid_dt` scheduling) via `StepClock`;
  `TimelineStop`/`StopReason` + `EffectBatch`; closure-driver; drop-tau (chosen-winners
  gate); forward `Target=Parameter`. Step-0 oracles first. Sub-agent reviewed; `EvalCtx.dt
  = dt_actual` settled (byte-identical w/ chain+PGAS). The items below are its sub-steps.
- [ ] **`StepClock` (#1 full).** Separate `dt_actual` / `schedule_dt` / `eval_dt`;
  decide `EvalCtx.dt` meaning explicitly. Removes the `dt`-overload class.
- [ ] **`TimelineStop` / `StopReason` (#2, A).** Schedule returns the next stop +
  its reasons; the driver handles reasons in one declared canonical order. Removes
  the zero-`dt` boundary loops and the `time-to-step` due-ness rediscovery in
  `apply_interventions_at` (effect application stops re-deciding due-ness — it
  applies a known batch).
- [ ] **Schedule owns `[t_start, t_end]` (B)** + reject pre-window boundaries.
- [ ] **`Substeps` obs-only type-safety (C)** — encode "no effect boundaries" in the
  type so the iterator can't yield zero-length steps forever.
- [ ] **`balance_negative = error|warn` (E)** — default error for forward/public;
  inference may downgrade to `-Inf`.

## Tier 4 — reactive interventions (the real stress test; `EffectAgenda` lands here)

- [ ] **`EffectAgenda` (#2/G)** replaces the static `all_intervention_times` vector;
  `due_effects(t, state, params) -> EffectBatch`.
- [ ] **`AgendaScope { SharedExogenous, ParameterDependent, ParticleLocal }`** — the
  capability classification. *Latent-state-reactive* (ParticleLocal) makes the agenda
  part of particle state (resampling clones it, PGAS ancestor-tracing accounts for it,
  CRN breaks) — must be explicit, never sneak in as "just another schedule."
- [ ] **Output/observation ordering convention (F)** — declared once; a separate
  `PostObservationReaction` stage for observation-triggered effects (not mixed with
  scheduled interventions). Default: an obs at `t` sees post-scheduled-effect state;
  reactive effects it triggers are enqueued *after* scoring.
- [ ] **The `Sense` stage + reactive gated out of PGAS** (per the topology proposal's
  closed-loop analysis + the reviewer's CRN point).

## Tier 5 — doc honesty + deferred

- [ ] **#5 — narrow Gillespie's "exact oracle" claim** in the capability surface /
  backend-rationalization note: exact only for autonomous integer CTMCs /
  piecewise-constant hazards, NOT for seasonal/real-coupled/scheduled-effect models
  (frozen-`lambda_total` SSA). The `PDMP thinning` TODO is the real fix.
- [ ] **Observation-data layer** — `bind`/`BoundObs`, per-stream `ResetWindow` (with
  multi-cadence so it's testable), `Counted` denom. (Separate proposal.)
- [ ] **exact-PGAS** — deferred, `gh#175`-gated.
- [ ] **minors** — dead `grid` field, dead `_tolerance` param, the `m1`–`m6` cleanups.
