# Proposal: Seed Timing Inference for Early-Outbreak Models

**Status:** draft for discussion — revised against the current codebase (post-PR-#65 lineage merge) and to compose with the lineage layer.
**Scope:** a mechanism for estimating the introduction/seed time of an epidemic, plus the inference machinery and honest-reporting outputs to support it.
**Primary application:** early cVDPV2 emergence dynamics (Nigeria), generalizing to any early-outbreak setting.

> **Provenance.** This revises a design draft circulated after colleague
> consultation. The substantive changes here are (1) two corrections forced
> by reading the current code — the event-time claim in §4 and the lineage
> contract in §8 — and (2) a new §11 ("Verification against the current
> codebase") that turns the design into an implementation contract. The
> deconvolution baseline (§3.5, §6.2) is elaborated per request. Citations
> were verified (see References).

---

## Summary

Estimating *when* an outbreak was seeded is the temporal twin of the
seed-magnitude problem camdl already wrestles with. The two are confounded
through the same exponential-growth mechanism, so the load-bearing design fact
is an **identifiability ridge**: during exponential growth the data constrain a
*product* of seed time and seed size, not either alone. This drives four
decisions.

1. The honest deliverable is a **posterior over the seed time τ**, not an
   IF2 point estimate, because the latter wanders along the ridge.
2. Seeding should be implemented as a **smooth quantity** — a continuous
   importation pulse — *not* a discrete event at τ. A discrete event has zero
   gradient almost everywhere and confounds random-walk filters; every
   production tool surveyed avoids it. The smoothing is governed by a **width
   parameter `w`** (§4), which is not a nuisance constant but a modeling knob
   that sets the τ-posterior width.
3. We support **two methodologically independent estimators** so that
   disagreement is *diagnostic* rather than a redundant agreement that
   manufactures false confidence. The cheap always-available one is a
   non-mechanistic deconvolution baseline; the principled one is the
   genealogical likelihood from the lineage layer, which informs exactly the
   early/deep phase the incidence data cannot.
4. The seed feature must **compose with the lineage / joint-inference path**
   (§8). Three contracts: the seed lives entirely in `P(counts)`; it is
   implemented as a lineage-trackable importation inflow (so the genealogy can
   attribute it); and genomic information enters as a *likelihood* on the shared
   τ, not a baked-in prior.

**Recommended minimal build:** a continuous-importation mechanism (smooth,
reuses the existing parameter / transform / perturbation machinery, gradients
flow), expressed as a source-less importation inflow into a tracked
compartment, with the joint `(τ, N_seed)` ridge reported as a **first-class
output rather than a caveat**, plus a deconvolution baseline in `camdl
compare`. §11 shows this mechanism is expressible in the DSL *today* with no
new language primitive and no inference-stack change.

### Notation

| Symbol | Meaning | Units |
|---|---|---|
| τ (tau) | seed / importation time | time (model clock) |
| N_seed | seed size (individuals introduced) | count |
| r | exponential growth rate of the early epidemic | 1/time |
| λ (lambda) | importation strength (continuous mechanism) | individuals/time |
| w | importation-pulse width | time |
| t_start (t0) | process time origin: start of latent dynamics before first observation | time |
| t_seed | seeding-window length (EpiNow2 sense) — distinct from `w`, see §4 | time |
| ρ (rho) | case-detection / reporting probability | dimensionless ∈ (0,1] |
| FoI | force of infection (per-susceptible hazard of infection) | 1/time |
| TMRCA | time to most recent common ancestor (genomic/genealogical) | time |
| LTT | lineages-through-time: count of ancestral lineages vs time before present | count |
| FIM | Fisher information matrix | — |
| PIT / CRPS | probability integral transform / continuous ranked probability score | — |

(In SIR notation γ often denotes the *recovery* rate; here we write ρ for the
detection probability to avoid the collision, since the detection–seed-size
ridge is what matters below.)

---

## 1. Motivation and the candidate estimands

In early-outbreak modeling the most actionable quantity is often "when did this
start?" For cVDPV2 specifically, the seeding date relative to a vaccination
campaign or the Switch determines whether an emergence is attributable to
mOPV2 vs nOPV2, and feeds directly into emergence-risk monitoring. But "seed
date" is not one quantity; it is at least three, with different identifiability
stories:

- **(E1) Importation time τ** — a small, fixed seed (e.g. 1 infected) enters at
  unknown τ; estimate τ with seed size fixed or strongly prior'd. *Cleanest
  identifiability.*
- **(E2) Epidemic origin / time-shift** — estimate how much unobserved burn-in
  preceded the first observation; equivalently estimate t_start and slide the
  whole trajectory. *Well-defined, slightly different question.*
- **(E3) Joint (τ, N_seed)** — estimate timing and magnitude together, accepting
  and reporting the ridge. *Most honest, least identifiable.*

We treat **E1 as the default target**, **E3 as the honest-reporting mode**
layered on top (§7), and **E2 as a secondary mechanism** (§4).

---

## 2. The load-bearing constraint: the seeding ridge

During the exponential phase, observed prevalence behaves like

```
I(t) ≈ N_seed · exp(r · (t − τ))
```

so the data constrain the combination `N_seed · exp(−rτ)`, not `τ`, `N_seed`,
and `r` separately. τ is identifiable only if you (a) fix the seed size, (b) put
a strong prior on it, or (c) pin the growth rate. This is the magnitude ridge
rotated onto the time axis — a **sloppy direction** of the FIM: the likelihood
is nearly flat along the `(τ, N_seed)` combination and steep orthogonal to it.

The literature is uniform on this. Smirnova et al. (2025, *Infectious Disease
Modelling*) fit growth-phase case counts and found that estimating the
**detection rate jointly with the initial number of cases I₀** caused the MCMC
to fail to converge: an increase in the detection rate was exactly compensated
by a decrease in initial cases, producing near-identical observed trajectories.
Fixing the detection rate eliminated the non-identifiability. The same ridge
appears in network-model work (Tien et al., arXiv:2208.07543): distinct
(degree, transmissibility) pairs yield indistinguishable prevalence and
daily-case curves.

**Design implications.**
- Default to a Bayesian posterior over τ with honest width. An IF2/MLE point
  estimate is unstable along the ridge and reports spuriously precise τ
  depending on initialization.
- Make the resolving information *explicit*: whether τ's posterior width comes
  from data or from a prior on seed size should be visible to the user, not
  buried (§7).

---

## 3. State of the art: method families and tradeoffs

Six families appear in the literature, differing in *estimand*, the *mechanism
by which τ enters*, the *smoothness of the likelihood*, and the *data modality*
needed.

| Family | Estimand | Mechanism for τ | Likelihood in τ | Data needed | Backend fit |
|---|---|---|---|---|---|
| Mechanistic onset-as-parameter (PMCMC) | E1/E3 | onset time is a model parameter | depends on seeding mechanism (§4) | case time series | PMMH yes; gradient backends only if smooth |
| Continuous importation pulse (`pomp` `iota`) | E1 | low external influx + fixed t0 | smooth | case time series | all backends; gradients flow |
| Seeding window + estimated initial level (EpiNow2) | E2 (→E1) | estimate initial trajectory over fixed t_seed | smooth | case time series | gradient/HMC-friendly |
| Free t_start / time-origin shift | E2 | t0 itself is a parameter | smooth-ish; couples to obs alignment | case time series | partial; touches obs→time map |
| Back-calculation / deconvolution | latent infection curve | reconstruct infections via delay convolution | smooth (regularized) | counts + delay distribution | standalone; non-mechanistic |
| Phylodynamic / genealogical | introduction date | coalescent / birth-death (+ clock) | n/a (separate likelihood) | genealogy / sequences | external or joint (§8) |

### 3.1 Mechanistic onset-as-parameter (PMCMC)

A stochastic SEIR in a state-space model with particle MCMC estimating onset
time alongside transmission and recovery. Shi et al. (2024, *Frontiers in
Public Health*) did exactly this on COVID-19 in Wuhan, Shanghai and Nanjing,
recovering onset time, key parameters and R₀ from early-stage case reports. The
estimand we want; it inherits whatever smoothness the seeding mechanism (§4)
gives it, and (as that paper notes) degrades with very short, noisy early
windows — which is precisely the regime the ridge (§2) describes.

### 3.2 Continuous importation pulse — the clean one

`pomp` bakes seeding in as a continuous import-rate parameter `iota` plus a
fixed time origin `t0` before the first observation (King, Nguyen & Ionides
2016, *J. Stat. Softw.* 69(12)). τ enters as an ordinary smooth rate
expression; gradients flow and iterated filtering / particle MCMC / particle
Gibbs all work without special-casing. **This is mechanism (B) below, the
recommended primary.**

### 3.3 Seeding window with estimated initial level — the clever one

EpiNow2 estimates an initial infection trajectory over a *fixed* seeding window
t_seed preceding the first observation, assuming constant exponential growth in
line with the initial reproduction number — **converting the timing question
into a magnitude question over a fixed burn-in window**, sidestepping
discontinuity (Abbott et al., `EpiNow2`, `estimate_infections()` model
definition). (Too short a t_seed forces an artificial initial decline.) This is
the disciplined form of mechanism (C). *Do not conflate its t_seed with the
importation-pulse width `w` — see §4.1.*

### 3.4 Free t_start / time-origin shift

Make t0 a parameter and let the whole trajectory slide. A particle-filter
COVID study (Bicher et al. / the Brazil medRxiv 2020 analysis) recovered a
negative t0 (epidemic began ~6 days before the first reported case).
Smooth-ish, but couples to the observation→time alignment.

### 3.5 Back-calculation / deconvolution — the non-mechanistic complement

This family is elaborated in §6.2 (it is the recommended cheap baseline).
Briefly: observed counts are a convolution of the latent infection curve with a
known infection-to-observation delay distribution; deconvolving recovers the
infection curve, whose left tail *is* the seeding signal. It shares no
likelihood ridge with the mechanistic model, so agreement / disagreement with
mechanism B is informative rather than redundant.

### 3.6 Genealogical / phylodynamic — the deep-phase anchor

When a genealogy or sequences exist, coalescent / birth-death models (with a
molecular clock, e.g. BEAST) estimate the clade TMRCA and the
introduction-to-detection lag. The transferable point: TMRCA approximates the
first sampled transmission event, *associated with but not equal to* the true
introduction — analogous to the introduction-vs-first-detection gap in case
data. For cVDPV2, the field-standard estimator is the VP1 molecular clock (a
small number of fixed substitutions then ≈1% divergence/year). With the lineage
layer (§8), camdl can move this from an external prior toward an internal
likelihood — with the caveat in §8 that what exists today is transmission-tree
*topology*, not a clocked phylogeny.

### 3.7 Stochastic early-phase / first-detection theory

Surviving epidemics grow *faster* initially than the asymptotic rate; the
first-detection-time distribution depends on testing effort. Czuppon et al.
(2021, *J. R. Soc. Interface*) used this to date UK Alpha to ~4 August 2020
(mean infection-to-sampling lag ≈46 days, SD 19.5) despite September detection.
Désirée et al. (2024, *PLOS Comput. Biol.*, "Using early detection data to
estimate the date of emergence") formalizes emergence-date estimation from
first-detection data directly. The right theory for priors on the
introduction→detection lag and for sanity-checking recovered τ.

---

## 4. How τ enters the model, and the width parameter `w`

**Current state (verified — see §11).** Event-schedule times must be
compile-time constants: the expander's `resolve_float_expr` evaluates a schedule
time and, if it does not reduce to a constant, raises **E401 "expected a
constant expression"** (it does *not* silently default to 0.0). So
`add(I, 1) at [tau]` with `tau` a parameter is a **hard compile error** today,
not a silent collapse. There is no path to an estimable *event* time without
lifting that restriction — which is exactly why mechanism A is dispreferred and
mechanism B (a *rate*, not an event) is the recommendation. Three ways to seed:

**(A) Parameterized event time** — lift the E401 restriction, evaluate fire
times at runtime; τ enters as a hard import pulse at τ. *Likelihood:*
discontinuous (the trajectory jumps as τ crosses a substep boundary).
*Backends:* IF2's random walk handles it poorly; NUTS/gradient methods are
**broken** (zero gradient almost everywhere). **Do not use for any gradient or
smooth-proposal backend.** Lifting E401 also runs against camdl's "no loose
semantics" stance unless the runtime-evaluated time is fully specified — a
separate decision from this proposal.

**(B) Continuous external importation pulse** — add a source-less inflow
`--> I` (or `--> E`) with rate `λ · smooth_pulse(t; τ, w)`, a smooth influx
centered at / switching on near τ with width w. *Likelihood:* smooth.
*Backends:* yes — τ is an ordinary smooth rate-expression parameter; gradients
flow, IF2 / PMMH / PGAS / NUTS all "just work." **Recommended primary.**
§11 shows this is expressible today with `exp`, the reserved time symbol `t`,
and existing source-less-inflow syntax.

**(C) Estimate t_start** — shift the whole trajectory's time origin; τ is
implicit via burn-in length before the first observation. *Likelihood:*
smooth-ish but couples to obs alignment. **Recommended secondary**, especially
in the EpiNow2 disciplined form (§3.3).

### 4.1 The width parameter `w`: what it is and why it earns its place

`w` is the temporal width of the importation pulse — the parameter that turns a
discrete "seed at τ" into a smooth, differentiable influx. In
`λ · smooth_pulse(t; τ, w)`: τ is the seed onset/center time, λ the strength of
the external influx (individuals/time), and **`w` the duration over which that
influx is active**. It is a *time scale*, not a magnitude.

By kernel:
- **Logistic soft-onset** `λ · σ((t − τ)/w)`, σ the logistic function: `w` is
  the timescale over which the influx switches from ≈0 to ≈λ. Monotone
  "switch-on," **no pre-τ influx** — the better default for introduction
  semantics. Expressible as `λ / (1 + exp(−(t − τ)/w))`.
- **Gaussian bump** `λ · exp(−(t − τ)² / (2w²))`: `w` is the standard
  deviation; importation is concentrated within ≈±w of τ. Symmetric and
  infinitely smooth, but has an **acausal tail** (nonzero influx before τ) —
  acceptable only when `w` is small relative to the generation interval.

**Why finite `w` is necessary (the differentiability argument).** A truly
instantaneous seed (a Dirac pulse at τ — mechanism A) makes the latent
trajectory a *step function* in τ: the likelihood is piecewise-flat with a cliff
where τ crosses a substep boundary. Its derivative with respect to τ is zero
almost everywhere, so gradient-based backends (NUTS, any autodiff path) get no
signal, and IF2's random walk sees a flat surface with a sudden jump it cannot
climb toward. A finite `w` replaces the cliff with a ramp, making the likelihood
smooth in τ so the gradient actually points at the right τ. **This smoothness is
the entire reason mechanism B is preferred over A — `w` is the device that buys
it.**

**`w` is the identifiability dial.** As `w → 0` the pulse approaches the
discrete event: maximal timing resolution in principle, but a stiff surface that
every sampler handles badly. As `w` grows the surface becomes smooth and easy to
explore, but "seeding" becomes a slow ramp and τ (its center) blurs into the
origin-time / burn-in question (E2). So `w` trades sampler health against
temporal resolution on τ, and directly sets the **width of the τ posterior**. It
is a modeling knob with a clear interpretation, not a nuisance to hide.

**Default and exposure.** Default `w` to ≈ one **generation interval** (mean
time from one infection to the infections it causes — the natural timescale of
transmission): short enough to preserve meaningful timing resolution, long
enough to keep the surface smooth. Expose it as a declared parameter with that
default; let the user *fix* it (treat as a smoothing bandwidth) or *fit* it with
a prior. **Never bake it in as a hidden constant** — a silent `w` is a silent
determinant of how precise τ looks.

**Disambiguation.** This importation-pulse width `w` is **distinct from** the
burn-in window length t_seed of the EpiNow2-style mechanism (C, §3.3): t_seed is
a *fixed pre-observation period* over which an initial trajectory is estimated;
`w` is the *active duration of an importation bump near τ*. Same width
intuition, different roles — keep them named differently so they never get
conflated.

**Lineage margin note.** `w` also governs how spread-out-in-time the imported
lineages are, hence how monophyletic vs polyphyletic the seeding looks in a
genealogy (one tight introduction → one deep MRCA; a broad pulse → several
staggered Import-rooted lineages). When genealogical data eventually enters the
likelihood (§8), the tree supplies a second, independent handle on `w` and τ —
the concrete reason to leave `w` fittable. Detail in §8.

### 4.2 smooth_pulse default

Recommend the **logistic soft-onset** as the default kernel (causal, no pre-τ
influx), with the Gaussian available where a symmetric bump is wanted and `w`
is small.

---

## 5. Refactor the `ivp` flag (prerequisite for the *magnitude* case, count-layer only)

The existing `ivp` flag conflates two orthogonal things (verified in §11):
1. **perturbation scope** — `if2.rs` uses `ivp` to *skip* a parameter from the
   standard per-observation random-walk perturbation (effectively "perturb only
   at t=0");
2. **initialization kind** — `pgas.rs` uses `ivp` to draw the initial
   compartment count as `Binomial(N, p)`.

A seed-*time* parameter wants *neither* cleanly: it is a **global timing
parameter** that affects the whole trajectory but has no Binomial initial draw.
Split `ivp` into two explicit attributes:

```
perturbation_scope : T0Only | Global
init_kind          : Fixed | BinomialDraw | None
```

Then τ (mechanism B) is `{ Global, None }`; a classic seed magnitude is
`{ T0Only, BinomialDraw }`; t_start (C) is `{ Global, None }` plus an
obs→time-map flag.

**Important sequencing fact (verified, §11):** mechanism B's τ does **not**
require this refactor to *start*. A plain, non-`ivp` parameter is already
`{ Global perturbation, no Binomial init }` — exactly what τ wants. The refactor
is needed for the clean seed-*magnitude* case (E3 with an estimated N_seed) and
to make the semantics explicit; it is **not** a blocker for the headline E1
build. Because it edits `if2.rs` and `pgas.rs` — inference math, high-risk per
`CLAUDE.md` — it should be its own carefully-reviewed slice: read full functions,
green tests at each step, no batching with the fixture work. This is purely a
**count-layer** concern: per §8 the lineage layer reads but never writes the
count process, so it adds no entanglement here.

---

## 6. Two independent estimators — not redundancy, triangulation

The naive "implement two for robustness" is a trap: two *mechanistic* variants
(e.g. B and C) share the same exponential-growth likelihood ridge, so they fail
in correlated ways and agree right when they're both wrong — manufacturing
confidence. Robustness comes from **methodological independence**.

### 6.1 The three estimators

- **Mechanistic smooth-importation (B)** — full state-space likelihood;
  posterior over τ consistent with the rest of the model.
- **Non-mechanistic deconvolution (§6.2)** — no transmission assumption;
  recovers the latent infection curve from the delay convolution alone. The
  cheap, always-available cross-check today.
- **Genealogical likelihood (§8)** — the *principled* independent estimator: it
  informs the early/deep phase that incidence data barely constrains, so
  incidence-vs-genealogy disagreement on τ is the ridge being resolved (or a
  transmission-model misspecification) — the diagnostic you actually want.
  Available once the lineage Tree-likelihood path lands.

When estimators disagree on the seeding window, the disagreement *localizes* the
fault (misspecified generation interval / FoI form / reporting structure) rather
than being a tie to average away. This is the blameless-postmortem use of a
second method.

### 6.2 The deconvolution baseline, in detail

**The model.** Let `i(s)` be the latent infection-incidence curve and `f(d)` the
known infection-to-observation delay distribution (the convolution of the
incubation/latent period with the reporting/ascertainment delay). Then expected
observed counts are

```
E[c(t)] = ρ · Σ_{s ≤ t} i(s) · f(t − s)
```

a discrete convolution of infections with the delay kernel, scaled by the
detection probability ρ. **Deconvolution inverts this** to recover `i(s)` from
observed `c(t)` and a specified `f`. The recovered `i(s)` is the latent
infection curve; **its left tail — the first time infections rise above zero —
is the seeding signal.** τ is read off the recovered curve, not from any
transmission model.

**Why it is methodologically independent of B.** Mechanism B's τ comes from
fitting a transmission model whose growth rate `r` is itself estimated; B's τ
and `r` share the exponential-growth ridge (§2). Deconvolution makes **no
transmission assumption at all** — it uses only the delay kernel `f`, which is
estimated from independent line-list / contact-tracing data (incubation and
reporting-delay studies), *not* from the case curve being deconvolved. So B and
deconvolution draw their information from disjoint sources: B from the *shape of
growth* under a mechanistic model, deconvolution from the *delay structure*
between infection and observation. They do not share the ridge, so when they
agree the agreement is evidence; when they disagree, the disagreement localizes
which assumption (transmission form vs delay kernel) is wrong.

**Method lineage and the stability trap.** The classical approach is
back-projection / back-calculation: Brookmeyer & Gail (1988, *JASA*) and Becker,
Watson & Carlin (1991, *Biometrics*) developed it for AIDS incidence
reconstruction. The naive inverse is **ill-posed**: deconvolution is numerically
unstable (small noise in `c(t)` produces large oscillations in the recovered
`i(s)`), and **right-censoring** (recent infections not yet observed) biases the
right end of the curve — exactly the wrong place if one is not careful, though
for *seeding* we care about the left end. The expectation–maximization
smoothing / Richardson–Lucy iterations classically used are unstable without
regularization. The modern fix is **regularized deconvolution**: Miller, Hannah
et al. (2022, *Epidemiology*; the `incidental` R package / penalized-likelihood
deconvolution) add a roughness penalty that stabilizes the inverse and handles
right-censoring. **A common naive shortcut — backward-shifting reports by the
mean delay — is biased** (it ignores the delay *distribution*'s spread) and
should not be used.

**What camdl consumes and emits.** Input: an observed count series and a delay
distribution `f` (specified as a parametric delay or an empirical PMF). Output:
a recovered latent infection curve `i(s)` with uncertainty, and a derived τ
estimate (the onset of the recovered curve, with an interval). It is
**standalone** — it does not touch the simulation backends, the lineage layer,
or the inference stack; it is a pure transform from `(counts, delay)` to
`(infection curve, τ)`.

**Native vs wrap (open question Q3).** Two paths:
- *Native Rust implementation* — a self-contained regularized deconvolution
  (penalized Poisson likelihood with a roughness penalty), type-safe, no
  external dependency, fully reproducible under camdl's seed/provenance model.
  More code, but it lives entirely inside the toolchain.
- *Wrap `incidental`* — shell out to the validated R package behind `camdl
  compare`. Faster to ship and uses a peer-reviewed implementation, but adds an
  R dependency and an external-process seam.
  Recommendation: **native**, on the grounds that this output may inform outbreak
  response and a self-contained, deterministic, reviewable implementation is
  worth the extra code — but flag this for decision (Q3). Either way it surfaces
  in `camdl compare` as an independent reference τ alongside mechanism B's
  posterior.

### 6.3 Recommended build order

1. **Mechanism B** (continuous importation), E1 default with E3 reporting
   layered on (§7), seed expressed as a lineage-trackable Import inflow (§8).
   *Highest value, lowest architectural risk — and buildable today (§11).*
2. **Ridge reporting** (§7) — the genuinely new output work; the highest-value
   honest deliverable.
3. **`ivp` → `{perturbation_scope, init_kind}` refactor** (§5) — its own
   careful inference-code slice; unblocks the clean seed-magnitude case (E3).
4. **Deconvolution baseline** in `camdl compare` (§6.2) — independent reference.
5. **(Optional) Mechanism C / t_start** in the EpiNow2 disciplined form.
6. **(Future) Genealogical likelihood** on the shared τ once the Tree-likelihood
   path exists (§8).

Avoid A entirely except as a deliberately-labeled "exact event" mode for
non-gradient experiments, with a loud warning that it breaks differentiable
inference.

---

## 7. The ridge is a feature, not a caveat

Per the explicit goal that "sometimes the ridge is nice to see," E3 should be a
first-class reporting mode, not a failure state. When seed size is *not*
strongly prior'd, the deliverable is:

- the **2D joint posterior** over `(τ, N_seed)` (or `(τ, r)`), rendered, not
  summarized away;
- the **principal ridge direction** — the dominant sloppy eigenvector of the
  local FIM (or the leading PCA axis of the posterior sample) — reported as the
  combination the data actually constrain, e.g. "`log N_seed − r·τ` is pinned to
  X ± Y; the orthogonal direction is prior-dominated";
- a **prior-vs-data decomposition**: how much of τ's marginal width collapses as
  the seed-size prior is tightened. This makes epistemic laundering impossible to
  do by accident — borrowed precision becomes visible.

This dovetails with prequential evaluation: a model non-identifiable in
`(τ, N_seed)` can still be perfectly calibrated in *forecast* terms, and
PIT/CRPS on held-out incidence will show the ridge doesn't hurt predictive
performance even when it wrecks parameter identifiability. Showing both axes
side by side is the honest story.

---

## 8. Designing for joint inference: the lineage path (don't block it)

camdl has a lineage layer (PR #65) that generates transmission genealogies from
the same compartmental dynamics, and we will eventually want *joint* inference
of seed timing with genealogical / lineage-type data. The seed feature must not
foreclose that. The lineage internals are out of scope here; what matters is a
few design contracts.

**The factorization sets the seam.** The lineage layer factors the augmented
model as `P(augmented) = P(counts) × P(identity | counts)`: the count trajectory
is fixed first (identity-free, byte-identical), and the genealogy is a
conditional sample on top — reading the count process but never writing it.
Consequence: **τ lives entirely in `P(counts)`.** The seed mechanism needs no
awareness of the lineage layer for forward simulation; the two couple only in
the *inference* direction, where the incidence likelihood and the genealogical
likelihood both read the same τ and sum. (This is also why the §5 refactor is
count-layer only.)

**Seed as a lineage-trackable Import inflow (the load-bearing contract —
already met).** Implement the seed influx as an ordinary **source-less inflow**
into a tracked compartment (`--> I`, with `I` reachable from a `#[lineage]`
transmission so it is in the identity-tracked subgraph) — *not* a special-cased
initial-condition tweak.

> **Correction from the prior draft.** The earlier draft said the seed inflow
> should "carry the `#[lineage]` annotation." That is the wrong mechanism.
> `#[lineage]` marks a *transmission* (sample a parent from the infector pool);
> an importation has **no infector** — it is minted with `parent = Import`.
> Putting `#[lineage]` on an inflow with no infector pool is meaningless. The
> correct contract is "a **trackable importation inflow** whose destination is
> in the identity-tracked subgraph," which the runtime already mints as `Import`
> automatically.

This contract is **already satisfied by the current runtime** (verified, §11):
the event recorder logs any transition whose destination is a tracked
compartment (`touches_tracked`), and the realizer mints `ParentRef::Import` on a
source-less inflow `(None, Some(dst))`. So a seed inflow into a tracked
compartment is recorded and Import-rooted with **zero new lineage code**, and
the timing of those Import-parented mints *is* τ — the tree's deepest structure
encodes the seeding for free. A back-door IC tweak that isn't a trackable
transition would be invisible to the genealogy; that is the one design choice
that *would* block joint inference, and mechanism B avoids it by construction.

**`w` gains a genealogical interpretation.** Once the genealogy is in the
likelihood, `w` controls seeding polyphyly, and the LTT curve (its left edge is
the genealogical analog of the TMRCA) carries τ. The genealogy informs the
early/deep phase that incidence data barely constrains — i.e. it can break the
ridge case counts cannot. This is the concrete payoff of leaving `w` fittable
(§4.1).

**Genomic information: likelihood, not (just) prior.** An earlier draft had the
VP1 molecular clock entering as a fixed prior on τ — a *cut* model that
firewalls genomic data into the prior. Given the lineage architecture,
hard-coding that cut is exactly what blocks joint inference. The coherent design
treats genealogical / genomic information as an *attachable likelihood term* on
the shared τ — the per-event line-list attribution log-probability the lineage
layer already accumulates, and eventually a Tree-level likelihood — parallel to
the incidence observation model. Keep the prior/cut as a deliberate **fallback**
(and as the firewall option when one stream is suspected misspecified).

> **Caveat from PR #65 (honest scope).** What is built today are **transmission
> trees with infection-time branch lengths**, *not* viral phylogenies with a
> molecular clock. So real VP1 sequence-divergence data still enters as an
> external prior on τ until an in-model mutation/clock layer exists. The
> transmission-tree *structure* (coalescent intervals scale as ~1/I(t), so deep
> intervals reflect the small early I) already carries timing information about
> τ through topology alone.

**Per-type seeding.** The lineage layer already does stratified, per-deme
attribution. For multi-emergence settings (several cVDPV2 emergence groups, or
any lineage-type observation stream), seeding should be indexable: a vector
`{τ_k}` of seed times, one per imported lineage/type, each an Import-rooted
inflow carrying its type tag. Parameterize seeding as repeatable-per-import-
source from v1 (even if v1 exercises one) so the multitype joint model is an
extension, not a rewrite.

**Inference discipline: one genealogy is a conditional sample, not data.**
Because the line list is a single draw from `P(identity | counts)`, τ must not
be fit to one realized tree as if deterministic — a single sampled tree's
apparent root time is a noisy estimate of τ. The seed-time posterior must
integrate over the identity/sampling stochasticity (marginalize the ensemble, or
go through summary-statistic / synthetic-likelihood summaries such as the LTT),
exactly as the factorization implies.

---

## 9. Evaluation plan

- **Simulation recovery (identifiability stress test):** simulate from known τ;
  check posterior recovery (i) seed size known, (ii) weakly prior'd, (iii) joint
  with N_seed. Expect tight recovery in (i), ridge-shaped posteriors in (iii).
  The failure in (iii) is *expected and correct*, not a bug.
- **Width sensitivity:** sweep `w`; confirm the narrow/wide tradeoff (§4.1) and
  pick a default tied to the generation interval.
- **Cross-method agreement:** run B and the deconvolution baseline on the same
  synthetic and real series; characterize when they agree, and construct at
  least one misspecification case where they should and do diverge.
- **Forward genealogical consistency (pre-joint):** simulate with known τ,
  generate a genealogy via the lineage layer, and check that LTT /
  coalescent-interval summaries are consistent with the count-fit τ. This
  validates the §8 connection before the Tree-likelihood path exists.
- **Prequential / forecast scoring:** PIT histograms and CRPS on held-out
  incidence, to show identifiability and calibration are separate axes.
- **Backend coverage:** confirm B gives consistent posteriors across IF2, PMMH,
  PGAS/NUTS; a discrepancy is a plumbing signal.

---

## 10. Open questions / scoping decisions

1. **Default estimand:** E1 (τ, seed size prior'd) as the headline with E3
   reporting always on? Or E3 default with opt-in pinning? *(Recommend E1
   headline + E3 reporting on.)*
2. **Import-inflow contract:** RESOLVED — the current runtime already supports
   source-less inflow into a tracked compartment with `parent = Import` minting
   (§8, §11). No extension needed for the forward/realize path.
3. **Deconvolution baseline:** native Rust implementation vs wrapping
   `incidental` behind `compare` (§6.2). *(Recommend native; flagged for
   decision.)*
4. **Obs→time mapping for mechanism C:** how invasive is making t_start
   participate in observation alignment? Determines whether C is a fast-follow
   or a larger lift.
5. **Shared-τ plumbing:** when the Tree-likelihood path lands, does τ plug into
   it via a simple shared-parameter likelihood sum without restructuring the
   seed mechanism? (If §8's contracts hold, it should.)

---

## 11. Verification against the current codebase

This section grounds the design in the code as merged on `main` (post-PR-#65),
so the implementer is not working from assumptions. Four facts were checked.

**(V1) Non-constant event times raise E401 — not a silent 0.0 collapse.**
`ocaml/lib/compiler/expander.ml` `resolve_float_expr` (≈line 1727) tries
const-evaluation, then IR reduction; if neither yields `Ir.Const`, it emits
`Diagnostics.error … ~code:"E401" "expected a constant expression"`. Schedule /
`at = [...]` times route through this. So mechanism A is blocked today by a hard
error (consistent with "no loose semantics"), and the prior draft's "silently
collapses to t = 0" was incorrect. *(There is a separate
`resolve_float_expr_simple` that returns 0.0 on non-const, used only for prior
bounds resolution — not for schedule times.)*

**(V2) Mechanism B is expressible in the DSL today — no new primitive.**
- `exp` is a recognized math function in rate expressions (e.g. the `e301`
  golden fixture uses `exp(I)`); dimensional analysis requires a dimensionless
  argument, and `(t − τ)/w` is time/time = dimensionless, so the logistic
  `λ / (1 + exp(−(t − τ)/w))` type-checks.
- `t`, `t_start`, `t_end`, `dt` are reserved time symbols (`expander.ml`
  `reserved_time_names`), so bare `t` in a rate resolves to simulation time.
- Source-less inflows already parse and run (`sir_demography.camdl`:
  `birth : --> S @ mu * N`).
  The seed transition `seed : --> I @ lambda / (1 + exp(-(t - tau)/w))` compiles
  and simulates with `tau`, `lambda`, `w` as ordinary parameters.

  **Implemented and verified.** The fixture
  `rust/crates/sim/tests/fixtures/seed_timing.{camdl,ir.json}` and the CLI
  end-to-end tests `rust/crates/cli/tests/seed_timing_e2e.rs` exercise this:
  the seed inflow is Import-rooted (V3); a particle-filter likelihood profile
  over τ is peaked at the true seed time (E1 identifiability); and `lambda`
  compiles as a `positive` parameter (the dimensional checker emits only a
  benign `I300` info on `w`'s dimension — no annotation required).

  **One bug surfaced and was fixed during implementation.** A rate using bare
  `t` was *not* classified as time-dependent (only named `TimeFunc` forcings
  were), so the **Gillespie** backend froze the seed propensity at its `t=0`
  value — silently producing wrong dynamics (a late seed produced zero inflow
  and no epidemic). The chain-binomial / tau-leap backends were unaffected
  (they re-evaluate every substep). Fixed by extending the classifier to
  `Expr::Time`; see
  `docs/dev/incidents/2026-05-20-gillespie-bare-time-frozen-propensity.md`.
  So mechanism B worked on the fixed-step backends with no change; Gillespie
  needed this one sim-core fix (now landed).

**(V3) The lineage Import contract is already met.**
`rust/crates/sim/src/lineage/event_log.rs` (≈line 197) records a transition when
`touches_tracked = source∈tracked ∨ destination∈tracked ∨ any parent_pool∈
tracked`; untracked transitions are skipped. `rust/crates/sim/src/lineage/
realize.rs` (≈line 244) handles the `(None, Some(dst))` inflow arm by minting a
fresh id with `ParentRef::Import`. So a source-less inflow into a tracked
compartment is recorded and Import-rooted with no new code. (If `I` is the
destination of a `#[lineage]` infection, `I` is in `identity_tracked_
compartments`, so the seed inflow into `I` is tracked.)

**(V4) `ivp` conflates perturbation scope and init kind.**
`rust/crates/sim/src/inference/if2.rs` (line 386):
`if spec.ivp || simplex_member_indices.contains(&spec.index) { continue; }` —
`ivp` controls IF2 perturbation scheduling. `rust/crates/sim/src/inference/
pgas.rs` (≈line 506): `ivp_mappings` drive the `Binomial(N, frac)` initial-count
likelihood and sampling. Both behaviors key on the single `ivp` flag, confirming
the §5 conflation. A plain non-`ivp` parameter is `{Global, None}` — what
mechanism B's τ wants — so the refactor is not a blocker for E1, but it touches
inference math and must be a conservative standalone slice.

**Net consequence for sequencing.** The headline build (mechanism B, E1) needs
**no language primitive, no lineage change, and no inference-stack change** — it
is a fixture + an evaluation harness + ridge-reporting output (plus the one
Gillespie classifier fix noted in V2, now landed). The `ivp` refactor (§5) and
the deconvolution baseline (§6.2) are separable follow-ons.

**Status (2026-05-20).** Mechanism B is implemented and verified end-to-end
(fixture, Import-rooting, τ identifiability profile). Still to build, in the §6.3
order: (2) ridge reporting (§7) — the 2D `(τ, N_seed)` posterior +
sloppy-eigenvector + prior-vs-data decomposition; (3) the `ivp` refactor (§5);
(4) the deconvolution baseline (§6.2). The genealogical likelihood on τ awaits
the lineage Tree-likelihood path.

---

## References

**Identifiability / the ridge**
- Smirnova, A. et al. (2025). *Estimation of the exponential growth rate of an
  epidemic.* Infectious Disease Modelling. (Detection-rate / initial-cases ridge;
  MCMC non-convergence resolved by fixing the detection rate.)
  ScienceDirect S2468042725001484.
- Tien, J. H. et al. *On parameter identifiability in network-based epidemic
  models.* arXiv:2208.07543.

**Mechanistic state-space / PMCMC**
- Shi, B., Yang, S., Tan, Q. et al. (2024). *Bayesian inference for the onset
  time and epidemiological characteristics of emerging infectious diseases.*
  Frontiers in Public Health 12:1406566. (PMCMC onset-time on stochastic SEIR;
  Wuhan/Shanghai/Nanjing.)
- King, A. A., Nguyen, D. & Ionides, E. L. (2016). *Statistical inference for
  partially observed Markov processes via the R package `pomp`.* J. Stat. Softw.
  69(12). (`iota` import rate + `t0` origin.)

**Renewal / seeding window**
- Abbott, S. et al. `EpiNow2` — `estimate_infections()` model definition.
  (Seeding-window initialization; timing-as-magnitude.)

**Back-calculation / deconvolution**
- Brookmeyer, R. & Gail, M. H. (1988). *A method for obtaining short-term
  projections and lower bounds on the size of the AIDS epidemic.* JASA 83:301.
- Becker, N. G., Watson, L. F. & Carlin, J. B. (1991). *A method of
  non-parametric back-projection and its application to AIDS data.* Biometrics /
  Statistics in Medicine.
- Miller, A. C. et al. (2022). *Statistical deconvolution for inference of
  infection time series.* Epidemiology 33(4); `incidental` R package. (Robust
  regularized deconvolution.)

**Genealogical / phylodynamic**
- Volz, E. M. (2009). *Complex population dynamics and the coalescent under
  neutrality.* Genetics 183:1421. (Structured-coalescent rate λ = 2f/I² — the
  rate validated in PR #65's Tier-4.)
- Attwood, S. W. et al. (2022). *Phylogenetic and phylodynamic approaches to
  understanding and combating the early SARS-CoV-2 pandemic.* Nature Reviews
  Genetics 23:547. (TMRCA, introduction-to-detection lag.)
- cVDPV2 VP1 molecular clock: GPLN VDPV reporting guidance; nOPV2 emergence-risk
  monitoring.

**Stochastic early phase / first detection**
- Czuppon, P., Schertzer, E., Blanquart, F. & Débarre, F. (2021). *The
  stochastic dynamics of early epidemics: probability of establishment, initial
  growth rate, and infection cluster size at first detection.* J. R. Soc.
  Interface 18:20210575. (Accelerated early growth; first-detection-time
  distribution; UK Alpha dated to ~4 Aug 2020.)
- *Using early detection data to estimate the date of emergence of an epidemic
  outbreak.* (2024). PLOS Comput. Biol. 20:e1011934.
