# Code review: the unified time system (cas-overhaul..HEAD)

Date: 2026-06-08
Reviewer: Claude (Opus 4.8), adversarial-workflow + maintainer-standard verification
Rubric: `docs/dev/code-review.md`
Scope: the "unified time system" arc since the `cas-overhaul` tag (`2bc76ac0`):
the merged-timeline **Schedule spine** (`schedule.rs`), the **pure
effect-resolution seam** (`effects.rs`, `lifecycle.rs`, `intervention.rs`), the
**substep clock / `s*dt` convention** (`resolved_expr.rs`), the rerouting of all
forward backends (`gillespie.rs`, `chain_binomial.rs`, `ode.rs`) and inference
algorithms (`pgas.rs`, `correlated_pf.rs`, `particle_filter.rs`, `if2.rs`)
through the spine, and the tau-leap deletion.

Method: 9 dimension reviewers (one per rubric area) → per-finding adversarial
verification (3 diverse-lens verifiers for Critical/High, 1 for Medium/Low).
20 raw findings, 19 confirmed, 1 partial, 0 refuted. The two Critical and three
High findings were then re-verified by hand against current code; the
verification commands are pasted inline below.

## Verdict

The spine refactor itself is sound — the premerge review's traced-clean
conclusion holds for the boundary math, CRN/RNG ordering, and the
effect-resolve/apply symmetry. **But two Critical defects produce silently wrong
scientific output on the production inference path**, and neither is caught by
the existing test suite:

1. The PGAS+NUTS gradient path scores an energy inconsistent with its own
   gradient for overdispersed models (`#1`).
2. The gh#191 real-compartment gate was wired into `profile`/`survey`/`nlopt`
   but **not** into the main `camdl fit run` path it was written to protect
   (`#2`).

Both are "compiles, runs, no error, wrong number" — exactly the class the rubric
and CLAUDE.md treat as critical for public-health output.

## Scope hygiene — already fixed / already known (NOT re-reported)

- Premerge **C1** (tau-leap deletion broke the integration gate) — fixed
  (`9975d370`, `2450fb0a`).
- Premerge **dt-rate oracle ratio-only hole** — fixed (`5621d67a`, absolute pin
  added, mutation-verified).
- The **σ²-evaluated-at-zeroed-state** note
  (`docs/dev/notes/2026-06-07-pgas-overdispersion-zeroed-state.md`) is a
  *sibling* of `#1` below — `#1` is the more severe defect (the gamma value term
  is **missing entirely** from the gradient path, not merely evaluated at the
  wrong state). The `#1` fix closes both in one place.
- The premerge LOW "`set(real_compartment, <0)` silently clamped" is the
  `Set`-family cousin of `#4` (the `Add` negative-amount hole) and `#6`
  (multi-writer `Set` divergence).

---

## CRITICAL

### #1 — PGAS+NUTS gradient-path value omits the gamma-multiplier density term

- **Location**: `rust/crates/sim/src/inference/pgas_grad.rs::complete_data_loglik_grad`
  (401–458) and `::log_transition_density_grad` (57–245); consumed at
  `pgas.rs:1947–1981` (the `run_pgas` NUTS closure) → `nuts.rs:201`.
- **Category**: statistical correctness.
- **Defect**: `complete_data_loglik_grad` returns a value `ll` containing the
  transition Binomial/Poisson density, the IVP Binomial density, and the
  observation density — but **not** the gamma-multiplier density
  `log Γ(g; dt/σ², σ²/dt)`. It nonetheless adds that term's **gradient** (line
  434). The value-function counterpart `complete_data_loglik` *does* add the
  gamma value (`pgas.rs:791–795`). NUTS sources **both** its potential energy
  (`ll → log_p`, `h0 = −log_p + KE` at `nuts.rs:201`) and its leapfrog force
  (`ll_grad_theta`) from this one call — so the integrator follows a force that
  includes `∂/∂θ log Γ` while the energy it is scored against omits `log Γ`.
- **Why it matters**: for any model with `overdispersed(rate, σ²)` where an
  estimated parameter feeds σ² (the standard cVDPV2 process-noise setup), the
  leapfrog dynamics and the slice/no-U-turn acceptance target *different*
  densities on the σ² coordinate. The σ² (process-noise) marginal — the
  parameter governing outbreak-size uncertainty — is sampled from the wrong
  stationary distribution, silently. For a parameter entering *only* through σ²,
  the entire missing term is its likelihood contribution. There is also a
  marginal-vs-conditional mismatch: the CSMC trajectory weight
  (`complete_data_loglik`, with gamma) disagrees with the NUTS θ-target
  (`complete_data_loglik_grad`, without).
- **Fix**: in `complete_data_loglik_grad`, add the gamma-multiplier density
  *value* to `log_p` at each substep, mirroring `pgas.rs:787–795`. Cleanest:
  make `log_gamma_density_grad_substep` return `(value, grad)` and add its value
  to `log_p` alongside its grad. While there, evaluate σ² at `counts_before`
  (as the gradient already does at `pgas_grad.rs:315` and `step_one` does at
  `chain_binomial.rs:353/359`), not the zeroed state the value function uses —
  fixing the zeroed-state note in the same edit. Add an FD-of-the-value-fn vs
  value-of-the-grad-fn consistency assertion (the current
  `gradient_check_overdisp` only checks grad-vs-FD-of-`complete_data_loglik`, so
  the missing value term is outside its assertion surface).
- **Severity**: Critical.
- **Verification (by hand)**:
  ```
  $ grep -n 'log_p +=' pgas_grad.rs
  172:  log_p += binom_logpmf(n_exit, n_src as u64, p_total);
  202:  log_p += binom_logpmf(flow_k, remaining, p_split);
  233:  log_p += crate::inference::obs_loglik::poisson_logpmf(flow, mean);
  385:  log_p += binom_logpmf(count, patch_pop as u64, frac);
  422:  log_p += td;                       # td = transition-only (no gamma)
  444:  log_p += obs_model.log_likelihood_from_flows_and_counts(...)
  # NO `log_p += log_gamma...`; gamma appears only as grad: line 434
  #   `for i in 0..d { grad[i] += gamma_grad[i]; }`
  $ grep -n 'transition_ll += log_gamma_density' pgas.rs   # value fn DOES add it
  795:  transition_ll += log_gamma_density;
  # NUTS closure: pgas.rs:1947 (ll,ll_grad)=complete_data_loglik_grad(...);
  #   :1958 log_p = beta*ll ; :1966 grad_z += beta*ll_grad_theta[i]*...
  # nuts.rs:201  h0 = -current_log_p + kinetic_energy(momentum)
  ```

### #2 — Real-compartment capability gate (gh#191) never called on `camdl fit run`

- **Location**: `rust/crates/cli/src/fit/methods.rs:402–449`
  (`check_model_capabilities`) vs `fit/mod.rs::cmd_fit_run_v2` and
  `fit/runner.rs::FitRunConfig::build`.
- **Category**: not wired through.
- **Defect**: the gh#191 fix correctly removed `REAL_COMPARTMENTS` from
  chain_binomial's *inference* capability set, so a real-coupled model on
  chain_binomial inference *should* be rejected. But `check_model_capabilities`
  is only called from `survey.rs:183`, `profile.rs:358`, and
  `nlopt_stage.rs:92` (hardcoded `"ode"`). The production `camdl fit run`
  dispatch (`cmd_fit_run_v2` → `FitRunConfig::build` → IF2/PGAS/PMMH/pfilter)
  never calls it, and there is no other real-compartment rejection on that path.
  The gate is dead weight on the exact path it was written to protect.
- **Why it matters**: a model with a real (ODE-coupled) reservoir — e.g. a polio
  environmental/water reservoir W coupled into the FOI — fit with
  `backend = "chain_binomial"` (the default) passes through with no error. Per
  incident `2026-06-07-chain-binomial-stale-real-state.md`, the inference loops
  carry no real state and never advance the reservoir; it stays frozen at its
  init value for the whole fit. Any parameter governing reservoir dynamics or
  reservoir→host transmission is fit against a constant W — a biased
  posterior/MLE with no diagnostic. This is a posterior that could move a
  vaccination decision.
- **Fix**: call `check_model_capabilities(fit.config.backend.as_str(),
  &compiled)?` inside `FitRunConfig::build` (alongside the existing
  obs-alignment gate at `runner.rs:~348`), so every stochastic-inference stage
  is gated at the single shared seam — mirroring how `nlopt_stage::run_stage`
  gates the ODE path. Add a *dispatch-level* regression (simulate a real-coupled
  fixture, assert `camdl fit run` exits with the gh#191 error); the existing
  unit test at `methods.rs:661–671` calls the function directly and so passes
  whether or not it is wired.
- **Severity**: Critical.
- **Verification (by hand)**:
  ```
  $ rg -n 'check_model_capabilities|required_capabilities' crates/cli/src | grep -v '#\[' 
  util.rs:1699           required = compiled.required_capabilities();   # simulate path
  profile.rs:358         check_model_capabilities(...)
  fit/methods.rs:402     pub fn check_model_capabilities(...)           # def
  fit/methods.rs:665,670 check_model_capabilities(...)                  # in #[cfg(test)]
  fit/nlopt_stage.rs:92  check_model_capabilities("ode", ...)
  survey.rs:183          check_model_capabilities(...)
  $ rg -n 'real_comp|REAL_COMPARTMENTS|reservoir|frozen' fit/mod.rs fit/runner.rs
  (no matches)           # no real-compartment rejection on the run path
  ```

---

## HIGH

### #3 — Exact backends (ODE, Gillespie) double-fire an intervention when two fire times round to the same dt-step

- **Location**: `ode.rs:225–237,272–282` and `gillespie.rs:243–259` (apply),
  via `effects.rs:258–277` (`due_effects`) over
  `intervention.rs:195–206` (`all_intervention_times`) walked by
  `schedule.rs:227–240` (`substep`).
- **Category**: numerical correctness.
- **Defect**: under `StepPolicy::Exact` the schedule walks every raw intervention
  fire time as its own boundary; at each landing `due_effects` re-derives
  due-ness by `time_to_step(t, grid_dt)` = `(t/dt).round()`. When two distinct
  fire times of the same intervention fall within one dt and round to the same
  step (e.g. `at: [2.3, 2.4]` at dt=1, or a `Recurring` period < dt), the
  intervention is applied **once per boundary** — twice. The cursor only advances
  past effects within `EFFECT_EPS` of the current `t`, not past every effect
  mapping to the same step.
- **Why it matters**: `Add` (importation), `AbsoluteTransfer`/`FractionTransfer`
  (S→V vaccination) are non-idempotent — firing twice doubles the people moved
  (two 50% pulls remove 75%, not 50%). The chain_binomial (Snap) path fires the
  same intervention exactly **once**, so the two backends silently disagree on
  the same model with no cross-backend agreement test covering it. A doubled
  campaign over-vaccinates in the counterfactual.
- **Fix**: make Exact effect application key on the *step*: after `due_effects`
  fires, advance the cursor past every remaining effect whose
  `time_to_step(·, grid_dt)` equals the current step. Equivalently, dedup
  `all_intervention_times` by `time_to_step(t, dt)` before handing it to the
  schedule (mirroring the `fire_times_to_steps` BTreeSet dedup the firing key
  already uses). Add a cross-backend agreement test: an `Add`/`Transfer` with
  `at: [2.3, 2.4]` at dt=1 must give identical post-intervention counts on
  chain_binomial, ode, and gillespie.
- **Severity**: High.
- **Verification (by hand)**: `time.rs:30` `(t / dt).round() as i64` (2.3→2,
  2.4→2); `intervention.rs:204` `times.dedup()` (Rust `Vec::dedup` =
  consecutive-equal only → [2.3,2.4] survive as two boundaries).

### #4 — Negative `add` in (−0.5, 0) bypasses the hard-error guard

- **Location**: `effects.rs::resolve_action:476–491` (discrete) and
  `::apply_action_f64:358–370` (ODE).
- **Category**: numerical correctness.
- **Defect**: both guards test the *rounded* value — `let count = v.round() as
  i64; if count < 0` and `if v.round() < 0.0` — not the raw resolved amount. For
  a resolved amount in (−0.5, 0) (e.g. −0.3), `round()` is −0.0, so the guard
  does not fire. An int target then pushes `IntDelta { delta: 0 }` (silent
  no-op); a real target pushes `RealDelta { delta: v }` = the negative amount
  (silent subtraction). The inline comment "A negative add is always a config
  bug … hard error on every path" is thereby false.
- **Why it matters**: a real-targeted `add(W, x)` whose expression lands in
  (−0.5, 0) — plausible for a fitted/parametric importation rate — silently
  *subtracts* each firing; for an `always_active` event firing every substep
  this accumulates and can drive a real reservoir negative with no error. The
  downstream negative-count safety net (`lifecycle.rs:92`) iterates
  `current.counts` only — it never scans `real.values` — so the negative real
  state is never caught. In inference this is a hard-error (non-recoverable)
  path, so the missed defect stays silent in a fit.
- **Fix**: guard on the raw `v` (`if v < 0.0`) before computing `count`, in both
  resolvers. Extend the post-INTERVENE negative scan to the real arena. The
  existing `add_negative_is_hard_error_on_any_path` test uses −1.0 (rounds to
  −1, caught) — add a −0.3 case on both int and real targets.
- **Severity**: High (narrow trigger band — requires a resolved add amount in
  (−0.5, 0) — but a real invariant violation with no second line of defense for
  the real arena).
- **Verification (by hand)**: `effects.rs:477` `let count = v.round() as i64`;
  `:480` `if count < 0`; `:489` `Arena::Real(i) => RealDelta { delta: v }`;
  `:359` ODE `if v.round() < 0.0`; `lifecycle.rs:92` scans `current.counts`
  only; test at `:669` uses `Expr::const_(-1.0)`.

### #5 — PGAS gradient scores deterministic source-less (inflow) transitions as Poisson

- **Location**: `pgas_grad.rs::log_transition_density_grad:224–242`.
- **Category**: statistical correctness.
- **Defect**: the ungrouped/inflow loop has no `DrawMethod::Deterministic`
  check — every non-handled ungrouped transition gets
  `log_p += poisson_logpmf(flow, rate*dt)` plus a Poisson gradient term. The
  value function (`pgas.rs:634–644`) builds an `is_determ[]` table and treats
  deterministic ungrouped transitions with an exact-count check (`flow !=
  round(mean)` → −∞; else *no* density). A source-less transition is ungrouped
  by construction (`compiled_model.rs:642` groups only transitions with a
  negative-stoich source), so a deterministic inflow (constant immigration,
  `transition.rs:34–37`) falls into the gap.
- **Why it matters**: for a model with a deterministic inflow (births /
  importation) fit under PGAS+NUTS, the gradient's `log_p` (the NUTS potential,
  `pgas.rs:1958`) carries a spurious Poisson factor the true model lacks →
  biased posterior on any parameter the inflow rate depends on; and that `log_p`
  no longer matches the CSMC weight from `complete_data_loglik` → marginal and
  conditional targets disagree. When `flow != round(mean)` the value rejects
  (−∞) but the gradient returns finite — accepting a trajectory the value
  forbids.
- **Fix**: mirror the value path — build/consult `is_determ` in the ungrouped
  loop, do the exact-count check, add no density/gradient term (as
  `pgas.rs:636–639`).
- **Severity**: High (triggers only for models with a `Deterministic`
  source-less transition under PGAS+NUTS; no gradient golden currently has one).
- **Verification (by hand)**: `grep -n 'is_determ' pgas_grad.rs` → no matches;
  `pgas_grad.rs:226` `if handled[tr_idx] || rate <= 0.0 { continue; }` then
  `:233` poisson; contrast `pgas.rs:634/636` (`rate <= RATE_EPSILON`, then
  `if is_determ[i]`).

---

## MEDIUM

### #6 — Coincident `set` events on one compartment diverge: chain (snapshot-relative additive) vs ODE (absolute last-wins)
- `effects.rs:493–502` (discrete `Set` → `(v.round() as i64) - snap.int.counts[i]`,
  applied additively against the frozen snapshot) vs `:372–375` (ODE `Set` →
  `int_vals[i] = v`, last-wins). Two events `set`-ting the same compartment to
  a then b: chain gives `a+b−S0`, ODE gives `b`. Distinct from M1
  (event-vs-intervention ordering); this is multi-writer `Set` within one stage.
  **Fix**: define the multi-writer-`Set` semantics (reject as ambiguous, or
  canonicalize to last-wins on both backends by resolving `Set` against running
  state, not the snapshot) and add a cross-backend fixture. **Category**:
  numerical correctness.

### #7 — Gillespie's sparse propensity update never refreshes real-compartment-coupled transitions
- `gillespie.rs:263–273, 356–389`; root cause `compiled_model.rs:601,624–630`
  (int-only dependency graph: `collect_int_comp_deps` records only
  `global_to_int` deps; `expr_is_time_dependent` is true only for Time/TimeFunc).
  A rate reading a real (RK4-integrated) compartment but no Time/TimeFunc (e.g.
  `beta_W*W`) is in neither the `comp_to_transitions` nor `time_dep` set, so its
  propensity stays frozen at the last full recompute (init, then every
  `FULL_RECOMPUTE_INTERVAL=10_000` events). The realistic SIWR test masks this
  because its rate also carries `beta_I*I/N`, forcing incidental recomputes.
  **Fix**: build a real-compartment dependency map and recompute dependent
  propensities after each `rk4_step`; or, conservatively, full-recompute after
  each `rk4_step` whenever the model has any real compartment. **Category**:
  numerical correctness. *(Possibly pre-existing — the int-only graph predates
  the time-system work; surfaced by the recent real-coupling surface.)*

### #8 — No FD-gradient test exercises an `Expr::Dt`-in-rate model through `complete_data_loglik_grad` under Exact clipping
- `gate_dt_rate_exact_clip.rs` (value-only); no `gradient_check*.rs` model has
  `Expr::Dt` in any `rate_grad`. The mechanism is currently **correct**
  (`pgas_grad.rs:78` builds `EvalCtx{dt: rec.dt_substep}`; `resolved_expr.rs:395`
  `Dt => ctx.dt`), but the NUTS-feeding gradient has no oracle — a future
  clip/StepClock refactor that froze the gradient's dt at `grid_dt` would pass
  the whole gradient suite. **Fix**: add a gradient arm to
  `gate_dt_rate_exact_clip.rs` FD-checking `complete_data_loglik_grad` on the
  `dt_rate` model under Exact with off-grid obs. **Category**: tests.

### #9 — Systematic-resampling loop duplicated verbatim
- `correlated_pf.rs:518–539` (`sorted_systematic_resample`) is a byte-for-byte
  copy of `resampling.rs:18–40` (`systematic_resample`); the only difference is
  the source of the single uniform (`rng.uniform()` vs pre-drawn `base_uniform`).
  PGAS/IF2/bootstrap already share the canonical one; CPM is the lone fork — a
  drift hazard that would bias the CPM-PMMH posterior relative to vanilla PMMH.
  **Fix**: extract `systematic_resample_from_uniform(log_weights, u0)`; both call
  sites delegate. **Category**: DRY.

### #10 — Dead public API: `next_stop`/`TimelineStop`/`StopReason` built and tested but wired into nothing; proposal claims it landed
- `schedule.rs:55–78, 259–283`. Zero callers in the workspace; every driver
  still uses `substep`/`*_due_at` and `due_effects` re-derives due-ness via
  `time_to_step` — the exact re-derivation the spine-v2 §B `next_stop` seam was
  meant to remove. The proposal's status table marks it "✅ landed." **Fix**:
  either delete the trio + tests and correct the status table, or route at least
  one driver through `next_stop` + a cursor-reading `due_effects` (closing
  lifecycle-review #2). Per CLAUDE.md, aspirational dead code is delete-on-sight.
  **Category**: type/trait design.

### #11 — Newtype hygiene: `dt_actual` and `grid_dt` are both bare `f64` in adjacent slots
- `chain_binomial.rs:313–324`, `effects.rs:258–266`, `schedule.rs:174–181,359`,
  `time_to_step(t: f64, dt: f64)`. The StepClock invariant ("eval on
  `dt_actual`, fire-keys on `grid_dt`") is the spine's central correctness
  contract, yet a transposition compiles and passes every on-grid golden
  (the two are equal under Snap and on-grid Exact) — failing silently only when a
  filter clips a substep to an off-grid observation, the path that feeds a
  posterior. **Fix**: introduce `Time`/`Dt`/`GridDt` newtypes (at minimum the
  `Dt`/`GridDt` pair) in the spine signatures; the swap becomes a compile error.
  **Category**: type/trait design.

### #12 — `camdl fit run` ignores the model's `simulate { dt }`, uses fit.toml `[config].dt` (default 1.0); `camdl simulate` honours it (gh#161)
- `fit/runner.rs:256` `let dt = fit.config.dt;` + `config_v2.rs:169`
  `default_dt() = 1.0`; contrast `main.rs:591` `a.backend.dt.or(model_dt)
  .unwrap_or(1.0)`. A modeller who sets `simulate { dt = 0.1 }` and runs
  `camdl fit run` without `[config] dt` silently fits at dt=1.0 — a 10× coarser
  Euler-multinomial step, which can shift the MLE/posterior for an O(dt) backend
  — and the resulting params re-simulated via `camdl simulate` use dt=0.1,
  diverging from what the fit saw. **Fix**: in `FitRunConfig::build`, default
  `[config].dt` from the model's `simulation.dt` (matching `camdl simulate`), or
  hard-warn on a mismatch. **Category**: user footgun.

### #13 — Promised analytic marginal-likelihood oracle for the bootstrap PF is never implemented *(workflow corrected Medium; reviewer claimed High)*
- `tests/particle_filter.rs` docstring line 4 claims "The marginal likelihood is
  analytically tractable, so we can verify the PF converges to the correct
  value," but no test computes that analytic marginal and compares
  `bootstrap_filter`'s estimate to it — the tests only check determinism,
  finiteness, variance-decreases-with-N, ESS bounds, and ic_free bookkeeping. A
  deterministic systematic bias (miscounted substep per window, missed/doubled
  obs increment, constant logsumexp offset) is reproducible across seeds and
  shrinks with N, so it passes all of them. The only absolute PF-value pins
  (`gate_inference_baseline.rs`) are self-captured ULP ratchets — they catch a
  *change* from the dev-machine capture, not a value that was wrong at capture
  time. **Fix**: forward-filter the pure-death (N≤100) + Poisson model over the
  integer-state distribution (a finite stochastic matrix) for the exact marginal,
  then assert `bootstrap_filter` at large N is within a few MC SEs. **Category**:
  tests. *(Per rubric §9, a missing known-correct oracle is itself the first
  finding — this is the headline inference output, shared by PGAS/IF2/PMMH.)*

---

## LOW

### #14 — ODE negative-add error still reports hardcoded `t: 0.0`
- `effects.rs:359–366`. Commit `6b07627b` fixed this in the discrete path
  (`resolve_action` passes `t`) but the parallel `apply_action_f64` ODE Add arm
  still uses `t: 0.0` — `t` (= `t_boundary`) is in scope at the call site. **Fix**:
  thread `t` through; better, fold the negative/finite checks into one shared
  helper (also the vehicle for the `#4` fix). **Category**: DRY.

### #15 — `Expr::Dt`-referencing rates run on Gillespie/ODE with no error, evaluating dt as the model grid resolution
- `gillespie.rs:148,…,362` (`dt = model.simulation.dt.unwrap_or(1.0)`);
  `required_capabilities()` (`compiled_model.rs:1015`) emits no flag for `Dt`
  usage, and the dispatch gate (`util.rs:1698`) checks only OVERDISP/REAL/BALANCE.
  A documented gh#54 dt-scaled rate dispatches to event-driven Gillespie and
  silently computes with dt=1.0. **Fix**: add a `RUNTIME_DT` capability, set it
  when any expression contains `Expr::Dt`, declare it only on chain_binomial +
  the inference filters; Gillespie/ODE then fail the gate with a named error.
  **Category**: user footgun.

### #16 — BetaBinomial obs-gradient domain guard compares un-rounded `observed` to rounded `n`
- `obs_model.rs:222–241` + `obs_loglik.rs:100–101`. The value path rounds k
  first; the gradient passes raw `observed` with guard `k > n` (k raw, n
  rounded), so for `round(observed) <= n_round` but `observed > n_round` (e.g.
  10.4 vs 10) the value is finite (nonzero true slope) while the analytic
  gradient is zeroed → spurious NUTS divergences at that obs. The Binomial arm
  rounds before comparing. **Fix**: round k before the domain check in
  `beta_binomial_logpmf_grad`. **Category**: statistical correctness. (Impact
  limited — needs fractional observed/`n_val` near a .5 boundary.)

### #17 — Bootstrap filter consumes the diagnostic RNG twice per obs when predictions + prequential are both on (the default)
- `particle_filter.rs:274–276` and `:304–307`. `diag_rngs[i]` is advanced by the
  prediction-quantile draws and then reused for the prequential `y_pred_samples`,
  so the prequential samples depend on whether predictions are enabled — both
  default on for the PFilter stage. The samples remain valid draws from
  `p(y_t|x_t)` (not posterior-moving) but it breaks reproducibility-relative-to-
  flags, despite the file engineering RNG-stream separation for the process path.
  **Fix**: give prequential its own RNG stream (a third disjoint offset).
  **Category**: statistical correctness.

### #18 — `lifecycle.rs` module-doc claims "two functions"; the module defines one
- `lifecycle.rs:12–22` vs `:50`. The doc says PROPOSE (stage 1) + INTERVENE/
  BALANCE tail (stages 3–4) live here; only `apply_post_advance` does — PROPOSE
  is `effects::resolve_event_batch`. Stale doc-vs-code in a load-bearing module.
  **Fix**: reword to one function + point to `resolve_event_batch`. **Category**:
  type/trait design.

### #19 — `balance_conservation.rs` asserts a conservation invariant that holds by construction *(workflow corrected Low; reviewer claimed Medium)*
- `tests/balance_conservation.rs:137–148`. Balance overwrites R with
  `round(eval(N0−S−I))` every substep (`lifecycle.rs:71–82`); with integer
  N0,S,I, `round` is the identity, so `S+I+R == N0` is a tautology that holds
  regardless of whether balance reads the right state, the right dt, or fires at
  the right time. The `R>0` control proves balance ran, not that it conserved
  correctly. **Fix**: assert R against a hand-computed expectation at a post-cull
  snapshot, or against an independent cumulative-culled-mass recomputation that
  does not go through the same `eval(N0−S−I)` path; or add a sibling balance with
  a non-integer expr where `round` is not the identity. **Category**: tests.

### #20 — Ungrouped-Poisson gradient loop uses `rate <= 0.0` instead of `RATE_EPSILON` (partial / threshold mismatch)
- `pgas_grad.rs:226`. The grouped path was fixed to `RATE_EPSILON` (line 117,
  with a "Now mirrors pgas.rs exactly" comment) but the ungrouped loop still
  guards `rate <= 0.0`; the value fn uses `RATE_EPSILON` (1e-15) at `pgas.rs:634`.
  For an ungrouped rate in (0, 1e-15] the value contributes nothing while the
  gradient adds a Poisson term — the same marginal/conditional drift the IM7/IM9
  fix closed for the grouped path, leaving the comment false. **Fix**: change line
  226 to `rate <= crate::chain_binomial::RATE_EPSILON`. **Category**: numerical
  correctness. (Razor-thin band; bundle with `#5`.)

---

## Recommended actions

1. **File `gh` issues for `#1` and `#2`** (Critical) and fix before any release
   that advertises overdispersed PGAS+NUTS or real-coupled inference. `#1` and
   `#5`/`#20` are all in `pgas_grad.rs` and should be fixed + tested together
   (the value/gradient consistency oracle covers all three).
2. **`#3` and `#4`** (High) are in the effect application path the spine just
   consolidated — fix with the cross-backend agreement tests they each name.
3. **`#11`** (newtypes for `Dt`/`GridDt`) is the structural change that would
   have made `#3`/`#11`-class swaps impossible; worth doing while the spine is
   fresh.
4. The **test findings** (`#8`, `#13`, `#19`) are the rubric's "tests that
   actually fail when the code is wrong" gap — each names the specific mutation
   its missing/strengthened test should catch.
