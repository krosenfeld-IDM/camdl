# Correlated PF returns -inf where the plain bootstrap PF is finite

Date: 2026-06-08
Project: camdl
Tags: inference, particle-filter, correlated-pf, pmmh, cpm
Status: bug, reproduced — localized but not yet root-caused in the CPM code.
Related: #17 (correlated_pf variable-noise-layout / CPM silent-decorrelation).

## Summary

The **correlated particle filter** (the `rho > 0` pseudo-marginal path used by
PMMH) returns `final_loglik = -inf` on a sharp, high-count NegBinomial
likelihood where the **plain bootstrap PF** (`rho = 0`) is finite — same model,
data, parameters, particles, seed, everything. Surfaced while investigating the
pre-existing `profile_pmmh_log_posterior_includes_focal_and_nuisance_uniform_priors`
red; that red was the *symptom*, this is the cause.

**Blast radius is narrow:** only PMMH *with correlation* (`--pmmh-rho > 0`),
which is the experimental/gated method. The production paths — IF2, PGAS, and
the plain bootstrap PF — are unaffected (verified: bare `pfilter` is finite
across the whole estimation box).

## Reproduction (verified)

Model `ocaml/golden/seir_observations.ir.json`, scenario `baseline` (β=0.3,
σ=0.2, γ=0.1, ρ=0.5, k=5, N0=1e5, I0=10); data = `simulate … --seed 42`
(weekly_cases, NegBinomial mean=ρ·incidence, peak ≈ 12k cases). Profile-PMMH,
sweep β=lin(0.295,0.305,2), 100 particles, 120 steps, seed 1 — **only `--pmmh-rho`
varied**:

```
pmmh-rho = 0.0   (plain bootstrap PF)  -> final_loglik = -177.30 / -177.61   FINITE
pmmh-rho = 0.5   (correlated PF)       -> final_loglik = -inf
pmmh-rho = 0.99  (correlated PF)       -> final_loglik = -inf
```

Corroborating, the bare plain PF is finite everywhere PMMH would explore
(`camdl pfilter … --params <θ> --data weekly_cases.tsv --obs weekly_cases`,
no `--scenario` — see footgun below):

```
β: 0.20→-476.7  0.28..0.32→≈-178..-180  (0.50→-inf, outside the box)
σ (at β=0.3): 0.10→-287.9  0.15→-192.1  0.20→-179.6  0.25→-179.5  0.35→-192.4
```

All finite across `[0.28,0.32] × [0.10,0.35]`. So the `-inf` is neither the PF
nor the incidence projection (the forward sim and plain PF both compute
`cumulative_flow(infection)` correctly); it is the **correlation machinery**.

## Hypothesis (not yet root-caused)

The Correlated Pseudo-Marginal (CPM) reuses a fixed block of underlying random
numbers across PMMH proposals to correlate successive PF runs. On a sharp,
high-count likelihood the per-window noise layout / resampling-coupling is the
suspect — consistent with #17 (variable-noise-layout, the `if noise_idx < len`
silent fresh-RNG fallback). A single window whose correlated draws push every
particle's predicted mean to 0 at a `y>0` week yields `-inf`, and unlike the
plain PF the correlated structure can make that systematic rather than rare.
Needs a trace of the per-window CPM weights vs the plain PF on this model.

## Disposition

- The `profile_pmmh` test now runs at `rho = 0.0` (plain PF) — the prior-sum
  invariant it pins is filter-agnostic, so this is honest, not a workaround;
  it points here.
- The correlated-PF fix is inference-math (high-risk) and PMMH is experimental
  — defer to a focused session against #17, with a CPM-vs-plain-PF per-window
  weight trace as the first diagnostic.

## Separate footgun found (file alongside)

`camdl pfilter --scenario <S> --params <FILE>` — the scenario's parameter values
**silently override `--params`** (the loglik was byte-constant across wildly
different `--params` until `--scenario` was dropped). A user pinning θ via
`--params` while passing `--scenario` is filtering at the scenario's θ, not
theirs — a silent wrong-θ likelihood. Worth a hard error on the conflict (or
documented precedence + a warning), mirroring how `--scenario` already conflicts
with `--enable/--disable`.

## Next

1. Trace CPM per-window weights vs the plain PF at β=0.30 on this model (find
   the window that goes -inf).
2. Cross-check against #17's noise-layout fix.
3. Decide the `--scenario`/`--params` precedence (hard error vs documented win).
