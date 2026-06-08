# Pre-merge adversarial review: feature/unified-timeline

Date: 2026-06-07
Scope: `feature/unified-timeline` (58 commits ahead of `main` b2f907f) — the
scheduling-spine / effect-purity-seam / StepClock / drop-tau / dt-rate-oracle
work, ahead of a fast-forward merge to main.
Method: four parallel adversarial subagents, one per risk dimension, each tasked
to *break* the branch (find the silent-wrong-answer, the byte-identity
violation, the vacuous test), report severity-ranked with file:line + repro,
and distinguish "verified (traced)" from "suspicion."

## Verdict

**Core is correct and well-oracled — merge after the blockers below are
cleared.** The inference-math and effect-seam reviewers each traced the
high-risk surfaces end-to-end and found **no CRITICAL/HIGH defect introduced by
the branch**. The blockers are an incomplete deletion (the integration gate) and
doc drift, not a math error.

## Findings and resolution

### CRITICAL

- **C1 — `make test` integration stage broken.** The tau-leap deletion
  (761c812) left `tests/test_ocaml_to_rust.sh:45` iterating `tau_leap` and three
  batch fixtures pinned to `backend = "tau_leap"`; both now hard-error (clap /
  TOML reject the variant). The scoped `cargo test -p sim`/`-p cli` gate used for
  drop-tau does not exercise the integration stage. **Fixed:** dropped tau from
  the smoke loop, repointed the three fixtures to `chain_binomial` (run counts
  are backend-independent → batch expectations unchanged). `make test-integration`:
  45 passed, 0 failed.

### HIGH

- **README.md + AGENTS.md + `--help`** still advertised tau-leap with
  copy-pasteable commands that now error; AGENTS.md actively told agents
  "Tau-leap is fine," steering agent-authored configs into C1. **Fixed.**
- **dt-rate oracle ratio-only hole** (`gate_dt_rate_exact_clip` test 2): the
  ratio `prop_short/prop_grid == dt_actual/grid_dt` proves `Expr::Dt` is *linear
  in the dt argument*, not `== ctx.dt` — a `Dt → k·ctx.dt` overload (or a read of
  the wrong-but-proportional dt field, the exact grid_dt-vs-dt_actual mixup this
  branch prevents) passes. **Fixed:** added an absolute pin
  (`prop == β·S·I/N·(dt_actual/τ)`), mutation-verified (`Dt → ctx.dt·2` keeps the
  ratio green but fails the absolute pin).

### MEDIUM (pre-existing — filed, not blocking this merge)

- **σ² evaluated at a zeroed state in the PGAS gamma-density value function**
  (`pgas.rs:759-784`) while sim + gradient use `counts_before` → value/gradient
  mismatch (biased NUTS) for *state-dependent* overdispersion. Present on `main`.
  Filed: `docs/dev/notes/2026-06-07-pgas-overdispersion-zeroed-state.md` (note,
  not incident — not yet reproduced). Companion gap: no oracle covers
  overdispersion under Exact clipping.
- **Seasonal gradient FD arm** (`pgas_exact_tiling` arm b) uses a `.max(1.0)`
  denom that makes its `1e-4` bound absolute for small gradients — the weakest of
  the exact-PGAS arms (authors concur; the magnitude family at uniform non-unit
  dt is the strong gate). Follow-up tightening, not a blocker.

### LOW (resolved or deferred)

- Stale `tau` present-tense in ~12 source comments — swept (delete-on-sight).
  Deferred as dated/historical: the `schedule.rs` Stage-1 module-doc narrative
  and `corner_cases/README.md` tau column.
- `set(real_compartment, <0)` is silently clamped (not erred) on ODE/chain; the
  new events-on-real surface makes a negative-real-`set` reachable on chain.
  Pre-existing-class; worth a deliberate clamp-vs-error decision since real
  reservoirs feed water-borne FOI. Flagged for the maintainer.

## What the reviewers verified clean (traced)

- StepClock dt-threading end-to-end (forward `step_one` ↔ PGAS density ↔
  gradient); `EvalCtx.dt` consumed live, no frozen dt-dependent cache.
- `build_substep_grid` Exact boundary/obs-keying; no off-by-one.
- CRN / RNG ordering preserved (the new effect seam is RNG-free; draw count and
  order unchanged); all `step_one` sites pass `dt_actual`/`grid_dt` in the right
  slots.
- `-inf` surfaced not swallowed; effect resolve/apply symmetric; fusion reads the
  pre-drain snapshot; ODE de-quant isolated to 3 baselines; event→intervention
  canonicalization pinned with negative controls.
- Capability coverage intact post-tau; run-identity index shift is the only
  identity change, no literal run_id pinned; CLI match exhaustiveness; no dead
  code / `#[allow(dead_code)]`.
- Test-rigor sweep: the branch's new gates are load-bearing with real negative
  controls; the `gate_constant_fold_ab` fold-ON path *is* exercised (committed
  folded fixture, not a flag toggle) — resolving a standing open question.
