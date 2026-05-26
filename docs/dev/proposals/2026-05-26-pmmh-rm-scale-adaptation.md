# Adaptive PMMH proposal scale via Robbins-Monro acceptance-rate tuning

Date: 2026-05-26
Author: vsb
Status: draft for review
Related: gh#74 Option A (warm-chain — deferred but complementary),
         gh#89 (profile cache key — closed), gh#109 (log_posterior in
         profile output — closed)

## Class

**code-vs-code**: the existing PMMH adaptive machinery (Haario AM via
empirical covariance + Cholesky, `rust/crates/sim/src/inference/pmmh.rs`
lines 121–228) is correct as far as it goes but is *insufficient* for
the failure mode hit by profile boundary cells: when initial proposal
scale is grossly mismatched to the local basin width, acceptance
collapses below 1% and the empirical covariance never accumulates
enough accepted samples to refine the scale. The fix is to add a
companion mechanism — Robbins-Monro acceptance-rate-targeted scale
tuning — that operates from the *first* step and shrinks/grows the
proposal scale until acceptance lands in the workable range, **before**
the covariance learner has anything to learn from.

## TL;DR

`AdaptiveProposal` learns the *shape* of the proposal (per-parameter
relative scale + cross-component correlation) via empirical
covariance once enough accepted samples exist. It does **not** tune
the *overall scale* — that's stuck at the user-supplied or
`--rw-sd auto`-derived `proposal_sd`. When that initial scale is
off by an order of magnitude (e.g. profile boundary cells whose
basin width is data-driven and unknowable from prior bounds),
acceptance never recovers.

This proposal adds Robbins-Monro (Andrieu & Thoms 2008 §3.2; Roberts
& Rosenthal 2009 §3) scalar scale tuning that:

- coexists with the existing `AdaptiveProposal` Cholesky machinery
- updates a single multiplicative scalar `λ` per step toward a target
  acceptance rate (default α* = 0.234 per Roberts-Gelman-Gilks 1997)
- uses a vanishing step size `γ_t = c · (t + t₀)^{-η}` so adaptation
  *settles* (Roberts & Rosenthal 2009 §3 — without vanishing γ the
  adaptation never converges and breaks the diminishing-adaptation
  ergodicity guarantee of Roberts & Rosenthal 2007)
- adapts during a user-tunable adaptation window then locks for the
  reported chain (preserving MH stationarity by exiting the adaptive
  regime before recording samples)

Cost: ~50–80 LOC in `pmmh.rs`, mostly additive (new fields on
`PMMHConfig` and `AdaptiveProposal`, one new method, a few lines in
the inner loop). No change to the MH acceptance ratio computation
itself — RM only modifies the proposal *kernel*, which is the
appropriate place to adapt.

## Why the existing adaptation is insufficient

Verified at `pmmh.rs:121-228`. `AdaptiveProposal::update` (line 152)
maintains Welford-style running mean + M₂. Once `n > d` AND
`steps_since_chol ≥ chol_interval=100`, `update_cholesky` computes:

```
Σ_prop = (2.38² / d) · Σ̂ + ε·I        where Σ̂ = M₂ / (n - 1)
L = chol(Σ_prop)                       (lower-triangular)
```

Proposals draw `Δ = L · z`, `z ~ N(0, I)`. Until Cholesky is valid,
falls back to the diagonal `proposal_sd[i] · z_i` (line 226).

**This is the canonical Adaptive Metropolis (AM) of Haario, Saksman,
Tamminen 2001.** It does two things well:

1. Learns *relative* per-component scales via the diagonal of Σ̂.
2. Learns cross-component correlations via the off-diagonal.

It does **not** tune the *overall* scale: the `2.38² / d` factor is
the asymptotic Gelman-Roberts-Gilks 1996 optimum *for a Gaussian
target*, and the proposal is anchored at the user-supplied
`proposal_sd` until enough samples accumulate to overwrite that
diagonal.

The chicken-and-egg the downstream agent hit at boundary cells:

- Initial `proposal_sd ≈ prior_bounds / 6` (auto rule).
- Local conditional basin width at τ near the data window: ~1–2 days.
- Prior bounds for τ: 85 days. → Initial scale ≈ 10× too coarse.
- Acceptance at step 0 ≈ 0.5% (verified in their incident).
- After 5000 steps at 0.5% acceptance: ~25 accepted samples.
- Empirical covariance over 25 samples in 9-d space: noise-dominated.
- Cholesky doesn't trip the validity check, or trips it on a noise
  estimate that doesn't help. Stay on the diagonal fallback. Acceptance
  stays at 0.5%. **Stuck forever within the budget.**

The fix is structural: we need a separate adaptation that can drive
the scale from a coarse initial guess to a workable range *using
rejections as well as acceptances*, before Cholesky has anything to
say.

## The Robbins-Monro update

### Mathematics

Define:

- `α*` ∈ (0, 1) — target acceptance rate (default 0.234 for d ≥ 2,
  0.44 for d = 1 per Roberts-Gelman-Gilks 1997)
- `a_t ∈ {0, 1}` — acceptance indicator at MCMC step `t`
- `λ_t > 0` — scalar multiplier on the proposal kernel at step `t`
- `γ_t > 0` — Robbins-Monro step size at step `t`

The update rule is:

```
log λ_{t+1} = log λ_t + γ_t · (a_t - α*)             … (1)
```

Each rejection (`a_t = 0`) decreases `log λ` by `γ_t · α*`.
Each acceptance (`a_t = 1`) increases `log λ` by `γ_t · (1 - α*)`.
At steady state the expected update is zero ⇔ `E[a_t] = α*` ⇔ the
chain accepts at the target rate.

The proposal kernel uses `λ_t` multiplicatively:

```
Δ_t = λ_t · K_t · z_t                              … (2)
```

where `K_t` is the existing kernel (`diag(proposal_sd)` while Cholesky
is invalid, `L_t` once it's been updated), and `z_t ~ N(0, I)`.
Effectively `λ_t` re-scales whatever the existing machinery proposed.

### Vanishing step size (load-bearing)

The standard recipe (Andrieu-Thoms 2008 §2.1; Roberts-Rosenthal 2009
§3) is:

```
γ_t = c · (t + t₀)^{-η}                            … (3)
```

with `η ∈ (1/2, 1]` and constants `c > 0`, `t₀ ≥ 0` tuned for the
problem. **The vanishing property `γ_t → 0` is required**, not
optional: a constant step size would make the adaptation oscillate
around the target indefinitely and would break the
diminishing-adaptation ergodicity theorem of Roberts & Rosenthal 2007
(Theorem 1, "containment + diminishing adaptation"). Without it, the
adaptive chain does not target the right stationary distribution
asymptotically.

Defaults proposed:

- `η = 0.6` (mid-range; Andrieu-Thoms 2008 footnote 5 suggests
  η ≈ 0.6–0.8 in practice; values closer to 1 adapt too slowly and
  closer to 1/2 are minimally damped).
- `t₀ = 50` (matches the existing `adapt_start` default; γ_t at
  t = adapt_start is c · 100^{-0.6} ≈ 0.063 · c — a modest first step).
- `c = 1.0` (unit log-step under maximum signal; combined with α* =
  0.234, the per-rejection log-decrement is ≤ 0.234/50^{0.6} ≈ 0.018,
  i.e. ≤ 1.8% multiplicative shrink per rejection at the start of
  adaptation, dropping to ~0.5% by step 1000).

These defaults are conservative: a chain with starting acceptance
0.005 needs `log λ` to shrink by roughly `log(10) ≈ 2.3` to bring
acceptance into the 0.15–0.5 zone. At average rejection rate 0.995
and an effective γ ≈ 0.05 early on, the per-step log-decrement is
≈ 0.05 · 0.234 ≈ 0.012; we need ~190 steps of pure rejection to
shrink λ by 10×. That fits comfortably in a 500–1000 step burn-in
on the boundary cells where this matters most.

### Target acceptance rate

Roberts-Gelman-Gilks 1997 establish 0.234 as the asymptotically
optimal acceptance rate for *random-walk Metropolis on a Gaussian
target as d → ∞*. The 0.44 figure that some texts quote is the
1-dimensional optimum (Roberts-Rosenthal 2001). Profile-PMMH on
seed-timing has ~9 estimated params per cell; 0.234 is the right
default. The 1-d branch matters for the rare case of a profile
where the user has pinned all-but-one estimated parameter.

The proposed default is dimension-keyed:

```
α*_default(d) = 0.234 if d ≥ 5 else (0.4 + 0.2 · (5-d)/4)
                                ^                ^
                              5-d=0 → 0.4      5-d=4 (d=1) → 0.6
```

Hmm, that interpolation overshoots — fix to a stepwise table per the
Roberts-Rosenthal asymptotic-vs-1d distinction:

```
α*_default(d) = 0.234 if d ≥ 5 else 0.44                  … (4)
```

i.e. flip at d = 5 (matches the threshold beyond which 0.234 is
within 5% of the true optimum per the original Roberts-Gelman-Gilks
table). Override via config.

## Integration with the existing `AdaptiveProposal`

The clean composition is **multiplicative**: the existing kernel
(diagonal fallback OR Cholesky factor) provides the *shape*; the new
λ provides the *scale*. At any step `t` ≥ `adapt_start`:

```rust
// Existing kernel sample (line 211-228 of pmmh.rs, unchanged in shape).
let unscaled_delta = if self.chol_valid {
    cholesky_mul(&self.chol, &z, d)
} else {
    z.iter().zip(fallback_sd).map(|(z_i, sd)| z_i * sd).collect()
};
// New: scale by current λ (line added by this proposal).
let delta: Vec<f64> = unscaled_delta.iter()
    .map(|d_i| self.log_scale.exp() * d_i)
    .collect();
```

And after the accept/reject decision:

```rust
if step >= config.adapt_start && step < config.adapt_start + config.adapt_n {
    let gamma_t = config.rm_c
        * ((step - config.adapt_start + config.rm_t0) as f64).powf(-config.rm_eta);
    let accept_indicator = if accepted { 1.0 } else { 0.0 };
    self.log_scale += gamma_t * (accept_indicator - config.target_acc);
}
```

Three properties of this composition:

1. **The Cholesky learns on samples that were proposed with the
   current λ.** That means `Σ̂` ends up encoding the *true* basin shape
   on the *post-RM* scale. When λ has shrunk by 10×, the empirical
   covariance reflects local geometry, not the prior-bounds-derived
   coarse initial geometry. The two adaptations cooperate.
2. **The 2.38²/d Gelman-Roberts-Gilks factor stays in the Cholesky
   path** (existing code at `update_cholesky`). RM's λ is an
   *additional* multiplicative correction. At the asymptotic optimum
   the empirical covariance plus 2.38²/d delivers near-optimal
   proposals; λ converges to 1 in expectation. Off-optimum, λ pulls
   the overall scale toward acceptance ≈ α*.
3. **Diminishing adaptation is preserved.** Both the covariance and
   λ updates use cadences that decrease in influence with `t`
   (Welford automatically diminishes; RM via γ_t). Roberts & Rosenthal
   2007's containment + diminishing-adaptation conditions are
   satisfied.

## Design decisions

The downstream-agent review of the v1 sketch (2026-05-26) raised
five sharp implementation specifics. Each is acted on below; where I
diverge I say why.

### D1. Vanishing step size

**Adopted as proposed.** Equation (3) above with defaults
`c = 1.0`, `t₀ = 50`, `η = 0.6`. Without vanishing γ the chain does
not target the right stationary distribution.

### D2. Per-component vs global scalar

**Recommendation: single global scalar λ.** The downstream agent
asked for "per-component RM (each θ_i has its own σ_i, each updated
by its own marginal acceptance signal)." Two reasons I'd ship the
global form for v1 instead:

- In Metropolis-Hastings, **acceptance is a joint event**: every
  proposal is one vector and one accept/reject decision. There is
  no per-component "marginal acceptance signal" without changing the
  proposal structure to single-component MH (one coordinate per
  step), which is a different algorithm than the current PMMH (and
  would slow each sweep by a factor of d).
- The shape/relative-scale concern (different SEIR params having
  different natural scales) is **already addressed** by the existing
  `AdaptiveProposal`'s empirical covariance. Σ̂'s diagonal entries
  give per-component variance; off-diagonals give the cross-component
  structure. The 2.38²/d factor + the empirical Σ̂ together
  asymptotically produce per-component proposals that match the
  target's local geometry. A separate per-component RM scalar would
  duplicate work that the Cholesky machinery already does.

What a per-component RM *would* add is a faster correction to scale
*before* Σ̂ has converged. But that's exactly the situation where
acceptance is too low for per-component to gather enough signal per
component — and the joint accept gives us *one* bit of information
per step regardless of how we partition it across components.

**Verdict**: ship global scalar λ in v1; revisit per-component or
the Vihola 2012 "Robust Adaptive Metropolis" rank-1 covariance
update if real models in flight need finer-grained adaptation.
Document the choice in `pmmh.rs` so a future reader sees the
rationale.

### D3. Target acceptance 0.234, not 0.44

**Adopted as proposed.** Equation (4) above; dimension-keyed default
with override.

### D4. Correlated PF noise (ρ > 0)

The downstream agent flagged that under `--pmmh-rho 0.99` (correlated
pseudo-marginal), the PF noise correlates across MCMC steps, so the
instantaneous accept indicator is a noisier RM signal than for
plain MCMC. They suggested a moving-window acceptance estimate
(last ~200 steps).

**Recommendation: use the instantaneous signal in v1; flag the
windowed-mean variant as a v2 if it's a problem in practice.**
Reasoning:

- The vanishing γ_t already provides averaging — early steps have
  large γ but their impact diminishes; later steps have small γ that
  smooths over local noise. Adding a moving-window mean is double
  damping.
- The standard RM literature (Andrieu-Thoms, Roberts-Rosenthal) uses
  the instantaneous signal precisely because the convergence proofs
  rely on it being a martingale-difference noise around the target.
  Replacing with a moving mean changes the stochastic-approximation
  noise structure.
- If the windowed estimate is needed in practice, it's a one-line
  swap: maintain a `VecDeque<bool>` of the last `W` accept
  indicators and use the mean. Defer.

### D5. Burn-in / adaptation window length

**Adopted with API extension.** Add `adapt_n: usize` to `PMMHConfig`
— the number of steps over which RM adapts before locking. Default
to `max(burn_in, 200)` since the worst-case basins need at least a
few hundred steps to shrink λ by 10×.

**Important separation of concerns**: `adapt_n` is the *adaptation
window* (RM + covariance updates allowed). `burn_in` is the
*recorded-sample window* (steps to discard from output). These are
independent in principle. Stan and PyMC distinguish them. Today's
PMMH conflates: adaptation ends at `n_steps` (always on),
burn_in defines what's discarded. Cleanest mental model:

- `adapt_n` ≥ `burn_in`: adapt during burn-in *and* part of the
  sampling phase (acceptable for diminishing γ; preserves the
  Roberts-Rosenthal 2007 ergodicity argument).
- `adapt_n` = `burn_in`: adapt during burn-in only, lock for the
  sample (the conservative choice that most users expect).
- `adapt_n` < `burn_in`: adapt for a portion of burn-in, lock for
  the rest of burn-in *and* the sample (useful for very short
  chains where you want a long stationary lock).

Recommend `adapt_n = burn_in` as the default to minimise surprises;
document that increasing `adapt_n` past `burn_in` is acceptable when
diminishing γ is in effect (which it always is under (3)).

## Companion: explicit rw_sd override in fit.toml (separate, smaller)

A second concern the downstream agent raised — "let users override
rw_sd in fit.toml" — is a separate, smaller change that does **not**
need its own proposal. The intent:

```toml
[stages.profile_pmmh]
# Default behaviour (unchanged): auto-derive from bounds.
rw_sd = "auto"

# OR: explicit per-param overrides (sensitivity studies, paper
# replication, ablation).
[stages.profile_pmmh.rw_sd]
tau   = 1.0
beta  = 0.05
```

Implementation: ~20 LOC in `fit/config_v2.rs` for the new toml shape;
plumbing through the existing rw_sd resolution path. Ships
independently of this proposal.

**The project rule still applies**: "never set rw_sd manually unless
there's a documented reason." The override is for paper replication,
sensitivity studies, and ablation experiments — *not* a workaround
for poor mixing. Mixing problems are what this proposal's RM tuning
solves.

## Types and code surface

### Existing `PMMHConfig` (pmmh.rs:28–46)

```rust
pub struct PMMHConfig {
    pub n_steps: usize,
    pub n_particles: usize,
    pub dt: f64,
    pub proposal_sd: Vec<f64>,
    pub adapt: bool,
    pub adapt_start: usize,
    pub thin: usize,
    pub burn_in: usize,
    pub rho: Option<f64>,
    pub n_source_groups: usize,
}
```

### Extended `PMMHConfig`

```rust
pub struct PMMHConfig {
    // Unchanged fields …
    pub n_steps: usize,
    pub n_particles: usize,
    pub dt: f64,
    pub proposal_sd: Vec<f64>,
    pub adapt: bool,
    pub adapt_start: usize,
    pub thin: usize,
    pub burn_in: usize,
    pub rho: Option<f64>,
    pub n_source_groups: usize,

    // ── New: RM scale adaptation knobs. All have sensible defaults
    //         via a builder; no caller needs to set these explicitly.
    /// Target acceptance rate. `None` → dimension-keyed default
    /// (0.234 for d ≥ 5, 0.44 for d < 5). Override only for
    /// sensitivity studies or paper replication.
    pub target_acc: Option<f64>,

    /// RM coefficient `c` in γ_t = c · (t + t₀)^{-η}.
    pub rm_c: f64,            // default 1.0

    /// RM offset `t₀` in γ_t = c · (t + t₀)^{-η}. Numerical guard
    /// against division-by-zero at t = 0 plus a starting-scale
    /// tuning knob.
    pub rm_t0: f64,           // default 50.0

    /// RM exponent `η` in γ_t = c · (t + t₀)^{-η}.
    /// MUST be in (0.5, 1] for ergodicity (Roberts-Rosenthal 2007).
    pub rm_eta: f64,          // default 0.6

    /// Number of steps over which to run the RM + covariance
    /// adaptation. Steps past this lock both. Default
    /// `max(burn_in, 200)`. Must satisfy `adapt_n ≤ n_steps`.
    pub adapt_n: usize,
}
```

### Extended `AdaptiveProposal` (pmmh.rs:121–135)

```rust
pub struct AdaptiveProposal {
    // Unchanged Welford / Cholesky machinery.
    d: usize,
    n: usize,
    mean: Vec<f64>,
    m2: Vec<f64>,
    chol: Vec<f64>,
    chol_interval: usize,
    steps_since_chol: usize,
    chol_valid: bool,

    // ── New: Robbins-Monro scale tuning.
    /// log of the scalar multiplier λ in Δ = λ · K · z. Initial 0
    /// (λ = 1, no scaling).
    log_scale: f64,
    /// Cached `target_acc` for the RM update step. Resolved from
    /// `PMMHConfig.target_acc` at construction.
    target_acc: f64,
}
```

### New method on `AdaptiveProposal`

```rust
impl AdaptiveProposal {
    /// Robbins-Monro update on the scalar log-scale `log_scale`.
    /// Call after every accept/reject decision while
    /// `step ∈ [adapt_start, adapt_start + adapt_n)`. Outside that
    /// window, do not call — the chain runs in fixed-kernel mode.
    ///
    /// Math:
    ///   γ_t  = rm_c · (rm_steps + rm_t0)^{-rm_eta}
    ///   Δlog λ = γ_t · (accepted ? 1 - α* : -α*)
    ///         = γ_t · (a_t - α*)
    ///
    /// Effect on the proposal: at the next sample_perturbation call,
    /// Δ scales by exp(log_scale).
    fn rm_update(
        &mut self,
        accepted: bool,
        rm_steps: usize,
        rm_c: f64,
        rm_t0: f64,
        rm_eta: f64,
    ) {
        let gamma = rm_c * (rm_steps as f64 + rm_t0).powf(-rm_eta);
        let a_t = if accepted { 1.0 } else { 0.0 };
        self.log_scale += gamma * (a_t - self.target_acc);
    }
}
```

### Changes to `sample_perturbation` (pmmh.rs:211–228)

```rust
fn sample_perturbation(&self, rng: &mut StatefulRng, fallback_sd: &[f64]) -> Vec<f64> {
    let d = self.d;
    let z: Vec<f64> = (0..d).map(|_| rng.normal()).collect();
    let lambda = self.log_scale.exp();   // ← NEW: RM multiplier

    if self.chol_valid {
        let mut delta = vec![0.0; d];
        for i in 0..d {
            for j in 0..=i {
                delta[i] += self.chol[i * d + j] * z[j];
            }
            delta[i] *= lambda;          // ← NEW
        }
        delta
    } else {
        z.iter().zip(fallback_sd)
            .map(|(&zi, &sd)| lambda * zi * sd)  // ← NEW
            .collect()
    }
}
```

### Changes to the inner loop (pmmh.rs:397–490)

The existing structure already has:
- `ap.update(&current_transformed)` on every step (line 490)

Add immediately after the accept/reject decision (line 470 area):
```rust
// gh#74-related: RM scale tuning while in the adaptation window.
if config.adapt && step >= config.adapt_start
   && step < config.adapt_start + config.adapt_n {
    if let Some(ref mut ap) = adaptive {
        ap.rm_update(
            accepted,
            step - config.adapt_start,
            config.rm_c,
            config.rm_t0,
            config.rm_eta,
        );
    }
}
```

That's it for the algorithm. The profile/fit-run callers don't
change; the new `PMMHConfig` fields all have defaults and the
behaviour preserves the existing flow when adaptation is disabled
or `adapt_n = 0`.

## Tests (TDD, in order)

1. **RM converges to target on a known Gaussian target.** Construct
   a closed-form unit Gaussian target, run PMMH with initial
   `proposal_sd` 10× too coarse, with adapt_n = 5000. Assert that
   the mean acceptance rate over the last 1000 steps is in
   `[α* - 0.05, α* + 0.05]`. This is the headline regression test.

2. **Vanishing γ asymptote.** After adapt_n steps, λ has stopped
   changing materially (max single-step Δlog λ in the last 100 steps
   < 1e-3). Pins (3)'s diminishing property.

3. **Diminishing-adaptation property (sanity).** Two PMMH runs with
   the same seed but `adapt_n=0` vs `adapt_n=1000`: after
   `n_steps = 10000`, the post-burn-in chains have indistinguishable
   sample moments (within Monte Carlo error). Pins the
   Roberts-Rosenthal 2007 "adaptation is innocuous" requirement.

4. **Boundary-cell regression (the downstream-agent incident).** A
   synthetic SEIR with a sharp likelihood, profile-tau at a boundary
   cell where `auto` rw_sd is 10× too coarse. Without RM: acceptance
   < 5%, R̂ across starts > 3. With RM: acceptance ∈ [0.15, 0.5],
   R̂ < 1.1. Slow integration test (one cell, marked
   `#[ignore]` by default; CI runs on-demand).

5. **Cache key extension.** Two `ProfileInputs` differing only in
   any RM knob (`target_acc`, `rm_eta`, `adapt_n`) must hash to
   different `inner_hash`. Same shape as the gh#89 / gh#90 cache-key
   tests. Pre-emptive: changing adaptation knobs is a content
   distinction.

6. **`adapt_n > n_steps` rejected at config-load**, with the
   actionable error pointing at the inequality.

7. **`rm_eta ∈ (0.5, 1]` enforced at config-load**, with a one-line
   reference to Roberts-Rosenthal 2007 in the error text. This is
   the load-bearing ergodicity constraint and a hard error is
   cheaper than a subtle wrong-stationary-distribution bug.

## Implementation outline

Suggested commit sequence (each green-tested in isolation):

1. Add new fields to `PMMHConfig` + `AdaptiveProposal` with defaults
   that reproduce *exactly* the current behaviour (target_acc inert
   when `adapt_n = 0`). Test: existing PMMH tests still pass
   bit-identically.
2. Add `rm_update` method + `log_scale` multiplier in
   `sample_perturbation`. Still inert (`adapt_n = 0`). Add the unit
   test that `log_scale.exp() = 1` after construction.
3. Wire the inner-loop call site. Default `adapt_n = burn_in`. Run
   the closed-form Gaussian convergence test (test #1 above) RED →
   GREEN. The RED proof in the commit message is the headline
   result.
4. Enforce config invariants (`rm_eta` range; `adapt_n ≤ n_steps`).
   Add tests #6, #7.
5. Surface in `profile.rs` + `fit/runner.rs` PMMHConfig
   construction (default-construct the new fields; no caller changes
   needed if defaults are right). Add the boundary-cell integration
   test (#4) gated on `#[ignore]`.
6. Profile cache-key extension (test #5).
7. fit.toml override for rw_sd (the companion §). Separate commit
   with its own test.

Total: ~150 LOC of code + ~250 LOC of tests. ~1.5 hours for an
agent.

## Migration / cache impact

This proposal changes the **proposal kernel** (Δ now has an
adapt-time-varying scalar multiplier). For chains drawn with
identical seeds before and after this change, the trajectory will
differ. Specifically:

- The MAP found within `n_steps` may differ (RM should find better
  MAPs on average for off-scale initial proposals).
- `acceptance_rate` will trend toward `target_acc` rather than
  whatever the initial scale produced.
- The empirical covariance learned by Cholesky will be the
  *post-RM* covariance, which is a different (correct) estimate
  than the *pre-RM* covariance.

**Cache invalidation**: existing cached fit/profile dirs will
re-compute on first run after this lands. This is the desired
behaviour — pre-fix cache entries reflect a different proposal
kernel and shouldn't be trusted to represent the new chain
statistics. Same shape as the gh#89 cache invalidation.

## References

- Andrieu, C. & Thoms, J. (2008). "A tutorial on adaptive MCMC."
  *Statistics and Computing* 18(4): 343–373. §3.2 is the canonical
  reference for scalar RM tuning on log-scale.
- Roberts, G. O. & Rosenthal, J. S. (2007). "Coupling and ergodicity
  of adaptive Markov chain Monte Carlo algorithms."
  *Journal of Applied Probability* 44(2): 458–475. The diminishing-
  adaptation + containment theorem (Thm 1) is what makes equation
  (3)'s vanishing γ load-bearing.
- Roberts, G. O. & Rosenthal, J. S. (2009). "Examples of adaptive
  MCMC." *Journal of Computational and Graphical Statistics* 18(2):
  349–367. §3 contains worked examples with γ_t = (t + 1)^{-η}
  including the convergence diagnostics this proposal's tests
  mirror.
- Roberts, G. O., Gelman, A. & Gilks, W. R. (1997). "Weak
  convergence and optimal scaling of random walk Metropolis
  algorithms." *Annals of Applied Probability* 7(1): 110–120. The
  0.234 asymptotic optimum target.
- Roberts, G. O. & Rosenthal, J. S. (2001). "Optimal scaling for
  various Metropolis-Hastings algorithms." *Statistical Science*
  16(4): 351–367. The 0.44 one-dimensional optimum.
- Haario, H., Saksman, E., Tamminen, J. (2001). "An adaptive
  Metropolis algorithm." *Bernoulli* 7(2): 223–242. Source of the
  existing `AdaptiveProposal` covariance machinery; predates the
  scale-tuning extension this proposal adds.
- Andrieu, C. & Vihola, M. (2014). "Markovian stochastic
  approximation with expanding projections." *Bernoulli* 20(2):
  545–585. Adaptive PMMH specifically; the theoretical justification
  for RM tuning applied to pseudo-marginal chains, which is the
  setting here.
- Vihola, M. (2012). "Robust adaptive Metropolis algorithm with
  coerced acceptance rate." *Statistics and Computing* 22(5):
  997–1008. The rank-1 covariance-update alternative ("RAM"); cited
  as a v2 option if global-scalar RM proves insufficient.

## Acceptance

This proposal is approved when:

- The math (§"The Robbins-Monro update") is acknowledged as
  correctly describing the standard recipe, and the cited papers
  are recognised as load-bearing.
- The integration design (§"Integration with the existing
  `AdaptiveProposal`") is accepted: multiplicative composition with
  the Cholesky kernel, not replacement.
- The "global scalar λ, not per-component" choice for v1 (§D2) is
  accepted, with the explicit Vihola 2012 RAM fallback flagged for
  v2 if needed.
- The companion rw_sd-override path (§"Companion") is accepted as a
  *separate* feature shipped independently, not as the fix for the
  boundary-cell mixing problem.
- The diminishing-γ enforcement at config-load (§"Tests" #7) is
  accepted as a hard error, not a warning.
