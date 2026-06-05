# Proposal: Survival-Conditioned Likelihood for Early-Outbreak Inference

**Status:** draft for discussion. **Scope:** an opt-in correction to the
PF-based likelihood estimators (PMMH, IF2, PGAS) for the selection bias
introduced when fitting stochastic compartmental models to outbreak datasets
that exist _because_ the outbreak survived early extinction. Adds one estimator
(a Monte-Carlo survival probability) and one inference-time log-likelihood
adjustment. Does not change the simulator, the IR, or existing inference math;
touches only the loglik aggregation step. **Primary application:** small-R₀ /
small-seed regimes — single introductions, cVDPV2 emergence, early-outbreak
Ebola — where the branching-process approximation predicts a non-trivial
extinction probability and naïve fitting inflates `R₀` estimates.

> **Provenance.** This proposal grew out of a question raised in conversation:
> should camdl condition the likelihood on the outbreak being observed, and does
> this already happen implicitly through the particle filter? The short answers
> are _yes for small-R₀ data_ and _no, it does not already happen_. The longer
> answers — including what pomp does and doesn't do, and why a Monte-Carlo
> survival estimator fits camdl's architecture better than the closed-form
> Galton–Watson formulas typically used by hand in pomp workflows — are in §3
> and §5 below. The cited literature was verified against primary sources
> (Britton & Pardoux 2019 ch. 1; Lloyd-Smith et al. 2005 supplement; pomp
> documentation), not from memory.

---

## Summary

The bootstrap particle filter underlying PMMH, IF2 and PGAS in camdl estimates
the unconditional likelihood `p(y₁:T | θ)` — the probability of seeing the
observed time series under parameters `θ`, averaged over _all_ latent
trajectories the model can produce, including those that go extinct in the first
few generations. Real outbreak datasets, however, exist because the outbreak
happened; we are implicitly conditioning on the event "the epidemic took off and
produced observable cases." Failing to match the inference target to this
conditioning is a well-known selection bias in stochastic-epidemic inference
(Britton & Pardoux 2019, ch. 1; Diekmann, Heesterbeek & Britton 2013, ch. 3).
The bias is small at high `R₀` and large seed, and `O(1)` on the log-likelihood
at low `R₀` and small seed. Direction: it inflates `R₀` (and similar
take-off-favouring parameters).

The mathematical fix is uncontroversial: divide by the take-off probability,

> `log L_cond(θ) = log L_PF(θ) − log P(take-off | θ).`

The design question is how to compute `P(take-off | θ)` for a heterogeneous
model zoo. Two paths, picked explicitly by the user via a `survival_method`
switch:

- **Analytic Galton–Watson `q(θ)`** for recognized offspring families (Markov
  SIR/SEIR → `q = 1/R₀`; NegBin offspring → closed-form fixed-point; Poisson
  offspring → Newton). Exact, noise-free, zero extra simulation cost; preserves
  PMMH exactness in the Andrieu–Doucet–Holenstein sense. Preferred wherever
  applicable — especially in the low-`R₀` / small-seed regime where MC variance
  bites hardest (§3.2).
- **Monte-Carlo `P̂_M`** as the generic fallback: per-`θ` proposal, run `M`
  independent unconditional forward simulations through the early phase.
  Adaptive `M` with floor 400 and a `5/P̂_pilot` top-up rule. Works for any model
  class without per-family derivation; necessary for spatial coupling,
  stratified mixing, and small-`N` cells where the branching approximation
  degrades.

The feature ships **off by default**. When on, the user picks the conditioning
event (`event = "latent_k1"` for the Galton–Watson default,
`event = "observed_k"` with a user-supplied threshold for operational
surveillance settings — see §3.3); the algorithm (PMMH or PGAS; IF2 is scoped
out of v1, §4.1); the estimator (Anscombe-smoothed plug-in default, or
geometric/inverse-Bernoulli for exact PMMH unbiasedness). A documented trap:
synthetic-recovery pipelines must not double-count — if the data generator
already discards extinct trajectories, the correction is required for unbiased
recovery; if the generator keeps everything, the correction is wrong.

---

## 1. The bias

### 1.1 Setup

We have a compartmental SDE/Markov-chain model with state `X(t)`, parameters
`θ`, and observations `y_t = h(X(t)) + noise` at times `t₁, …, t_T`. The PF
estimator targets

> `L(θ) := p(y₁:T | θ) = E_{X|θ}[∏_t p(y_t | X(t))],`

where the expectation is over the _unconditional_ law of trajectories.
Trajectories that go extinct before observations begin (or that fade out without
producing `y_t > 0` cases) contribute negligibly to this expectation because
`p(y_t > 0 | X(t)=0)` is zero or near-zero under the standard count-data
observation families (Poisson, Negative Binomial). The PF correctly down-weights
these particles at resampling. The estimand `L(θ)`, however, still integrates
over them — their contribution is just small.

### 1.2 The mismatch

Real outbreak datasets are not unconditional draws. They are drawn from

> `p(y₁:T | θ, A),  where  A = {the outbreak took off}.`

The conditional law factorises as

> `p(y | θ, A) = p(y, A | θ) / P(A | θ).`

The identity `p(y, A | θ) = p(y | θ)` holds **iff** `{Y = y} ⊆ A` almost surely
under the model — i.e., every latent trajectory that could have produced `y` is
in `A`. There are two ways to arrange this, and they differ in how `A` is
defined and where the threshold lives:

1. **Latent-state definition, `K = 1`** (Galton–Watson-aligned).
   `A := {max_t I(t) ≥ 1}`. Under the standard count-data observation model
   `p(y_t | X_t = 0) = 𝟙{y_t = 0}` (Poisson(0) and NegBin(0, k) both put mass 1
   at 0), so observing any `y_t ≥ 1` certifies `I(t) ≥ 1`. Identity is exact.
   This is the branching-process-extinction case the textbook formula
   `1 − q^{n_seed}` is built for.
2. **Observation-side definition, any `K`**. `A := {sup_t y_t ≥ K}` (or any
   other functional of `y` the surveillance system uses to trigger inclusion).
   `A` is a function of `y`, so `{Y = y} ⊆ A` trivially when `y` itself
   satisfies the condition. Identity is exact. The MC estimator (§3.1) must in
   this case run the observation model and threshold simulated _observations_,
   not the latent state.

The mathematically **broken** option — which earlier drafts of this proposal
fell into — is to define `A` on the latent state with `K > 1` (e.g.,
`A := {max_t I(t) ≥ 50}`) and try to apply the simple subtraction
`log L_PF − log P̂`. Then there exist trajectories that produced `y` despite the
latent `I` never reaching `K` (reporting flukes), so `{Y = y} ⊄ A`,
`p(y, A | θ) < p(y | θ)`, and the "correction" introduces its own bias. We
commit to options (1) or (2) explicitly in §3.

Under either, the corrected log-likelihood is

> `log L_cond(θ) = log L(θ) − log P(A | θ).  (★)`

This is the survival-conditioned log-likelihood. The MLE of `L_cond` removes the
selection bias that arises from analyzing only the trajectories that "happened."

### 1.3 Direction and size of the bias

The omitted term `log P(A | θ)` is more negative for `θ` values with _lower_
take-off probability — i.e., values closer to the epidemic threshold (`R₀ ↘ 1`,
small seed, high recovery rate). When this term is dropped, those values are
_under-weighted_ in the likelihood ratio: every `θ` is judged purely on its fit
to the surviving data, without paying for the probability that survival was
possible. Equivalently, high-take-off `θ` values get a free ride. The MLE drifts
toward those values. For epidemic models this typically means **`R₀` is biased
upward**, with the bias most severe for the early outbreak regime — exactly the
regime where parameter estimates feed into the most consequential public-health
decisions.

Numerical scale (Markovian SIR with Geometric offspring; see §2.2): extinction
probability `q = 1/R₀`, so the omitted term per seed is
`−log(1 − R₀^{−n_seed})`. For `R₀ = 2`, `n_seed = 5`: ≈ 0.03 nats — swamped by
data signal in any non-trivial dataset. For `R₀ = 1.2`, `n_seed = 1`: ≈ 1.79
nats — large enough to shift point estimates and tighten or invert credible
intervals.

### 1.4 Does the PF already handle this?

No. The PF returns a consistent estimator of `L(θ)`, the unconditional
likelihood, factorised as `∏ₜ p(y_t | y₁:ₜ₋₁, θ)`. The per-step conditional
log-likelihood that pomp exposes via `cond_logLik` (King, Nguyen & Ionides 2016,
JSS 69(12); pomp man page) is conditional on _past observations_, not on the
survival event `A`. The PF resampling step kills extinction-bound particles
because they're incompatible with the next data point, but the estimator still
targets the integral over unconditional trajectories — particles that "would
have gone extinct but were resampled away" still contribute to the variance, not
the bias, of the estimator. The conditioning in (★) is a different mathematical
operation, not implemented by any current camdl path.

---

## 2. The classical correction (analytic Galton–Watson)

This section establishes the theoretical baseline that the MC estimator in §3
approximates. The reader who wants only the camdl design can skim to §3.

### 2.1 Branching-process approximation

Britton & Pardoux (2019), ch. 1, develops the relevant theory. Their Theorem
1.2.5 establishes that the stochastic SIR/SEIR epidemic process and the
corresponding _branching process_ are equal in distribution up to the first
re-contact event ("ghost"), and that this re-contact time `T^N → ∞` in
probability as `N → ∞`. Their Corollaries 1.2.6–1.2.7 then transfer the
branching-process extinction theorem to the epidemic: for `R₀ ≤ 1` the epidemic
almost surely fades out; for `R₀ > 1` the final size `Z^N` is bimodal with one
mode at `o(N)` (_minor outbreak_) and one at `O(N)` (_major outbreak_), with the
minor-outbreak probability tending to the branching-process extinction
probability `q`:

> `q = smallest fixed point in [0,1] of  z = g(z),`

where `g` is the probability generating function (PGF) of the offspring
distribution. For independent initial infectives, `n_seed` of them gives overall
extinction probability `q^{n_seed}` and take-off probability

> `P(A | θ) = 1 − q(θ)^{n_seed(θ)}.  (♦)`

This is the formula the literature uses by hand when correcting likelihoods (cf.
Lloyd-Smith et al. 2005 supplement; Trapman 2007 Theor Pop Biol 71:160–173; the
texts of Andersson & Britton 2000 and Diekmann–Heesterbeek–Britton 2013).

### 2.2 Per-family `q(θ)`

The PGF `g` — and hence `q` — depends on the model family.

**Markovian SIR / SEIR.** Each infective has Geometric offspring with parameter
`p = γ/(λ + γ) = 1/(R₀+1)` (cf. Britton & Pardoux 2019, Example 1.3.3, where the
per-individual offspring is `Geom(p)` and the total tree size follows a Negative
Binomial). The PGF is `g(s) =
p / (1 − (1−p)s)`, and `g(q) = q` solves to

> `q = 1 / R₀  (R₀ > 1, Markovian SIR/SEIR).`

The latent period of SEIR does not change `q`; it changes only the _timing_ of
offspring, not the offspring distribution. The fixed-point equation is identical
to the Markovian SIR result.

**Reed–Frost / chain-binomial with Poisson offspring.** When offspring per
generation is `Poisson(R₀)` (the standard continuous-time Reed–Frost
approximation, also used in pomp's `dmeasure` examples), the PGF is
`g(s) = exp(R₀(s−1))`, and `q` solves

> `q = exp(R₀ (q − 1)),`

with closed-form `q = −W(−R₀ e^{−R₀}) / R₀` (Lambert-W), but in practice
resolved by Newton iteration. Borel distribution for the total tree size; see
Britton & Pardoux 2019, Example 1.3.2.

**Negative-binomial offspring (superspreading).** Lloyd-Smith et al. 2005
(Nature 438:355–359, supplement S1) introduces the offspring distribution
`NegBin(mean = R₀, dispersion = k)` to capture individual-level heterogeneity in
secondary cases. PGF `g(s) = (1 + (R₀/k)(1−s))^{−k}`, with `q` solving

> `q = (1 + (R₀/k)(1 − q))^{−k}.`

The Poisson result is recovered as `k → ∞`. Lower `k` (heavier-tailed
superspreading) _increases_ `q` — i.e., make extinction _more_ likely even at
the same mean `R₀` — because most chains die out and most transmission
concentrates in a few. For SARS-style `k ≈ 0.16`, `R₀ ≈ 3`: `q ≈ 0.76`, versus
`q ≈ 0.06` for the Poisson approximation at the same mean. This is a regime
where ignoring the correction is particularly costly.

**Multi-type / spatial.** Multi-type branching processes (Mode 1971) give a
vector fixed-point equation `q_i = g_i(q)` where `g_i` is the PGF of offspring
type-vector seeded by a type-`i` individual. Tractable but per-model.

### 2.3 What this asks of camdl

Implementing (♦) analytically inside camdl requires a per-model-family mapping
`(θ, model structure) ↦ q(θ)`. SIR/SEIR is one line. NegBin-offspring is closed
form. Spatial coupling is a tractable but nontrivial numerical fixed-point
solve. Stratified models with heterogeneous mixing matrices require care about
whether the offspring distribution is the leading eigenpair of the
next-generation matrix or something more refined. Each new model class needs a
derivation, documentation, tests, and a maintenance commitment.

For a framework like pomp where the user owns the `dmeasure` snippet, the
correction is folded in by hand at the modeler's discretion. For a DSL framework
like camdl, where we'd want to apply the correction generically, this becomes
uncomfortable.

---

## 3. Camdl's approach: Monte-Carlo survival probability

We propose estimating `P(A | θ)` directly by Monte Carlo from the existing
simulator.

### 3.1 The estimator

For each `θ` proposal during PMMH (or each PGAS sweep — see §4 for why IF2 is
_not_ on this list in v1), run `M` independent forward simulations of the model
from `t = 0` (or `t = t_seed`) through an early-phase horizon `t★`, _without
observation conditioning_ (no PF, no resampling, no weights). The functional
thresholded depends on which definition of `A` we committed to in §1.2:

> **(1) Latent-state `A = {max_t I(t) ≥ 1}` (default).**
> `P̂_M(A | θ) = (1/M) ∑_{m=1}^M 𝟙{max_{t ∈ [0, t★]} I_m(t) ≥ 1}` — i.e., the
> trajectory produced at least one secondary case. `K = 1` is fixed by the
> identity in §1.2 and is _not_ a free parameter. Recovers
> `1 − q(θ)^{n_seed(θ)}` in the large-`N` limit for whatever offspring
> distribution the simulator implements.

> **(2) Observation-side `A = {sup_t y_t ≥ K_obs}` (user-supplied operational
> threshold).** `P̂_M(A | θ) = (1/M) ∑_{m=1}^M 𝟙{sup_t y_t^{(m)} ≥ K_obs}`, where
> `y_t^{(m)}` is obtained by **running the observation model** on the `m`-th
> simulated trajectory. The MC routine must therefore evaluate `dmeasure` (or
> sample from it), not just inspect latent compartment counts. This is what an
> operational user wants when "this dataset exists because surveillance picked
> it up at ≥ `K_obs` cases."

`P̂_M` is unbiased for `P(A | θ)` with variance `P(A | θ)(1 − P(A | θ)) / M`. The
corrected log-likelihood is

> `log L̂_cond(θ) = log L̂_PF(θ) − log P̂_M(A | θ).`

The earlier draft's "threshold latent `I` at user-chosen `K`" option is dropped
— it does not correspond to any consistent definition of `A` and silently
introduces bias (§1.2).

### 3.2 Variance and cost

The delta-method approximation for the log-scale standard deviation is

> `SD(log P̂_M) ≈ √( (1 − P) / (M · P) ),  where  P = P(A | θ).`

This is **largest exactly where the correction matters**. The honest table:

| `P(A | θ)` | regime | `SD(log P̂)` at `M=400` | correction `|log P|` | |
---------- | ------------------- | ----------------------- |
-------------------- | | `0.99` | `R₀ ≫ 1`, big seed | 0.005 nats | 0.01 nats |
| `0.50` | `R₀ ≈ 2`, `n_seed=1`| 0.05 nats | 0.69 nats | | `0.17` | `R₀ = 1.2`,
`n_seed=1` | 0.11 nats | 1.79 nats | | `0.05` | `R₀ = 1.05`, `n_seed=1`| 0.22
nats | 3.00 nats | | `0.017` | `R₀ = 1.017`, `n_seed=1`| 0.38 nats | 4.07 nats |

So in the regime the correction is `O(0.01)` nats and irrelevant, the MC noise
is also `O(0.005)` and harmless. In the regime the correction is `O(1)` nats and
dominant, the MC noise is `O(0.1)` nats — non-negligible but still much smaller
than the bias it corrects. The worry is the third column down the page: when `P`
falls below `~0.05`, the MC estimator's log-scale noise approaches the same
order as the correction itself.

Two failure modes hide under the variance:

- **Zero-success degeneracy.** With `M` independent Bernoulli(`P`) trials, the
  probability of seeing zero take-offs is `(1−P)^M`. To keep this below 1%, we
  need `M ≳ log(0.01) / log(1−P) ≈ 4.6/P` for small `P`. At `P = 0.05`,
  `M ≳ 92`; at `P = 0.01`, `M ≳ 460`. The Anscombe-smoothed estimator (§4.2)
  prevents divide-by-zero blow-up but produces a wildly biased value at exactly
  the parameter vectors that matter.
- **PMMH proposal cliff.** A `θ'` proposal that lands at `P̂ = 0` has
  `log L̂_cond(θ') → +∞` under the raw estimator, which is rejected by the chain
  only because of `L̂_PF`. With a heavy-tailed proposal, rare zero-success events
  can spike the chain dynamics; with Anscombe smoothing the spike is bounded but
  biased.

**Adaptive-`M` rule.** Rather than a global `M`, the estimator should run a
cheap pilot batch of `M_pilot ≈ 50` sims, compute `P̂_pilot`, and then top up to

> `M_full = max(M_floor, ⌈5/max(P̂_pilot, 1/M_pilot)⌉),`

where the floor (we suggest `M_floor = 400`) covers the well-mixing regime and
the `5/P̂` term covers the `P → 0` regime. This makes the zero-success
probability never exceed `~e^{-5} ≈ 0.7%` per call. The cost is data-driven and
concentrates compute where it's needed.

**This is the strongest argument for an analytic-`q` fast path where one is
available.** For Markov SIR / SEIR with the `q = 1/R₀` formula, the correction
is exact and noise-free at zero incremental cost beyond the fixed-point solve.
The "MC for everything" position taken in the first draft is wrong; MC is the
right _generic fallback_, not the right default for model families where
closed-form `q` exists. The user-explicit `survival_method` switch in §6.1
implements this.

Per-`θ` MC cost: `M` forward sims through `[0, t★]`. Each sim is orders of
magnitude cheaper than a single PF likelihood evaluation (which is
`M_particles × T_obs` of the same per-step work, plus resampling).
Embarrassingly parallel via the existing rayon scaffolding.

### 3.3 The take-off threshold (which definition of `A`)

Per §1.2 there are two consistent definitions, and the user picks one
explicitly. Conflating them is the (avoidable) way to introduce
bias-on-top-of-bias-correction.

1. **`A = {max_t I(t) ≥ 1}` — Galton–Watson default.** The trajectory produced
   at least one secondary infection. `K` is fixed at 1; not a tunable. Recovers
   `1 − q^{n_seed}` in the large-`N` limit. Cleanest mathematical story; also
   the right default for synthetic-recovery validation where the data generator
   simply discards "no infections ever happened" trajectories.
2. **`A = {sup_t y_t ≥ K_obs}` (or cumulative-`y ≥ K_obs`) — operational
   definition.** User supplies `K_obs`. Threshold is on simulated
   _observations_, not latent state, so the MC routine evaluates the observation
   model. Right when the data exist because surveillance triggered at a known
   case-count threshold (e.g., AFP investigation thresholds for polio;
   case-cluster thresholds for Ebola).

We **do not** offer "threshold the latent `I` at a user-chosen `K > 1`" as an
option. It looks like a sensible middle-ground but violates the identity in §1.2
and silently biases the correction.

`K_obs` for option (2) must be supplied; we do not auto-default it from the data
because doing so would couple the conditioning to the sample, breaking the
conditional-likelihood interpretation.

### 3.4 The horizon `t★`

The branching-process approximation is valid while `S/N ≈ 1`, which in practice
means before the susceptible pool is meaningfully depleted. For `N = 10⁶` and
`K ≤ 50`, this is essentially the whole pre-takeoff period; `t★` = "time at
which `S/N` drops to 0.99" is a defensible default. For small `N` the branching
approximation degrades and the MC estimator becomes the more correct quantity
anyway — MC is robust to this; analytic Galton–Watson is not.

A simpler operational `t★`: end-of-data (`t_T`). Slightly more expensive but
always defensible — "took off" means "produced ≥ K infectives at some point
during the observation window." This is what we'd default to in the absence of
user input.

### 3.5 What `P̂` is conditioning on, precisely

`P̂_M(A | θ)` estimates the _unconditional_ probability of crossing the
threshold, computed from forward sims that ignore the data. This is correct: the
conditioning factor `P(A | θ)` in (★) is by definition an unconditional property
of the model under `θ`. We do _not_ want to condition `P̂` on `y` — that would
estimate a different and irrelevant quantity. The PF and the MC survival
estimator are computed from independent random number streams in v1; paired CRN
for variance reduction is a §8.5 follow-up to be measured, not committed to.

---

## 4. Composition with the PF estimator

### 4.1 Aggregation point — PMMH and PGAS only in v1

The PF returns `log L̂_PF(θ)` per PMMH proposal / per PGAS sweep. The survival
estimator returns `log P̂(A | θ)`. They subtract:

> `log L̂_cond(θ) = log L̂_PF(θ) − log P̂(A | θ).`

Concretely, this is one line at the loglik aggregation point in
`inference/pmmh.rs` and `inference/pgas.rs`. The estimator lives in a new
`inference/survival.rs`.

**IF2 is scoped out of v1.** This is not a code-organization choice; it's a
correctness constraint. IF2's parameter swarm is weighted by the per-particle
filter likelihood throughout the iteration. Subtracting `log P̂(A | θ̂)` at the
_reported_ loglik step only changes what we print, not what the swarm climbs —
the swarm still concentrates on `argmax L_PF`, producing a "corrected likelihood
reported at an uncorrected MLE." To shift the IF2 estimate, the `−log P(A | θ)`
term has to enter the parameter-particle re-weighting at every iteration, which
is materially more invasive than the PMMH/PGAS hook and needs its own design
pass (do we re-estimate `P̂` per particle? per iteration with shared seed? per
cooling step?). v1 ships PMMH/PGAS support and an error message on IF2 +
`survival_conditioning.enabled`. v2 deals with IF2.

### 4.2 Estimator menu and PMMH unbiasedness

PMMH (Andrieu, Doucet & Holenstein 2010, JRSSB 72:269–342) requires the
likelihood estimator to be non-negative and unbiased for the chain to target the
correct posterior. `L̂_PF` is unbiased; the challenge is producing an unbiased
estimator of `L_PF / P(A | θ)` — i.e., an unbiased estimator of `1/P(A | θ)`.
Three options, tradeoffs explicit:

**(a) Anscombe-smoothed plug-in (default).**
`P̂_smooth = (N_takeoff + 0.5) / (M + 1)`;
`log L̂_cond = log L̂_PF −
log P̂_smooth`. Bounded compute (fixed `M` per call,
adaptive per §3.2). Bias `O(1/M)` from Jensen on `1/P̂` — the chain targets a
shifted posterior, but the shift is the same order as the noise floor of the PF
estimator itself and is invisible in practice for `M ≳ 200`. **Practical
default.** No exactness guarantee.

**(b) Geometric / inverse-Bernoulli (exact).** Run independent forward sims
until the first take-off; let `G` be the trial index (so `G ~ Geom(P(A | θ))`
and `E[G] = 1/P(A | θ)` _exactly_). Use `L̂_cond = L̂_PF · G`. Because `L̂_PF ⫫ G`
under independent RNG streams, the product is exactly unbiased and
PMMH-exactness is preserved. Cost: expected `1/P` sims, unbounded as `P → 0`;
relative SD `√(1 − P) ≈ 1` regardless of `P`. This is a Russian-roulette / Lyne
et al. (2015, Stat. Sci. 30:443–467) style estimator. Right choice when (i) the
user wants formal PMMH guarantees and (ii) `P` is bounded away from 0. Wrong
choice when `P` can be tiny — the unbounded tail of `G` is a real liability.

**(c) Analytic `q(θ)` (exact, free).** For model families with closed-form
extinction probability — Markov SIR/SEIR, NegBin offspring, Poisson offspring
with Newton solve — `P(A | θ) = 1 − q(θ)^{n_seed(θ)}` is _exact_.
`L̂_cond =
L̂_PF / (1 − q(θ)^{n_seed(θ)})` is exactly unbiased for `L_cond`, zero
MC noise, zero extra simulation cost. **This is the right choice in the low-`P`
danger zone whenever applicable.** §6.1's `survival_method = "analytic_sir"`
etc. exposes this; the user is responsible for asserting that the model fits the
assumed offspring family.

Recommendation: default (a) for genericness; offer (b) as a flag for formal
guarantees; _prefer_ (c) when the user can assert the offspring family.
Synthetic-recovery cells should run all three and report agreement (§7).

### 4.3 Reproducibility

The MC sims must consume a deterministic RNG stream so seed-paired runs (the
existing scenario `enable`/`disable` mechanism) produce identical corrections.
Add a `survival_seed_offset` derived from the top-level run seed; document it in
`RNG_DETAILS.md`. For the geometric estimator (b), seed-pairing also pins the
trial count `G` per call.

---

## 5. Relation to prior work

This section is included so a reader from the Britton / King / Ionides
communities knows where we sit and what we do and don't claim.

### 5.1 Stochastic-epidemic theory

The branching-process / extinction-probability machinery is textbook (Andersson
& Britton 2000; Diekmann, Heesterbeek & Britton 2013; the self-contained chapter
in Britton & Pardoux 2019, arXiv:1808.05350, which we use as the citable
reference). The epidemic-to-branching- process coupling underpinning Theorem
1.2.5 there is due originally to Ball & Donnelly (1995, _Stoch. Proc. Appl._
55:1–21); we cite both. The take-off-vs-fade-out dichotomy and the
`1 − q^{n_seed}` correction are not original to us and are explicitly framed in
those sources as the right inference target for emerging-outbreak data.
Lloyd-Smith et al. 2005 contributed the NegBin offspring extension. Trapman
(2007), Trapman & Reluga (2014), and the network-epidemic literature extend `q`
to network structure.

### 5.1a Prior art in the PF/pMCMC tradition

The PF-inference community has explicitly acknowledged this conditioning and
consciously omitted it. The clearest statement is in Gill, Koskela, Didelot &
Everitt (2023, arXiv:2311.09838, revised 2025), §2.1, p. 4 (we read this passage
directly to verify the quote):

> _"This likelihood should ideally be conditioned to assign probability 1 to the
> event that the epidemic does not die out or become negative... However, this
> conditioning is computationally expensive and the probability of sampling
> below `−x_{n−1}` is small, especially as the epidemic grows. As such, we omit
> the condition in practice."_

Their per-day Skellam-conditioning is mathematically distinct from our global
take-off conditioning, but the accumulated daily-survival condition over the
full series is the same event, and their stated omission rationale ("small when
the epidemic grows") is precisely the high-`R₀` regime where our correction is
also negligible. The _low-_ `R₀` / small-outbreak regime — where they tacitly
concede the omission isn't negligible — is what this proposal targets. This
paper is a better anchor for the §5.3 "currently unfilled in this tradition"
framing than the Britton textbooks, because it's a working PF-inference paper
making the omission consciously rather than a theoretical text deriving the
underlying formula.

### 5.2 What pomp does (and doesn't)

pomp (King, Nguyen & Ionides 2016, JSS 69(12)) is the most influential PF-based
inference framework in epidemiology, and the most direct methodological
neighbour to camdl. pomp's `pfilter` returns the unconditional log-likelihood
`log p(y₁:T | θ)`, with per-time-step factors accessible via `cond_logLik` — the
latter is the conditional log-likelihood "Pr[y_t | y_{1:t-1}, θ]" (pomp
`pfilter` manual page), i.e. conditioning on past observations within the
sequential factorisation of `L(θ)`, _not_ on the survival event `A`. The pomp
package does not implement the (★) correction.

The published pomp Ebola analysis (King, Domenech de Cellès, Magpantay & Rohani
2015, Proc R Soc B 282:20150347, which is about deterministic-vs-stochastic
fitting on cumulative-vs-raw incidence — _not_ about extinction conditioning)
likewise does not apply the correction. We've checked the pomp Ebola model page
(`kingaa.github.io/manuals/pomp/html/ebola.html`); the `dmeasure` combines case-
and death-reporting likelihoods with no extinction term. (Verified by reading
the page on 2026-05-23. If a more recent King-group analysis adds the
correction, we'd want to know.)

The pomp idiom, where the correction _is_ applied, is to fold it into the
user-defined `dmeasure` snippet — typically as a constant additive term (in
`θ`-independent regimes) or as an inline analytical `q(θ)` evaluation. This
works because pomp users write per-model C snippets; each modeler decides if and
how to condition. The frameworks itself stays opinion-free.

### 5.3 Our approach

A careful pomp user can absolutely apply (★) by hand inside `dmeasure`. We are
not claiming pomp can't do this. What camdl is doing differently:

1. **Making the correction available as a framework primitive** with two backing
   routes (MC generic; analytic `q(θ)` for recognized model families) and a
   user-explicit `survival_method` switch. The user picks; camdl doesn't
   silently auto-detect (v1) and doesn't force a `dmeasure`-snippet rewrite
   (which our DSL-first architecture doesn't support anyway).
2. **Making it ergonomic to opt in or out at the inference-config level** rather
   than at the model-spec level. The same model file can be fit with or without
   the correction, which matters for sensitivity analysis and for
   synthetic-recovery validation where the default may legitimately need to
   flip.
3. **Documenting the trap** (§8.1) so users don't accidentally double-count in
   synthetic-recovery pipelines.
4. **Making the choice of conditioning event explicit and typed**
   (`event = "latent_k1" | "observed_k"`, §3.3) rather than leaving it as an
   implicit modeling assumption buried in `dmeasure`. This is the §1.2 lesson we
   learned the hard way in drafting: the identity that makes (★) clean is
   fragile under sloppy `K`-definitions, and surfacing the choice is the
   cheapest way to prevent that class of misuse.

---

## 6. API and semantics

### 6.1 Configuration

Add to the inference configuration (TOML, applies to PMMH and PGAS only in v1;
see §4.1 on IF2):

```toml
[survival_conditioning]
enabled = false # default OFF; user must turn on
algorithm = "pmmh" # "pmmh" | "pgas"; if2 rejected in v1
event = "latent_k1" # "latent_k1" (Galton–Watson) | "observed_k"
k_obs = 0 # required when event = "observed_k"
survival_method = "mc" # "mc" | "analytic_sir" | "analytic_negbin"
# (analytic_poisson coming with the chain_binomial
# fast path; see §8.5)
estimator = "anscombe" # "anscombe" | "geometric"
# (anscombe = approx, bounded cost;
#  geometric = exact, unbounded tail)
n_sims_floor = 400 # MC budget floor; adaptive 5/P̂ on top (§3.2)
horizon = "data-end" # "data-end" | "s99" | <real>
seed_offset = 0 # RNG offset (default fine)
```

`enabled = false` is the load-bearing default: existing fits continue to produce
unconditional likelihoods unchanged. No silent semantic change to any current
pipeline. When `survival_method = "analytic_*"`, `n_sims_floor` and `estimator`
are ignored — `q(θ)` is computed directly from the formula, the correction is
exact and noise-free, and PMMH retains Andrieu–Doucet–Holenstein exactness.

(Caveat: the actual CLI/flag surface should follow the `camdl-book`-consultation
discipline our CLAUDE.md memory specifies before we land flags. The TOML above
is a strawman for review, not a commitment. See follow-up note at the end of
§8.)

### 6.2 Where it hooks in

```
inference/
  survival.rs     <- new: P̂(A | θ) estimator (MC + analytic dispatch)
  pmmh.rs         <- one-line addition at loglik aggregation
  pgas.rs         <- one-line addition at loglik aggregation
  if2.rs          <- error if survival_conditioning.enabled (see §4.1)
```

The survival estimator depends only on the existing simulator (`Simulate`
trait + `Capabilities`) and, for `analytic_*` paths, small per-family `q(θ)`
routines living next to `survival.rs`; no new IR fields, no new expressions, no
schema bump.

### 6.3 Reporting

When the correction is on, the per-iteration diagnostics should report both
`log L̂_PF` and `log P̂_M` separately, not just the combined `log L̂_cond`. This
lets the user see when the correction is load-bearing and when it's noise. Add a
column to the existing fit reports (`fit_summary.json`, the markdown / TeX
renderings).

---

## 7. Validation plan

### 7.1 Bias reduction at low R₀ / small seed

Synthetic recovery sweep:

- True model: Markovian SIR with `R₀ ∈ {1.2, 1.5, 2.0}`,
  `n_seed ∈
  {1, 5, 25}`, `N = 10⁶`.
- Generate `~200` synthetic datasets per cell, **discarding extinct trajectories
  using the same definition of `A` as the correction**. For the default
  `event = "latent_k1"` cell, "extinct" = `max_t I(t)
  = 0`. For an
  `event = "observed_k"` cell with `k_obs = 50`, "extinct" = `sup_t y_t < 50`.
  Mismatch here is the single most common way a recovery sweep silently
  passes/fails for the wrong reason; the test harness should assert the
  consistency before running.
- Fit each cell with and without the correction.
- Expected outcome: corrected MLE recovers `R₀_true` within MC noise;
  uncorrected MLE biased high, with the bias magnitude tracking
  `−log(1 − R₀^{−n_seed})` per dataset.

This is the load-bearing experiment. If the bias reduction doesn't match theory,
the proposal is wrong somewhere.

### 7.2 Agreement with analytic Galton–Watson

For Markovian SIR cells where `q = 1/R₀` is exact, the MC estimator should agree
with `1 − R₀^{−n_seed}` to within `O(1/√M)`. A small unit test against this
closed-form is cheap and pins the implementation.

Likewise for the NegBin-offspring case (Lloyd-Smith): MC vs the fixed-point
solve. This guards against the kind of "the formula in the paper assumes a
different convention" bug that bites whenever NegBin parameterisations are
involved.

### 7.3 Synthetic-recovery no-op when generator doesn't pre-filter

Generate synthetic data _without_ discarding extinct trajectories (simulate once
per seed, accept whatever). Fit with the correction on. Expected outcome:
corrected MLE is biased _low_ by the same `O(1)` amount the uncorrected MLE is
biased high under §7.1. This validates the documented trap and gives us a
regression test for the "corrected fit on uncorrected synth" misuse.

### 7.4 Real-data sanity on a familiar problem

Re-fit the He, Ionides & King 2010 measles dataset (or a subset) with and
without the correction. Expectation: at the He et al. MLE, `P̂(A | θ) ≈ 1` and
the correction is negligible. This is a sanity check that we don't break
high-`R₀` regimes; not a discovery experiment.

### 7.5 cVDPV2 / emerging-outbreak case study

The motivating application. Re-run the cVDPV2 early-emergence fits from the
Nigeria work; report posteriors with and without the correction. Two reports,
not one:

- **`R₀` posterior**: expected to move down, with magnitude consistent with the
  `1 − q^{n_seed}` prediction at typical posterior `R₀`.
- **Seed posterior** (`n_seed` / `t_seed`): the seed magnitude is itself a
  parameter on the cVDPV2 ridge (see the seed-timing proposal,
  `2026-05-20-seed-timing-inference.md`), and the correction does meaningful
  inferential work on a weakly-identified quantity. Report the seed-magnitude
  posterior shift, and run a `K_obs` sensitivity sweep (for the
  `event = "observed_k"` cell) at `K_obs ∈ {1, 5, 25, 100}` to characterise how
  much the seed posterior depends on the conditioning event. This is where the
  threshold sensitivity flagged in §8.2 will actually bite for applications.

### 7.6 Open empirical questions and what tests would settle them

A few choices in this proposal are defensible but not certain; we'd rather treat
them as testable hypotheses than as foregone conclusions. Each becomes a
follow-up experiment to schedule alongside v1:

1. **Anscombe-vs-geometric in PMMH.** The §4.2 claim that the Anscombe `O(1/M)`
   bias is "invisible in practice" at `M ≳ 200` is plausible but not measured.
   Test: same low-`R₀` synth cell as §7.1, run PMMH with (a) Anscombe, (b)
   geometric, (c) analytic `q`, and compare posterior means and 95% CIs across
   the three. If Anscombe and analytic agree within MCMC noise, the bias is
   indeed invisible and we're fine to default to Anscombe; if not, geometric
   becomes the default.
2. **Adaptive-`M` floor.** The `M_floor = 400` and the `5/P̂_pilot` rule are
   educated guesses. Test: profile PMMH wall-time and acceptance rate across
   `M_floor ∈ {100, 200, 400, 800}` and `M_pilot ∈ {25, 50, 100}` on a low-`P`
   cell; pick the floor that gets to `~25%` acceptance at minimum cost.
3. **Operational `K_obs` vs latent `K=1`.** For real outbreak data the
   operational definition is more honest, but the Galton–Watson default is
   mathematically clean. Test: cVDPV2 case study with both, report posterior
   agreement (or disagreement, and which way).
4. **Branching approximation degradation at small `N`.** For `N ≪ 10⁶`, analytic
   `q` diverges from "actual" survival probability. Test: synthetic recovery at
   `N ∈ {500, 5000, 50000,
   10⁶}` with both `analytic_sir` and `mc` paths;
   identify the `N` at which the two diverge by more than 5% on `P̂`. This tells
   us when to warn users against `analytic_*`.
5. **Multi-seed independence assumption.** `P(A) = 1 − q^{n_seed}` assumes the
   `n_seed` introductions are independent Galton–Watson trees. If introductions
   are tightly coupled in time (e.g., a single super-spreader event seeds
   several initial infectives in the same household), independence fails. Test:
   synthetic recovery with correlated-vs-independent seeds; quantify the bias of
   `analytic_sir` under correlation.

These belong in `docs/dev/notes/` as separate investigations rather than
blockers on the v1 implementation, but they should be _scheduled_, not deferred
indefinitely.

---

## 8. Caveats and non-goals

### 8.1 The double-counting trap

Stated again because it's the most common way this gets misused.
Survival-conditioned likelihoods are correct when the data are drawn from
`p(y | θ, A)` — i.e. real outbreak data, or synthetic data generated by a
pipeline that discards extinction. They are incorrect when the data are drawn
from the full `p(y | θ)` — i.e. synthetic data generated by a "simulate once,
take what you get" pipeline. The flag must be off by default, and the reporting
needs to make the conditioning visible in every emitted fit summary.

### 8.2 Threshold sensitivity

Different `K` give different `P(A | θ)`. We document this and require the user
to pick a `K` explicitly (with hinted defaults). We do _not_ attempt to
"auto-tune" `K` based on the data — that would couple the correction to the
dataset in a way that breaks the conditioning interpretation.

### 8.3 Where this doesn't help

- Datasets with strong post-takeoff signal (long, dense time series at
  moderate-to-high `R₀`): the correction is `O(0.01)` nats; not worth the cost.
- Datasets where the conditioning isn't `{epidemic took off}` but something more
  refined (e.g. "epidemic reached city X by time Y"). This proposal handles the
  standard take-off conditioning. A generalisation to user-defined conditioning
  events is a future extension, not in scope here.
- Models where the unconditional likelihood is what the user actually wants
  (e.g. _forecasting_ future outbreaks given current `θ`, where we explicitly
  want to integrate over extinction).

### 8.4 What we are _not_ claiming

- Not claiming the correction is novel. It is the standard Galton–Watson
  conditioning, due (in the epidemic context) to a chain of contributors from
  Bartlett through Britton.
- Not claiming pomp can't do this. A careful pomp user can apply (★) inside
  `dmeasure`. We're observing that the camdl architecture makes the MC variant
  available cheaply and generically.
- Not claiming this fixes general model misspecification. It addresses one
  specific selection bias.

### 8.5 Follow-ups

- **CLI/flag surface design**: per our `camdl-book` consultation memory, the
  actual CLI flag names and command surface should be designed against the first
  two book chapters (`getting-started.qmd`, `experiments.qmd`) before
  implementation. The TOML stub in §6.1 is a placeholder.
- **IF2 support** (§4.1). The per-particle-weighting integration is its own
  design problem; defer to v2.
- **Auto-detect analytic family from model topology.** v1 makes
  `survival_method` user-explicit; the right destination is for camdl to inspect
  the IR (compartment structure, transition rate expressions) and pick
  `analytic_sir` automatically when the model is recognized SIR/SEIR. Risky to
  ship in v1 — the detection logic needs careful negative testing
  (false-positives on lookalike models would be silently wrong, which is worse
  than the present "user knows what their model is" requirement).
- **User-defined conditioning events** (§8.3): per-model `A` specified in the
  DSL (e.g., "epidemic reached `city = X` by `t = Y`") as a v2 extension.
- **`chain_binomial` analytic Poisson-offspring fast path.** The
  `q = exp(R₀(q−1))` Newton solve is six lines; add
  `survival_method
  = "analytic_poisson"` once we have a clear use case.
- **Variance-reduction via CRN.** Currently the PF and the survival MC use
  independent RNG streams. With paired CRN (the same noise vector applied to
  both), the variance of `log L̂_PF − log P̂` is reduced; this is worth measuring
  once the v1 estimator is in place.

---

## References

The following were checked against primary sources for this proposal (not relied
on from memory).

- **Andersson, H. & Britton, T. (2000)**. _Stochastic Epidemic Models and Their
  Statistical Analysis_. Lecture Notes in Statistics 151, Springer. — General
  stochastic-epidemic theory background.
- **Andrieu, C., Doucet, A. & Holenstein, R. (2010)**. Particle Markov chain
  Monte Carlo methods. _J. R. Statist. Soc. B_ 72(3):269–342. — PMMH
  foundations; relevant for §4.2 on unbiasedness requirements.
- **Ball, F. & Donnelly, P. (1995)**. Strong approximations for epidemic models.
  _Stoch. Proc. Appl._ 55(1):1–21. — The primary source for the
  epidemic-to-branching-process coupling that Britton & Pardoux (2019) Theorem
  1.2.5 packages.
- **Britton, T. & Pardoux, E.** (eds., **2019**). _Stochastic Epidemic Models
  with Inference_. Lecture Notes in Mathematics 2255, Springer.
  arXiv:1808.05350. — Theorem 1.2.5 and Corollaries 1.2.6–1.2.7 are the
  theoretical basis for §2.1. Examples 1.3.2–1.3.3 give the Borel and
  Negative-Binomial total-progeny laws used in §2.2.
- **Gill, A., Koskela, J., Didelot, X. & Everitt, R. G. (2023, rev. 2025)**.
  Bayesian inference of reproduction number from epidemiological and genetic
  data using particle MCMC. arXiv:2311.09838 (JRSS-C). — §2.1, p. 4: explicit
  acknowledgment that "this likelihood should ideally be conditioned to assign
  probability 1 to the event that the epidemic does not die out or become
  negative... we omit the condition in practice." The cleanest prior-art anchor
  for the framing in §5.1a.
- **Diekmann, O., Heesterbeek, H. & Britton, T. (2013)**. _Mathematical Tools
  for Understanding Infectious Disease Dynamics_. Princeton University Press. —
  General reference; ch. 3 on branching approximations.
- **King, A.A., Domenech de Cellès, M., Magpantay, F.M.G. & Rohani, P. (2015)**.
  Avoidable errors in the modelling of outbreaks of emerging pathogens, with
  special reference to Ebola. _Proc. R. Soc. B_ 282:20150347,
  https://doi.org/10.1098/rspb.2015.0347. — Cited here only to _correct_ a
  common misattribution: this paper is about deterministic-vs-stochastic models
  on cumulative-vs-raw-incidence data, not about extinction conditioning. The
  widely-shared intuition "the King paper says you have to condition on
  survival" is, as best we can tell, a conflation with the broader
  Britton-tradition literature.
- **King, A.A., Nguyen, D. & Ionides, E.L. (2016)**. Statistical inference for
  partially observed Markov processes via the R package pomp. _J. Stat. Softw._
  69(12). — Pomp reference. Confirms `pfilter` returns the unconditional
  `log p(y|θ)` factorised as `∑ log p(yₜ|y₁:ₜ₋₁, θ)`.
- **Lloyd-Smith, J.O., Schreiber, S.J., Kopp, P.E. & Getz, W.M. (2005)**.
  Superspreading and the effect of individual variation on disease emergence.
  _Nature_ 438:355–359 (and supplementary information S1). — Negative-binomial
  offspring; the formula `q = (1 + (R₀/k)(1−q))^{−k}` is the application of
  standard branching-process theory to the NegBin offspring family they
  introduce.
- **Lyne, A.-M., Girolami, M., Atchadé, Y., Strathmann, H. & Simpson, D.
  (2015)**. On Russian roulette estimates for Bayesian inference with
  doubly-intractable likelihoods. _Stat. Sci._ 30(4):443–467. — Background for
  the geometric / inverse-Bernoulli estimator in §4.2(b); not relied on directly
  but the right reference for the unbounded-cost exactness tradeoff.
- **Trapman, P. (2007)**. On analytical approaches to epidemics on networks.
  _Theor. Pop. Biol._ 71(2):160–173. — Extensions of `q` to
  network/structured-population settings; not used directly here but the natural
  next reference once we go beyond homogeneous mixing.

Pomp behavior verified by reading the package documentation at
`https://kingaa.github.io/manuals/pomp/html/pfilter.html` and the Ebola model
documentation at `https://kingaa.github.io/manuals/pomp/html/ebola.html` on
2026-05-23. If either has been updated since then to include
survival-conditioning hooks, this proposal should be revised.
