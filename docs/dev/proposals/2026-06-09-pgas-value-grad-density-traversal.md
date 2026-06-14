# RC1: one shared per-substep density skeleton in PGAS (value + grad)

- **Status:** **v3** — re-validated against `main` post-#218 (2026-06-14). All
  four drift sites (#197, #200, #3, #4) **FIXED** by narrow mirrored patches,
  each pinned by the spine oracle. Remaining: the structural single-skeleton
  HARDENING refactor (gh#79) — no behavior change. High-risk inference math
  (`CLAUDE.md`): `pgas.rs`/`pgas_grad.rs`/`chain_binomial.rs`.
- **Issues:** #197 (fixed), #200 (fixed), #79 (the remaining refactor); retires
  the #20/#76 one-off-patch cycle. RC1 in
  `docs/dev/reviews/2026-06-08-systemic-root-causes.md`.
- **Sequencing:** before #191's reservoir threading (§2.5 of
  `2026-06-09-real-compartments-inference-stack.md`) — both edit these density
  functions _and_ `step_one`.

## 0. v3 re-validation (2026-06-14) — corrected inventory + phasing

The code drifted since v2; re-verified against `main` (post-#218):

- **#197 — LIVE, reproduced → FIXED here (Phase 1).** The grad path added the
  gamma _gradient_ but never its _value_ to `complete_data_loglik_grad().0` (the
  NUTS energy), so on an overdispersed model the energy was low by `Σ log Γ`
  (40–84 nats across σ on `sir_overdispersion`), silently biasing the σ²
  posterior and diverging from MH / swap (both `complete_data_loglik().total`).
  **Phase-1 fix:** one shared
  `obs_loglik::gamma_multiplier_log_density(shape, scale, g)` helper; the value
  fn calls it (provable no-op — value goldens unchanged) and
  `gamma_density_value_and_grad_substep` adds each gamma value DIRECTLY into
  `log_p` (in the value fn's left-fold order, not pre-summed — `f64` add is
  non-associative, so a pre-summed `(g1+g2)` would differ by a ULP for
  multi-gamma substeps) and returns the gradient. Pinned by the **spine oracle**
  (`gradient_check_overdisp.rs`:
  `complete_data_loglik_grad(θ).0 ==
  complete_data_loglik(θ).total`,
  f64-exact, RED→GREEN) — covering BOTH single-gamma (`sir_overdispersion`) and
  multi-gamma (`sir_two_overdispersed`, 2 gammas/substep) so the summation-order
  path is guarded. This also resolves the #4 floor (the shared helper uses the
  value side's `g.max(LOG_PROB_FLOOR)`).
- **#200 — FIXED (this branch).** The grad's _ungrouped/source-less_ loop
  (`pgas_grad.rs::log_transition_density_grad`) now mirrors the value fn: a
  `Deterministic` transition gets the exact-count guard (no density term, no
  gradient) instead of `poisson_logpmf`. Pinned by a programmatic
  deterministic-inflow spine-oracle test (RED 34.8 nats → GREEN bit-exact).
- **#3 — FIXED (this branch).** The ungrouped grad loop now skips on
  `RATE_EPSILON` (was `0.0`), matching the value fn and `step_one`. (The grouped
  loop was already aligned by IM7/IM9.)
- **#4 — resolved by the Phase-1 helper** (was latent behind #197).

**All four drift sites are now closed** by narrow mirrored patches, each pinned
by the spine oracle. **Phase 2 (still open) is now a pure HARDENING refactor**
(gh#79): the §3/§4 single per-substep skeleton so value+grad drive two
accumulators off ONE branch — making the oracle hold _by construction_ rather
than by three mirrored loops that can re-drift. No behavior change (the sites
are already closed); the win is structural. NOTE for the implementer: §4's
anchors are stale — `log_gamma_density_substep` does not exist (the value/grad
gamma value now share `obs_loglik::gamma_multiplier_log_density`); the value
transition density lives in `compute_source_group_probs` +
`exit_and_split_log_density`; and the skeleton must thread #218's per-stream
`acc` lifecycle (`fold_into_acc` → score → `reset_due_acc`,
`n_interval_streams`) and the realized `(rec.t0, rec.dt_substep)`. The spine
oracle (value + det-inflow cases) already exists in `gradient_check_overdisp.rs`
as the by-construction gate.

## 1. What exists, and how it drifts

The per-substep transition+gamma density is computed in **three** places that
must stay in lockstep:

- `complete_data_loglik` (`pgas.rs`) — the **value** (`LogLikComponents`, with a
  transition/observation/ivp decomposition, `pgas.rs:187-196`).
- `complete_data_loglik_grad` (`pgas_grad.rs`) — returns `(log_p, grad)`. **Its
  `.0` is the energy NUTS integrates** (`pgas.rs:1956-1967`, `nuts.rs`).
- `step_one` (`chain_binomial.rs`) — the **producer** the two densities mirror
  (gamma draw/push order, the rate guards). RC1 unifies the two densities; it
  leaves `step_one` as the reference they must match (and #191's reservoir
  threads through `step_one` too — coordinate).

The binding contract — _every term in `grad` is in `log_p`, under the same
guard/iteration_ — lives only in a doc comment. `f64` is `f64`; the loops drift.
**Four** known divergences, all the same root:

1. **#197 (the shipped one):** `complete_data_loglik` adds the gamma value
   (`pgas.rs:804`); `complete_data_loglik_grad` adds the gamma **derivative**
   (`pgas_grad.rs:434`) but **never `log_p += gamma_value`** — so its returned
   `log_p` (`:460`), the NUTS energy, is _missing the term whose gradient it
   carries_. (The value fn is correct; the **grad-fn's log_p** is low.)
2. **#200:** a deterministic source-less transition is scored Poisson on the
   grad side, exact-count on the value side — the loops disagree on _which_
   term.
3. **Ungrouped rate skip:** value uses `rate <= RATE_EPSILON` (`pgas.rs:634`),
   grad uses `rate <= 0.0` (`pgas_grad.rs:226`).
4. **Gamma `g` floor:** value uses `g.max(LOG_PROB_FLOOR)`, grad uses
   `if g > 0.0`.

## 2. Why it causes problems

NUTS integrates Hamiltonian dynamics with **energy =
`complete_data_loglik_grad
(..).0`** and **force = its `grad`**; correctness
needs `force == ∂energy`. With the gamma value missing from the energy but
present in the force, the σ² posterior is **silently biased** — the chain mixes,
R̂/ESS look healthy, no error.

Worse, it makes the **samplers mutually inconsistent**: NUTS targets the energy
_without_ gamma (`grad.0`), while MH-within-Gibbs (`--no-nuts`) and the
replica-exchange swap both score with `complete_data_loglik().total` _with_
gamma (`pgas.rs:2112`, `:2168`). So today the same fit run targets _different
stationary distributions_ depending on `use_nuts`, and a swap mixes a
NUTS-updated rung (gamma-less energy) against a gamma-inclusive value. The fix
(below) makes all of them read one density → consistent.

It slipped because the FD gradient test **exists but is mis-fixtured**:
`gradient_check.rs::test_nuts_target_gradient_on_z_scale` FD-checks `grad`
against the grad-fn's own `.0` and _would_ catch #197 — but it runs on
`sir_basic` (`gradient_check.rs:262`), which has no overdispersion, so the gamma
term is never exercised.

## 3. The fix: share the per-substep skeleton; keep both public signatures

**Do not** collapse the return type behind a `want_grad: bool` (that just moves
the two drifting branches inside one function) and **do not** drop
`LogLikComponents` (the cold-rung output `pgas.rs:2178` and the targeted `-inf`
diagnostics `pgas.rs:1597/1706` read its transition/obs/ivp split — an
error-quality feature).

Instead, factor the **per-substep transition+gamma iteration skeleton** — the
source-group walk, the _single_ `gamma_idx` accounting, the _single_ rate guard
(`RATE_EPSILON`) and gamma floor — into one routine that, per term, drives **two
accumulators off the same branch**: a value accumulator and an (optional)
gradient accumulator. Both public functions are thin layers over it:

- `complete_data_loglik` → the value accumulator → `LogLikComponents`
  (decomposition preserved; layers IVP/obs on top of the shared transition+gamma
  primitive).
- `complete_data_loglik_grad` → both accumulators → `(log_p, grad)` where
  `log_p` is built from the **same** per-term values.

Because both `log_p`s now come from one per-term routine with one guard, the
four divergences close _by construction_, and the §5 exact oracle holds.
(Picking the guards: take the value side's `RATE_EPSILON` and
`g.max(LOG_PROB_FLOOR)` as canonical; pin the choice with a test.)

## 4. Implementation (for the implementing subagents)

1. **Extract
   `substep_transition_gamma(model, rec, params, t, dt,
   estimated_to_model)`**
   emitting per-source-group per-term contributions (value always; gradient via
   the existing `eval_resolved` chain when the caller wants it), under one
   guard + one `gamma_idx` step. This is gh#79's "one iterator."
2. **`log_gamma_density_substep` returns `(value, grad)`** (not grad-only); the
   skeleton adds both halves in the same `Some(overdispersion)` branch.
3. **Re-express the public fns over the skeleton.** `complete_data_loglik` sums
   the value side (keeping `LogLikComponents`); `complete_data_loglik_grad` sums
   both → `(log_p, grad)`. **The grad-fn's `log_p` now includes gamma** (the
   #197 fix). `complete_data_loglik`'s value is **unchanged** — there is **no
   value golden to re-bless**.
4. **Value-only callers stay on the value side** (no grad cost): MH-accept
   (`pgas.rs:2112`), swap-ll recompute (`:2168`), warmup (`:1914`), sanity
   (`:1591`), and the CSMC ancestor-weight uses of
   `log_transition_density_substep` (`:1207`). The bootstrap PF is **not** a
   consumer (it scores via `ObservationModel`, `particle_filter.rs:286-303`).
5. **Out of scope (note it):** `step_one`'s own gamma loop is the third copy;
   RC1 keeps it as the reference the densities mirror (don't fork it here — #191
   also edits it).

## 5. Tests (what makes it stick)

- **The spine oracle** —
  `complete_data_loglik_grad(θ).0 == complete_data_loglik
  (θ).total` for the
  same trajectory, **f64-exact** (exactness is a _consequence_ of both routing
  through the one skeleton — a mere gamma-patch into the grad fn's separate code
  would not guarantee it). RED on current code (grad-fn `log_p` low by gamma) →
  GREEN.
- **Re-fixture the existing FD test** — run
  `gradient_check.rs::test_nuts_target_gradient_on_z_scale` (or a sibling) on an
  **overdispersed** model so the gamma arm is FD-checked, plus a deterministic
  source-less transition (#200) and the `RATE_EPSILON`/`g`-floor edges (3,4).
- **The real observable — a posterior-recovery gate.** No committed gate pins a
  NUTS energy/posterior on an overdispersed model today (verified:
  `gate_inference_baseline` = PF marginal; `gate_pgas_density_baseline` =
  `seasonal_drift`, no gamma). **Add** a gate that fits an overdispersed model
  and asserts the σ² posterior recovers (it does not, today). This is the
  user-observable fix, not a golden re-bless.
- **#179 coverage meta-test** — every gradient arm has an FD check + the
  value-of-grad==value oracle.

## 6. Sequencing & ownership

RC1 before #191's density threading (same functions + `step_one`); retires
#197/#200/#79 and the #20/#76 cycle; reconciles the NUTS/MH/swap energy
inconsistency. Inference owner's lane — coordinate; do not fork these files in
parallel with active inference work.

## Changelog (v1 → v2, from the design review)

- **Direction fix:** the value fn already has gamma; the **grad-fn's `log_p`**
  (NUTS energy) is what's low. No value-golden re-bless; the observable is the
  posterior. (v1 §4.6 had this backwards.)
- **Seam:** per-substep transition+gamma skeleton with two accumulators, **not**
  a `want_grad: bool` collapsing the return; **keep** `LogLikComponents` (decomp
  used by diagnostics + cold-rung output).
- **Dropped the false "bootstrap-PF value-only consumer"** claim; named the real
  value-only callers (MH/swap/warmup/sanity/CSMC).
- **Four drift sites** (added `RATE_EPSILON`-vs-`0.0` and the `g`-floor), and
  `step_one` named as the third traversal (out of RC1's scope, noted).
- **Tests:** re-fixture the existing FD test onto overdispersion (it's
  mis-fixtured, not blind); add a posterior-recovery gate instead of a re-bless.
- Corrected "NUTS energy = `complete_data_loglik`" →
  `complete_data_loglik_grad
  (..).0`.
