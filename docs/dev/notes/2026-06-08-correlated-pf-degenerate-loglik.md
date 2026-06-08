# Correlated PF returns -inf where the plain bootstrap PF is finite

Date: 2026-06-08
Project: camdl
Tags: inference, particle-filter, correlated-pf, pmmh, cpm
Status: FIXED (2026-06-08). Not a numerical degeneracy in the CPM draws — a
swallowed structural validation error. Root cause + fix below ("Fix (landed)").
GH: gh#193 (this bug); gh#194 (the `--scenario`/`--params` footgun below).
Related work: correlated_pf variable-noise-layout / CPM silent-decorrelation.

## Root cause (confirmed)

The "Hypothesis" section below (per-window noise degeneracy) is **wrong**. The
`-inf` is a structural validation error that profile/PMMH swallows into
`-inf`, with two independent links:

1. **The obs grid starts at t=0.** `weekly_cases` is `regular start=0 step=7`,
   so the observation times are `[0, 7, 14, …]`. The FIRST correlated-PF window
   `[t_start=0, obs(0)=0]` is **zero-substep**. The plain bootstrap PF handles
   this fine — it scores the t=0 observation at the initial state (negbin_logpmf
   contributes ≈0) — but the CPM uniform-window gate
   (`correlated_pf.rs:202-233`) requires every window, the first included, to
   have exactly `steps_per_obs` substeps, so it returns
   `SimError::Validation("…the FIRST window […] has 0 substep(s)… Drop to
   vanilla PMMH (rho = None), or align the observation grid…")`.

2. **profile.rs swallows that Err into -inf.** The correlated-eval closure at
   `rust/crates/cli/src/profile.rs:1388-1389` is
   `match bootstrap_filter_correlated(…) { Ok(r) => r.log_likelihood, Err(_) =>
   f64::NEG_INFINITY }`. The gate's actionable message is discarded; every
   PMMH step returns `-inf` → `acc_rate = 0`, `final_loglik = -inf`,
   `loglik_trace = [-inf; …]` from step 0 (the initial fresh-noise eval is
   already -inf, which rules out `correlate()`).

Because the profile default is `--pmmh-rho 0.99`, this hits **every**
regular-schedule model whose obs grid starts at t=0 — i.e. the common case.

### Proof (verified, `tests/correlated_pf_finite.rs::diagnose_gh193_obs_starts_at_t0`)

Golden SEIR process + NB incidence obs on the weekly grid PREPENDED with the
t=0 observation (the real `start=0` schedule):

```
plain PF on t=0-starting grid: loglik = -189.596…            (FINITE)
correlated PF on t=0-starting grid: Err = …the FIRST window
  [t_start=0.0000, obs(0)=0.0000] has 0 substep(s)…          (gate rejects)
```

On the SAME process+obs WITHOUT the leading t=0 (grid `[7,14,…]`), CPM is
finite and within MC distance of the plain PF (`-190.11` vs `-189.67`), so the
draw machinery is sound. The discriminator is solely the zero-width first
window + the swallowed error.

## Proposed fix (two parts)

- **Primary — handle the leading zero-width window.** When `obs(0) == t_start`
  the first window consumes no noise (`gamma_noise[0]` is sized but untouched;
  windows 1..n index their own `gamma_noise[obs_idx]`), so the existing
  indexing is already safe. Allow it in the gate and score the t=0 obs at the
  initial state, exactly as the plain PF does. This makes CPM work on the
  default `regular start=0` grid.
- **Safety net — never swallow a structural `SimError::Validation` into -inf.**
  A genuine mid-period non-uniform grid (e.g. obs at `[5,12,19]`) must still
  surface the actionable message (preflight abort), not present as a mysterious
  all-(-inf) profile. `profile.rs:1389` (and any sibling CPM eval closures in
  `fit/pmmh.rs`) is the swallow site.

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
`cumulative_flow(infection)` correctly) — it is the swallowed obs-grid
validation error described under "Root cause (confirmed)" above.

## Fix (landed)

Two commits, TDD-driven:

- **A1 — filter accepts the leading t=0 window** (`sim`,
  `correlated_pf.rs`). Extracted the gate into the shared
  `validate_cpm_obs_grid` and allowed a leading window that coincides with
  `t_start` (it is empty, consumes no noise block, and obs(0) is scored at the
  initial state — bit-for-bit like the plain PF). Keyed on `obs(0) == t_start`
  within `EFFECT_EPS`, **not** `interval_steps == 0`, so a sub-dt offset
  (`obs(0)=0.3`) still errors (the Exact iterator would take a clipped substep
  there and silently mis-size the noise). Catch-test:
  `tests/correlated_pf_finite.rs::correlated_pf_finite_on_t0_starting_grid`
  (RED on the unfixed gate → GREEN: CPM −190.09 vs plain −189.81). End-to-end,
  `camdl profile --pmmh-rho 0.5` on golden `seir_observations` now yields finite
  per-cell logliks (−177.5…−179.9, acc 0.7–0.95) where every cell was
  `final_loglik=-inf` / `acc_rate=0`.

- **B — preflight instead of swallow** (`cli`, `profile.rs` + `fit/pmmh.rs`).
  The CPM obs-grid check is θ-independent, so call `validate_cpm_obs_grid` ONCE
  at profile/fit setup when `rho` is set and abort with the actionable message.
  The in-loop `Err(_) => -inf` is left intact — a genuine θ-specific degenerate
  likelihood still maps to `-inf`/reject, as MCMC requires; only the structural
  obs-grid error surfaces up front. Verified end-to-end: a mid-period grid
  (`[5,12,19,…]`, `t_start=0`) now aborts with "the FIRST window
  [t_start=0.0000, obs(0)=5.0000] has 5 substep(s)… Drop to vanilla PMMH…"
  before any cell runs, instead of a silent all-(-inf) profile.

The `profile_pmmh` invariant test still runs at `rho = 0.0` (plain PF) — the
prior-sum invariant it pins is filter-agnostic, and with A1 the correlated path
is no longer degenerate on the standard grid, so it could be re-enabled at
`rho > 0` if desired (separate change).

## Separate footgun found (filed as gh#194)

`camdl pfilter --scenario <S> --params <FILE>` — the scenario's parameter values
**silently override `--params`** (the loglik was byte-constant across wildly
different `--params` until `--scenario` was dropped). A user pinning θ via
`--params` while passing `--scenario` is filtering at the scenario's θ, not
theirs — a silent wrong-θ likelihood. Worth a hard error on the conflict (or
documented precedence + a warning), mirroring how `--scenario` already conflicts
with `--enable/--disable`.

## Next

1. Decide the `--scenario`/`--params` precedence (gh#194: hard error vs
   documented win).
2. Optional: re-enable the `profile_pmmh` prior-sum invariant at `rho > 0` now
   that the correlated path is non-degenerate on the standard grid.
