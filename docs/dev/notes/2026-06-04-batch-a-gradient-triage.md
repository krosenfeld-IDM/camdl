# Batch A triage: inference-gradient correctness

Date: 2026-06-04 Project: camdl Tags: inference, gradients, pgas, nuts, triage

## Context

Follow-up to the alpha-issue triage. "Batch A" was the cluster I ranked
priority-zero: silently-wrong gradients that corrupt Bayesian posteriors with no
error. Five issues (#128, #76, #20, #95, #78), each root-caused against the code
by a read-only investigation agent. Headline: **the open-issue list was stale —
two of the four "bugs" were already fixed.** The genuine remaining work is one
live sampler bias (#95) plus a guard (#78) and small defensive adds. Assume
nothing is current until checked.

## Verified states

| #                       | State         | Evidence                                                                                                                                   | Remaining                                                                                               |
| ----------------------- | ------------- | ------------------------------------------------------------------------------------------------------------------------------------------ | ------------------------------------------------------------------------------------------------------- |
| **#20** σ² gamma grad   | FIXED         | `00b1a2d`; FD-pinned; red→green bug-injection (rel_err 1.00→0 across 7 FD tests)                                                           | +scipy value anchors (defense-in-depth); #79 = dup loop + zero-fill asymmetry                           |
| **#76** obs-param grad  | FIXED (gated) | `cb46b40`; NegBin/Normal/Poisson/Binomial/Bernoulli FD-tested 1e-4                                                                         | BetaBinomial α/β (implementing); parametric `DerivedExpr` chain-rule — both **gated by C1, not silent** |
| **#128** rate_grad drop | LATENT        | `compiled_model.rs:898` `filter_map` drop; cannot fire on current compiler output (autodiff emits only declared-param keys)                | defensive hard-error (~1–2h); construction-site ± `ir::validate` (#123)                                 |
| **#95** Gillespie bias  | **LIVE**      | `gillespie.rs:187` freezes λ at interval start; piecewise-constant on the output grid after `424b6a9a`; ~11% off in the incident's numbers | thinning sampler + sound bound; **proposal (next morning)**                                             |
| **#78** `--check-grads` | NOT BUILT     | `fd_check` + `test_nuts_target_gradient_on_z_scale` exist test-only                                                                        | the class guard; ~250 LoC; API decisions pending                                                        |

## BetaBinomial gradient (the #76 residual, implementing)

Scipy-validated (`betabinom.logpmf`, rel err ~1e-8), ψ = digamma:

```
∂logP/∂α = ψ(k+α) − ψ(α) − ψ(n+α+β) + ψ(α+β)
∂logP/∂β = ψ(n−k+β) − ψ(β) − ψ(n+α+β) + ψ(α+β)
```

Wiring it also removes the BetaBinomial fence from the C1 gate (`pgas.rs:1600`)
and inverts `pgas_gate_betabinomial.rs` (it asserted the _refusal_). The
parametric-`DerivedExpr` fence stays.

## The pattern under all of it

Each slipped the **same** way (and the same way the run_id and stale-binary bugs
slipped this week): FD/gradient tests run only on golden fixtures, so the bug
can't manifest — #128's key always resolves, #76's fixtures never use
BetaBinomial, #95's distributional tests are all time-homogeneous. The
structural cures, in order of cleanliness for _our_ regressions:

1. **Compile-coverage** (gh#176 literate-doctest) — every language feature
   compiles. Catches expander/compiler bugs (the #160 class).
2. **FD-gradient-coverage meta-test** (filed) — every (likelihood × projection ×
   estimated-param-routing) gradient arm has an FD test with a param actually
   routed through it; auto-assert a new arm fails until tested. Catches
   silent-zero gradients (the #76 class). Would have caught BetaBinomial at CI,
   no runtime cost.
3. **#78 `--check-grads`** (user-FD at runtime) — the lighter net for
   un-enumerated feature _combinations_ in user models. Complementary, and lower
   priority than (2).

## Stale-build hazards (a recurring tax)

Three instances this week, same family:

- release binary not rebuilt by `make test` (gh#178, fixed: `test-rust` →
  `build-rust`).
- generated `ir_version_generated.ml` not rebuilt (gh#178, fixed: `test-ocaml` →
  `build-ocaml`).
- incremental `cargo test -p sim --test X` linking a stale `sim` rlib (an
  agent's σ² tests went falsely-RED, then green after a forced rebuild).

Lesson: gate with a clean `make test`, never incremental per-crate `cargo test`.

## Decisions the maintainer owns

- **#20** — sign off the σ² density-gradient derivation (verified term-by-term +
  bug-injection; scipy value anchors being added), then close.
- **#76** — closeable as filed; BetaBinomial implementing; `DerivedExpr`
  chain-rule + gate/no-op-coupling hardening are follow-ups.
- **#95** — the sampling algorithm (Lewis–Ogata thinning + a _sound_
  non-monotone-rate bound) — **next morning**.
- **#78** — six small API calls (flag name, tolerance, error-vs-warn,
  init-vs-sample, on-by-default, pfilter scope), and whether the
  FD-gradient-coverage meta-test demotes it.
- **#128** — construction-site hard-error ± `ir::validate` (#123).

## Next

BetaBinomial gradient in flight (worktree; scipy + FD; gate inversion). #95
proposal in the morning. #78 + #128 are small defensive adds once the API and
placement calls are made.
