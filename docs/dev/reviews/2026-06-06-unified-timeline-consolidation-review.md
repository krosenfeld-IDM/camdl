# Unified-timeline + substep-time consolidation — code review

Date: 2026-06-06
Branch: `feature/unified-timeline` (`6686f1a`..`824fca4`, vs `main` `9a4be52`)
Scope: the work toward
[`2026-06-05-unified-timeline-effect-architecture.md`](../proposals/2026-06-05-unified-timeline-effect-architecture.md)
and
[`2026-06-05-substep-time-sdt-convention.md`](../proposals/2026-06-05-substep-time-sdt-convention.md),
ahead of
[`2026-06-05-observation-data-binding.md`](../proposals/2026-06-05-observation-data-binding.md).

The companion design map is
[`2026-06-06-scheduling-effect-topology.md`](../proposals/2026-06-06-scheduling-effect-topology.md);
this document is the audit of what the branch does *today*.

## Verdict

The **spine extraction is sound and the byte-identical claim holds where it
matters.** Every stepping loop — all four forward backends and all four filters
— now routes timing through one `Schedule` (`sim/src/schedule.rs`); the
floating-point robustness fix (`dt.min(boundary − t)`, never `(t+dt) − t`) is
real and correctly motivated by the continuous PGAS density; and the eight
`s·dt` density/gradient reconstruction sites the substep-time proposal worried
about genuinely now read `rec.t0` / `rec.dt_substep`, each guarded by a
contiguity `debug_assert`. This is the bug-surface win the proposals promised,
and it lands without disturbing the delicate reference-trajectory path (PGAS
stays `Snap` by default).

Two qualifications, one structural and one a gap:

- The landed `Schedule` is **far thinner than the proposal's sketched
  `Boundary` / `Trigger` / `Stage` / `Effect` / `Observe` / `Mutate` /
  `Constrain` / `ResetWindow` / `EffectCaps` type system + driver trio** — what
  shipped is a boundary cursor over parallel `output` / `effect` / `obs` time
  vectors. This is the *right* seam (consolidate the substrate, not the
  algorithms) and it actively helps the obs-data refactor. The proposal
  over-specified types; the code right-sized. Do not "fix" this.

- The proposal's **centerpiece — one canonical substep lifecycle, "the analogue
  of SLiM's published tick cycle" — is not enforced.** Two of four forward
  backends still run the opposite within-substep effect order, there is no
  cross-backend *agreement* test, and the lifecycle was never written to
  user-facing docs. The byte-identical Stage-1 timing extraction succeeded; the
  Stage-2 lifecycle canonicalization did not happen, and the Stage-2 work that
  *did* land (the obs-alignment gate) is validation-only.

## What is verified sound (read first-hand, not inferred)

- **Schedule routing is total.** `Schedule::new` is constructed in every
  backend entry (`chain_binomial.rs:159`, `tau_leap.rs:90`, `ode.rs:176`,
  `gillespie.rs:121`) and every filter (`particle_filter.rs:153`, `if2.rs:242`,
  `correlated_pf.rs:209`, `pgas.rs:350`/grid at `:1473`). No stepping loop
  hand-rolls a `t += dt` boundary walk anymore; the inner filter walk is the one
  shared `Schedule::substeps` iterator.

- **The FP-robustness fix is correct and the reasoning is right.**
  `Schedule::substep` returns `dt.min(boundary − t)` directly (`schedule.rs:162`);
  the `substep_is_bit_exact_dt_min_not_t_to_minus_t` test pins it at
  `t = 1095.7275, dt = 0.1` (`schedule.rs:439`). Integer draws are insensitive
  to the ULP; the continuous PGAS density (`shape = dt/σ²`) is not — exactly the
  motivation in the fp-robustness note.

- **The PGAS density/gradient consumers read the realized record, never
  recompute `s·dt`.** `complete_data_loglik` (`pgas.rs:693-694`), `csmc_as`
  producer (`pgas.rs:1021-1022`), traceback (`pgas.rs:1245-1247`),
  `complete_data_loglik_grad` (`pgas_grad.rs:410-411`) all bind
  `let t = rec.t0; let dt_s = rec.dt_substep;`. The two remaining
  `t_start + s·dt` sites (`pgas.rs:344`, `:831`) are uniform-grid *builders*
  (numerically identical to `substep_time`), not density reconstructions.

- **One grid, no producer/consumer mismatch.** `run_pgas` builds the grid once
  (`pgas.rs:1473`) and seeds the reference on that same grid
  (`simulate_reference_on_grid(&grid.steps)`, `:1504`), so the reference, the
  CSMC free particles, and the density all tile against one source of truth.

- **The `(algorithm × obs-alignment)` gate is clean** (`fit/methods.rs:350`,
  `resolve_obs_alignment`): every unsupported combination is a loud error
  (if2/pfilter snap → error; PGAS exact → not-implemented; correlated-PMMH +
  off-grid → error), wired into dispatch via `obs_on_grid`
  (`fit/runner.rs:335-338`, robust `t_start + k·dt` form, 1e-9 tol). It
  *validates* today; the threading into Schedule policy is deferred (PGAS
  `step_policy` hard-pinned to `Snap`, `fit/pgas.rs`).

- **Gillespie honours its special obligation.** After an intervention it does a
  *full* propensity recompute (`gillespie.rs:192-194`, `:232-234`) so a fresh
  exponential is drawn next iteration — the §2.3.1 SSA requirement, correctly
  implemented.

- **The exact-PGAS machinery is honest about its incompleteness.** It is gated
  behind `step_policy` (default `Snap`, byte-identical) and hard-rejects
  always-active-event models under `Exact` (`pgas.rs:1458-1467`).

## Findings — Major (correctness; before calling this "consolidated")

### M1 — coincident effect-order diverges across backends; no agreement test

Verified by direct read. The within-substep order is *opposite* on the two
backend families, and the event also reads a *different state*:

- **chain_binomial:** event delta computed from the **start-of-step snapshot**
  and applied **atomically with transitions** (`inject_event_deltas`,
  `chain_binomial.rs:445-449` → applied at `:451-454`), *then* interventions
  (`:510-520`), *then* balance (`:522-538`). Order: `event(snapshot) →
  intervene → balance`.
- **tau_leap / ode:** `apply_interventions_at` **then** `apply_events_at`
  (`tau_leap.rs:128,130`; `ode.rs:227,229` and `:266,268`), and `apply_events_at`
  computes the event delta from the **post-intervention current state**
  (`intervention.rs:229-235`). Order: `intervene → event(post-intervention)`.

For a coincident event + intervention these give different results *and* the
event reads a different state. The proposal flagged this (§"Coincident-boundary
order is non-canonical…") and assigned the tau/ode rewrite + a hand-computed
agreement fixture to Stage 2. Neither landed. Worse,
`gate_corner_case_baseline.rs` runs `all_lifecycle` per-backend with *separate*
expected hashes — it **pins the divergence as blessed** rather than flagging it,
which is precisely the "re-baseline is not self-validating" trap the proposal
named. Pre-existing, not a regression, but it is *the* cross-backend
disagreement the unification was sold to eliminate.

Action: do the Stage-2 canonicalization (rewrite tau/ode to event-fused-first,
re-baseline) **plus** a hand-computed cross-backend *agreement* fixture; or
defer explicitly and add a test that asserts the *known* divergence so a future
reader can't read the blessed hashes as agreement. (Maintainer wants the former
— exact consolidation. See the design doc for the canonical order.)

### M2 — the runtime sub-`dt` collision guard the proposal mandated does not exist

The proposal demands it twice (lines 481-482, 798-799): "feed two *distinct*
sub-`dt` obs times and assert the **runtime** hard error, not a generator
constraint." It is missing. The only obs-time guard,
`validate_obs_times_increasing` (`multi_stream_obs.rs:269-282`, the gh#188 work),
rejects only `t[i+1] <= t[i]` — equal or out-of-order. Two **distinct,
strictly-increasing, sub-`dt`-separated** times (obs at 3.0 and 3.4, dt=1) pass
it, then collide in `build_obs_at_substep` (`pgas.rs:280-292`): both round to the
same substep, `map.insert` is last-wins, and **one observation silently drops
from the PGAS likelihood** → wrong posterior. The Exact `build_substep_grid` arm
has no collision detection either. This is a silent-wrong-answer hazard on a
correctness surface, and it is the obs-data proposal's `build_obs_at_substep`
finding. Add the runtime guard, red-first, before merge.

## Findings — Major (proposal-vs-code drift; reconcile before sharpening obs-data)

The unified-timeline proposal lists several things *as its own deliverables*
that did not land. obs-data must not assume them; the proposals need a status
pass or obs-data will be sharpened against fiction.

### M3 — `ResetWindow` / per-stream accumulator reset: not built

No `Effect` / `ResetWindow` types exist; the reset is still **global**
(`particle_filter.rs:401`). The unified-timeline proposal lists `ResetWindow`
among "the concrete types"; the obs-data proposal says the per-stream reset
"re-homes here." It did not. **obs-data still owns the §5.2.1 per-stream-reset
work in full.**

### M4 — "consolidate the two scattered capability gates into one seam" is unmet — and shouldn't be met

`resolve_obs_alignment` (`fit/methods.rs:350`, algorithm × alignment) is a clean
*new* gate, but `check_model_capabilities` (`fit/methods.rs:402`, model ×
backend) and the forward gate (`util.rs:1699`) remain separate. These are
genuinely different axes; keeping them separate is the honest seam. The proposal
text claiming "one seam" should be corrected to "three gates, three axes" — this
is a doc fix, not a code fix.

### M5 — the canonical substep-lifecycle doc/figure never shipped

The proposal made it a Stage-0 deliverable ("the contract everything else
refactors against," with a figure in the language spec / user-features). `rg`
over `camdl-language-spec.md`, `user-features.md`, `dsl-cheatsheet.md` →
nothing. Given M1 (the contract isn't even *followed* by tau/ode), documenting
and enforcing it are now coupled work.

### M6 — correlated-PF defense-in-depth is half-done

The proposal wanted the silent CPM decorrelation fixed two ways: (1) the gate
keeps off-grid correlated-PMMH out, (2) the `if noise_idx < len` guard becomes a
hard error. Only (1) landed — off-grid correlated-PMMH is rejected upstream
(`resolve_obs_alignment` + the uniform-spacing check, `correlated_pf.rs:188-199`),
so the `if noise_idx < gamma_row.len()` fallback (`correlated_pf.rs:333`) is
unreachable *in valid runs*. But it remains a silent `if`, not a
`debug_assert!`/hard error: if the upstream gate is ever bypassed it silently
reads the wrong particle's noise. Cheap to harden. (Related latent edge,
pre-existing: `steps_per_obs` is sized from `obs(1) − obs(0)` but the first
window is `[t_start, obs(0)]`; if `obs(0) − t_start ≠ obs_dt` the first window's
substep count differs and `noise_idx` mis-indexes. The uniform-spacing check
does not cover the first offset. CPM is experimental/gated; worth a note.)

## Findings — Minor / smells

- **m1 — dead `grid` field + `grid()` getter** (`schedule.rs:86,238`). Zero
  callers. It carries the proposal's "one snap grid is the single source of
  truth" intent, but `substep` uses `self.dt` and interventions still snap via
  each backend's own `resolve_fire_steps(cfg.dt)` / gillespie's
  `iv_resolution_dt = unwrap_or(1.0)`. The promised seal is unimplemented; the
  field is aspirational dead code. Wire it (kill `unwrap_or(1.0)`) or delete it.

- **m2 — dead effect-cursor bookkeeping in chain_binomial**
  (`chain_binomial.rs:246-248`). `cursor.effect_idx` is advanced but never
  *read* in the Snap path (`substep` ignores effects). The
  `all_intervention_times(...)` populating `effect_times` (`:165`) and this
  advance loop are wasted work. Removable.

- **m3 — dead `_tolerance` parameter** (`intervention.rs:84`).
  `apply_interventions_at` ignores it; firing keys purely on
  `time_to_step(t, dt)` (`:95-99`). Every backend passes a tolerance
  (`1e-10` for tau/ode/gillespie, `dt*0.5` for chain_binomial) that does
  nothing. Drop the parameter, or make firing actually use a tolerance (it
  currently does not — firing is rounded-step-index containment everywhere).

- **m4 — two hand-rolled `t_start + s·dt` builders bypass `substep_time`**
  (`pgas.rs:344`, `:831`). Bit-identical, so not a bug, but weakens the
  single-source-of-truth claim. Route through `Schedule::substep_time`.

- **m5 — CRN test weaker than its label** (`schedule.rs:530-540`).
  `n_cursors_identical_sequence` asserts `walk(s) == walk(s)` from fresh default
  cursors — it proves `walk` is deterministic, not that N cursors *at different
  positions* don't alias shared state. The CRN property is actually guaranteed
  by the types (`Cursor: Copy`, `Schedule` immutable). Strengthen it to
  interleave several cursors advanced to different indices against one reference
  walk, or downgrade the comment's claim. (Its sibling
  `substeps_iterator_matches_the_manual_filter_walk` is genuinely solid.)

- **m6 — no `--obs-alignment` CLI flag.** Exposed only via
  `fit.toml [backend].obs_alignment` (`config_v2.rs`). Violates the
  "every behaviour expressible as a CLI flag; fit.toml bundles, doesn't gate"
  principle. It is validation-only today, so the flag is cheap and safe to add.

- **m7 — ODE interventions take a lossy f64→i64→f64 round-trip**
  (`ode.rs:226,265` `to_states`; back at `:230,269`). The `Action` path
  (`apply_intervention`, `intervention.rs:257`) prefers `global_to_int`, so an
  ODE model's count compartments — held as `f64` for smooth integration — are
  *rounded to integers* at every intervention firing, the action applied on
  integers, then cast back. For `Set`/`Add`/`AbsoluteTransfer` this quantizes
  continuous state. Root cause: the `Action` ADT is integer-typed; ODE is the
  one f64 backend. This is the deepest abstraction break (see the design doc);
  flag, don't fix here.

- **m8 — gamma-density value at a *genuinely shortened* substep is untested**
  (Stage-3 gate, not a merge blocker). `pgas_exact_tiling.rs`'s shortened-substep
  *value* recompute uses a non-overdispersed model; `gradient_check_overdisp.rs`
  exercises the gamma gradient only at *uniform* non-unit dt (0.9125, 0.5) where
  `dt_substep == dt` everywhere. The most `dt_substep`-sensitive density term
  (`shape = dt_substep/σ²`) is never fed a `dt_substep` that differs from its
  neighbours. Must be closed before exact-PGAS becomes default.

## obs-data readiness (the downstream concern)

- **Ready:** the scoring seam (`MultiStreamObsModel::log_likelihood`, gh#139, all
  four algorithms) and obs-times-as-boundaries (`Schedule::with_obs`). The thin
  schedule extends cleanly to obs-data's union axis (`obs_times` is already a
  parallel vector). The conservative type scope *helps* here — a heavy
  `Effect`/`Stage` ADT would have pre-shaped obs-data's `Observe`/`TemporalKind`.

- **Not done; obs-data must build:** per-stream `Interval` accumulator reset
  (M3) and the runtime collision guard (M2). The obs-data proposal currently
  claims both "re-home here"; correct that before sharpening it.

## Things I looked for and found OK

- Density sites all read `rec.t0`/`rec.dt_substep` (no `s·dt` leak into the
  density). Verified by grep + read.
- `run_pgas` reference seeded on the same grid CSMC tiles against. No mismatch.
- Gillespie post-intervention propensity recompute is present and correct.
- The off-grid corpus (`tests/fixtures/corner_cases/`) is genuinely off-grid;
  the inference baseline pins an off-grid loglik (`sir_incidence_offgrid`).
- exact-PGAS is gated and hard-errors on the cases it cannot yet handle.

## Recommended landing sequence

1. **M2** collision guard (red-first) + **M1** decision (canonicalize tau/ode,
   or assert the known divergence) — the correctness gates.
2. **M3–M6** reconciliation: correct the proposal claims (ResetWindow,
   gate-consolidation, defense-in-depth) and ship the lifecycle doc/figure
   (**M5**), driven by the design map.
3. Minor cleanup: dead `grid` (m1), dead effect-cursor (m2), dead `_tolerance`
   (m3), route the two builders through `substep_time` (m4), strengthen the CRN
   test (m5), add `--obs-alignment` (m6).
4. *Then* re-sharpen obs-data against the now-accurate surface.

The cross-cutting recommendation: add at least one **cross-backend agreement
invariant** (two backends, same result on a coincident-effect model where they
legitimately should agree). Today every gate pins each backend independently, so
a divergence gets blessed rather than caught — which undercuts the "one surface
→ fewer bugs" thesis the whole effort rests on.
