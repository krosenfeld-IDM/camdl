# Real-compartment support across the inference stack (gh#191 full fix)

- **Status:** Draft **v3** — reworked after an adversarial design review
  (2026-06-09, verdict _needs-rework_) that found four correctness holes +
  design edits (changelog at end). High-risk inference math (`CLAUDE.md`):
  implementation is the inference owner's lane
  (`rust/crates/sim/src/inference/`). This doc designs; it does not unilaterally
  implement.
- **Issue:** gh#191 (full fix; interim `REAL_COMPARTMENTS`-deny gate shipped).
  Tracked at `docs/dev/lifecycle-consolidation-todo.md` (Tier 2).
- **Template:** forward analogue `5c7585c` /
  `docs/dev/incidents/2026-06-07-chain-binomial-stale-real-state.md`:
  `run_chain_binomial` calls `rk4_step` on start-of-step `int_s` then
  `clamp_nonneg`, **before** `step_one` (which already takes
  `real: &mut
  RealState`, `chain_binomial.rs:328`). v3 reuses _that_
  `rk4_step` — no new advance function.

## 0. Reframe: inference-stack-wide, and it includes the observation seam

`ParticleState` (`types.rs:235`) is `counts` + `flow_accumulators` — **no real
reservoir** — and is the one state type for PF / IF2 / PMMH / correlated-PF /
PGAS. So all five mis-fit real-coupled models (reservoir frozen at init). The
zeroed reservoir is read in **four** classes of site, all of which must be
fixed: the **draw** (`step_one`, via `ChainBinomialProcess::step:102` and the
two direct callers), the **transition density/gradient**
(`pgas.rs`/`pgas_grad.rs`), the **complete-data density/gradient** (the primary
θ|X consumers, §2.5), and — the hole v2 missed — the **observation
projection/likelihood** (`MultiStreamObsModel`, §2.5c). The interim gate (§2.8)
currently hides all of this; re-granting it without fixing every site
reintroduces silent `W=0`.

## 1. The abstraction (forks are legitimate)

Shared seam: `ProcessModel` trait → `ChainBinomialProcess::step` → `step_one`;
`ObservationModel::log_likelihood` shared by all. Propagation forks three ways:

| algo               | advance                                                                        |
| ------------------ | ------------------------------------------------------------------------------ |
| bootstrap PF / IF2 | `ProcessModel::step` (PF `particle_filter.rs:248`, IF2 `if2.rs:421`)           |
| PMMH               | wraps the bootstrap PF                                                         |
| correlated-PF      | direct `step_one` (`correlated_pf.rs:461`) — `par_iter` + custom loop          |
| PGAS               | direct `step_one` (`pgas.rs:918`/`1140`) — `SubstepRecord` + ancestor sampling |

Both bypasses are legitimate (the trait's `step()->()` can't express
`SubstepRecord`/ancestor sampling; correlated-PF's noise loop is bespoke). So we
do **not** widen the trait (§3).

## 2. The fix

**2.1 `ParticleState` gains `real: RealState`** (not a raw `Vec<f64>` —
`step_one` and `rk4_step` already take `RealState`, so a raw vec would force a
wrap at every call site). Consequences to wire explicitly:

- `ParticleState::new(n_compartments, n_transitions)` → `(…, n_real)`
  (`types.rs:244`); thread `n_real` through `ParticleSwarm::new`
  (`types.rs:281`) and every `states_buf` allocation.
- Every resample copies `real` beside `counts` via `copy_from_slice` (PF
  `particle_filter.rs:382`, correlated-PF `correlated_pf.rs:514`, IF2
  `if2.rs:551`) — these are pre-allocated double-buffers, _not_ `.clone()`, so
  the buffer particles must be sized with a `real` of the right length.
- `Resettable::reset_accumulators` (`types.rs:259`) **must not** touch `real`
  (persists like `counts`; forward never resets `real_s`).

**2.2 Advance via the existing `rk4_step`, not a new helper.** At each
propagating site, build a transient `IntState`/`RealState` (cheap; `step_one`
already wraps `counts` at `chain_binomial.rs:339`) and call
`rk4_step(model, &int_s, &mut
state.real, params, t, dt)?` then
`state.real.clamp_nonneg()` — _exactly_ the forward sequence
(`chain_binomial.rs:232-234`), start-of-step, **before** `step_one` mutates
counts. One advance function across forward + inference (the §3 verdict).
**Exception — the PGAS reference particle** (`j == j_ref`) skips `step_one`
(`pgas.rs:1135`) and clamps counts from `ref_rec.counts_after` (`:1154`); its
reservoir must likewise be **set from the record, not RK4-advanced** (§2.6),
else the conditioned trajectory's reservoir drifts from the recorded one.

**2.3 Apply at the propagating sites:** `ChainBinomialProcess::step` (fixes PF +
IF2 + PMMH at once), correlated-PF, PGAS free particles (`j != j_ref`).

**2.4 Seed from ICs — and re-seed each Gibbs sweep (the θ-dependent-IC fix).**
`ChainBinomialProcess::initial_state` discards the real state today
(`chain_binomial_process.rs:65`); `CompiledModel::initial_state` already
populates it (`compiled_model.rs:1128-1167`). Stop discarding it. **Crucially**,
for _parameterized_ ICs the real seed is **θ-dependent** (the IC expr is
evaluated against params, `compiled_model.rs:1154-1170`), so PGAS `csmc_as` and
`simulate_reference_on_grid` must **re-seed `particle_reals`/`real` from
`initial_state(current θ)` every sweep** (replacing the zeroed allocs at
`pgas.rs:909`, `:1035`). This re-seed is what makes the `∂/∂θ=0` claim (§5)
sound: the _in-trajectory_ reservoir is conditioned `X`, but the _initial_
reservoir is a function of θ, recomputed per sweep rather than differentiated.

**2.5 Thread the recorded reservoir into ALL score-side consumers** (each builds
a zeroed `RealState` today):

- **(a) `complete_data_loglik` / `complete_data_loglik_grad`** — the _primary_
  θ|X scoring/grad consumers (`pgas.rs:733`, `pgas_grad.rs:414`), which
  reconstruct the density from `SubstepRecord` + `trajectory.initial_counts`.
  Add a `reals_before: &[f64]` argument and thread it to every call site
  (`pgas.rs:733,
  923, 1207, 1311`; `pgas_grad.rs:414`).
- **(b) `log_transition_density_substep` / `…_grad`** (`pgas.rs:556`,
  `pgas_grad.rs:73/293`) — same `reals_before` plumbing.
- **(c) The observation path (the v2-missed fourth site).**
  `MultiStreamObsModel` stores one zeroed `real_s` (`multi_stream_obs.rs:252`,
  with a comment at `:250-251` asserting "likelihood eval never reads real
  compartments" — _true only under the gate §2.8 removes_) and threads it into
  `eval_stream_projection` (`:198/:211/:384`) and `eval_likelihood_resolved`
  (`:412/:454/:506/:525`). A model **observing** a real compartment (e.g.
  `project = derived(W/(W+kappa))`, cholera environmental surveillance) scores
  at W=0. Fix: thread the live per-particle/per-substep reservoir into those
  evals (a struct-field→per-call refactor — scope accordingly) and update/delete
  the stale comment. (Rejecting real-projecting observations instead is wrong —
  environmental surveillance is a first-class use case.)

**2.6 `SubstepRecord` reservoir field semantics.** Store the
**start-of-substep** reservoir (post-RK4, pre-`step_one`) — mirroring
`counts_before`, _not_ `counts_after` (before-vs-after is a silent off-by-one in
W). It is consumed **first** by `complete_data_loglik`/`_grad` (§2.5a),
secondarily by the ancestor traceback (`pgas.rs:1282-1296`). On resample it is
**permuted by the ancestor index** (mirror `pgas.rs:1119`), and the reference
particle reads it directly (§2.2). `PGASTrajectory` (`pgas.rs:152`) gains
`initial_reals` beside `initial_counts`.

**2.7 Completion criterion** — re-grant chain-binomial `REAL_COMPARTMENTS` for
inference (`check_model_capabilities`); flip the `fit::methods` gate test to
accept + fit.

## 3. Consolidation verdict (resolved): reuse `rk4_step`, build nothing new

The review killed v2's bespoke `rk4_advance_reservoir`: a new wrapper over the
existing `rk4_step` that only the inference sites call — while the forward path
keeps calling `rk4_step` directly — is a _fourth_ advance flavor and the exact
forward/inference drift seam this work exists to remove. **Reuse `rk4_step`
(`ode_integrator.rs:21`) at every site** (§2.2). The forks stay forked (legit);
the _shared_ thing is `rk4_step` itself. No trait-widening, no new helper.

## 4. Tests (must catch the missed sites, not just the rate coupling)

- **Observation-side:** a fixture whose **observation projects/likelihoods a
  real compartment W** (not merely a rate that reads W) — asserts the §2.5c
  obs-side W=0 is caught.
- **Parameterized θ-dependent IC:** a fixture whose real IC depends on θ —
  asserts the §2.4 re-seed-on-θ-change path (not a frozen t0 reservoir).
- **Ancestor-lineage:** `dW/dt ≠ 0` **and** ≥1 post-resample observation, so the
  reservoir provably follows the ancestor, not the slot (§2.6).
- **Draw-vs-score consistency** (`complete_data_loglik` on a recorded path ==
  density implied by the draw); **per-algo correctness** on
  `real_coupled_rate.ir.json`; **CRN preservation**; the **gate re-grant** flip.

## 5. Capability blockers: none — with two claims corrected

- **NUTS gradient is mechanical, not a blocker** — rates read compartments via a
  single `Expr::Pop` (`ir/expr.rs:217`); `autodiff.ml:19` differentiates
  `Pop →
  Const 0.0` for int _and_ real, because in the θ|X step compartment
  values are conditioned `X` (`autodiff.ml:7-10`). So no `∂(reservoir)/∂θ`, no
  sensitivity equations — `pgas_grad.rs` needs only the live reservoir in its
  rate/grad _value_ eval (§2.5), so **value + grad land together**.
  _Correction:_ this is sound **only with the §2.4 per-sweep re-seed** — the
  initial reservoir under parameterized ICs _is_ θ-dependent; it is recomputed
  per sweep, not differentiated.
- **No extra density term** — `dW/dt` is a deterministic RK4 integral (no RNG,
  `ode_integrator.rs:85-98`); conditioned on (X, θ) the reservoir carries no
  probability mass. (Holds _because_ `dW/dt` is deterministic; a future
  stochastic-reservoir SDE would need a term — none exists.)
- _Correction:_ v2's "no structural integer-only assumption in any algo" was
  **wrong** — `MultiStreamObsModel.real_s` (`multi_stream_obs.rs:252`) is
  exactly such an assumption in the obs seam; §2.5c removes it.

## 6. Sequencing & the four must-get-right pieces

One coherent change (value + grad + all algos), gated by correctness on:

1. **The observation-projection site** (§2.5c) — the silent-W=0 hole.
2. **The PGAS reference-particle clamp-from-record** (§2.2/2.6).
3. **The per-sweep IC re-seed** for θ-dependent ICs (§2.4).
4. **`SubstepRecord` reservoir = start-of-substep**, permuted by ancestor index
   (§2.6) — the lineage + time-axis traps. Then: §2.1 plumbing → reuse
   `rk4_step` at sites → thread scoring/grad/obs → re-grant + flip gate.
   Inference owner confirms the record shape + the per-loop start-of-step
   ordering against the live code.

## Changelog (v2 → v3, from the design review)

- Dropped the new advance helper → reuse `rk4_step` (drift-seam smell).
- Reservoir typed `RealState`, not `Vec<f64>` (avoid per-call wraps).
- Added §2.5c: the observation-projection W=0 site (the missed fourth site) +
  the stale `multi_stream_obs.rs:250-251` comment.
- Named `complete_data_loglik`/`_grad` as the primary score consumers + the
  `reals_before` signature fan-out (§2.5a/b).
- Reference particle clamps reservoir from record, advance skipped (§2.2).
- Per-sweep re-seed for θ-dependent parameterized ICs (§2.4); corrected the §5
  `∂/∂θ=0` argument's precondition.
- `SubstepRecord` reservoir = start-of-substep (before/after);
  `PGASTrajectory.
  initial_reals` (§2.6).
- Resample-buffer sizing + `ParticleState::new(n_real)` signature (§2.1).
- Corrected two wrong §5 claims.
