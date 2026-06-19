# Diagnosing a fit that won't behave

A fit can fail in two fundamentally different ways, and they call for opposite
responses. Either the **model** is misspecified — it cannot generate data that
looks like what you observed, no matter the parameters — or the **inference** is
failing — the model is fine but the sampler or filter cannot navigate to the
answer. The symptom you see (chains that won't mix, a likelihood that won't
climb, an estimate pinned at a bound) usually does not tell you which one you
have. Tuning the sampler when the model is wrong wastes hours; fixing the model
when the sampler is the problem does nothing. The first job is therefore
**diagnosis, not tuning**.

This page is the decision tree. For how the particle filter, IF2, PGAS, and NUTS
actually work — and what each diagnostic column means — see
[`camdl docs inference`](inference.md); this page assumes that mechanics and
focuses on _which question to ask first_.

## 1. First question: is it the model or the inference?

Do not touch a sampler knob until you know. The single highest-return test is
**synthetic self-consistency**: simulate data from the model at a plausible
parameter vector θ, then re-fit (or re-filter) and see whether you recover θ.

```bash
# Generate synthetic data at a known θ
camdl simulate model.camdl --params theta.toml --obs synth.tsv --seed 1

# Re-fit with a fit.toml whose [data] points at synth.tsv, then compare the
# recovered estimate to theta.toml. (Data path lives in the fit.toml's
# [data] block — there is no --data flag on `camdl fit run`.)
camdl fit run fit_synth.toml --seed 2
```

Read the result this way:

- **Recovers θ on synthetic but fails on the real data → misspecification.** The
  inference machinery works; the model cannot reproduce the real data. Stop
  tuning and fix the model (a missing mechanism — seasonal forcing, a second
  introduction, reporting structure — is the usual culprit).
- **Fails even on synthetic → it's the inference.** The data came from exactly
  this model, so any failure to recover θ is the filter/sampler, not the model.
  Now sampler diagnostics are worth your time.

This test belongs **early**, before any sampler tuning — it flips the entire
diagnosis and costs one simulate-plus-fit.

## 2. "Looks fittable" ≠ "is fittable"

A likelihood landscape that looks smooth and peaked can still be unfittable, for
two reasons:

1. **A central value per grid point hides the per-evaluation noise the sampler
   actually eats.** For a stochastic model the likelihood at a fixed θ is itself
   a noisy Monte-Carlo estimate; a landscape that reports only a summary smooths
   that away.
2. **A likelihood-only landscape sampled over bounds ignores the prior**, so a
   direction the prior would downweight still looks freely explorable.

In camdl, `camdl survey` is exactly this likelihood landscape — it draws points
by Latin-hypercube over the declared `[estimate]` bounds. Two things to know:

- It does **not** silently smooth the noise. By default it runs several particle
  replicates per point and reports `loglik_se` (the replicate standard error on
  the log scale) and `mean_ess` alongside `loglik`, summarises the across-point
  SE distribution in `summary.json`, and warns when too many points exceed the
  ~1.7-nat reliability bar (Doucet et al.). **Read `loglik_se` and `mean_ess`,
  not just the shape of `loglik`.** A peak built from points with large SE or
  collapsed ESS is an artifact.
- It is **prior-free by construction**. The landscape is the likelihood, not the
  posterior, so a flat direction a prior would tame still looks explorable. Heed
  the bound-clustering warning (top points pinning against a bound) — that is
  the landscape telling you a direction is unidentified by the data alone.

## 3. Two inference failure modes — opposite fixes

If §1 pointed at the inference, separate these two, because their fixes are
unrelated.

**(a) Particle-filter marginal noise (ESS collapse).** Methods that feed on the
_marginal_ likelihood — PMMH, and IF2's particle ranking — break when the
filter's log-likelihood estimate is too noisy. First ask whether it is even
fixable: scale particles at one fixed θ and watch the loglik standard deviation.

```bash
camdl pfilter model.camdl --params theta.toml --data cases.tsv \
    --replicates 20 --particles 1000 --output ll_1k.tsv
# repeat at --particles 4000, 16000 and compare the reported loglik ± SD
```

If the SD falls like $1/\sqrt{N}$, more particles help. If it **plateaus**, no
particle count saves you — this happens when many observation streams are
observed at the same time (high _effective observation dimension_), where the
bootstrap filter is structurally inadequate. `camdl pfilter --pf-health`
measures this directly (the Snyder et al. 2008 $\exp(\tau^2/2)$ implied-N
estimate); see [`camdl docs inference`](inference.md) (the `--pf-health`
section). The fix there is a different method, not brute-force N.

**(b) Geometry (ridges, flat or stiff directions)** stalls gradient-based
PGAS-NUTS, which is a different problem with a different fix (reparameterize,
tighten priors, or add identifying data).

**Crucially: PGAS-NUTS is immune to the PF marginal noise of (a).** It runs on
the _smooth complete-data conditional_ likelihood — it conditions on a sampled
latent trajectory rather than marginalizing it out with a noisy filter — so
"PMMH is dead on this problem" does **not** imply "PGAS is dead." If marginal
noise is killing PMMH/IF2, PGAS is often still viable. (See the
marginalize-vs-condition contrast in [`camdl docs inference`](inference.md).)

## 4. Pinning parameters helps geometry, not PF noise

Fixing or pinning parameters (`--fixed name=value`) reduces dimension and can
unstick the **geometry** problem (b) — fewer ridges to climb. It does
**nothing** for the per-evaluation PF noise of (a): the ESS at a given θ does
not depend on how many parameters are free. If your problem is marginal noise,
pinning parameters will feel like it should help and won't. Attack the problem
you actually have.

## 5. Use the right "predicted value" for the diagnostic

Three different predictions answer three different questions; using the wrong
one hides the very misfit you're hunting.

- **Free-forward (unconditional posterior-predictive)** —
  `camdl simulate
  --replicates N` at the estimate. Exposes _generative_
  misspecification: can the model, run forward on its own, produce data like the
  observations?
- **One-step-ahead** — the `camdl pfilter --trace` predictive quantiles. The
  right tool for _timing_ questions (does the model anticipate each next
  observation?).
- **Conditioned / smoothed path** — `camdl pfilter --save-paths`. This is pulled
  toward the data by construction and will track it even for a misspecified
  model. It **cheats** for the purpose of model-checking: a smoothed ribbon that
  hugs the data is not evidence the model is right.

And beware the **mean** free-forward trajectory in a stochastic model: averaging
over replicates with jittered epidemic take-offs smears the peak later and lower
than any single run. For timing, show the quantile ribbon, not the mean. The
divergence between the unconditional ribbon and the smoothed ribbon _is_ the
diagnostic — see the "unconditional vs smoothing" plot in
[`camdl docs inference`](inference.md).

## 6. Read the MLE for "compensation" signatures

A point estimate can be a symptom rather than an answer. Watch for:

- a parameter **pinned at a bound** (survey's bound-clustering warning and the
  IF2 chain-agreement ranges surface this);
- an **unphysical** value — $R_0 < 1$ for an outbreak that visibly grew,
  overdispersion slammed to its maximum, a reporting rate at 0 or 1.

These are the optimizer contorting one parameter to absorb a structural misfit
elsewhere. When you see one, the estimate is telling you the model is wrong, not
giving you the answer — go back to §1.

## 7. The meta-lesson: a fighting sampler is doing model-checking

A framework whose particle filter degenerates and whose sampler stalls on a
misspecified model is, in effect, performing model criticism for you. That
pushback is a **feature**, not a defect to tune away. When the sampler fights
you, suspect the model before the sampler — §1 is how you confirm it.

## camdl-specific gotchas

Concrete things that trip up real fits, verified against the current code:

- **The IR cache does not track files loaded via `read()`.** After editing a
  file pulled in with `read()` (a `zones.tsv`, contact matrix, or population
  table) _without_ touching the `.camdl`, camdl can serve stale compiled IR —
  the cache key currently folds only the model file's own bytes. Pass
  `--no-ir-cache` after changing a `read()`-loaded file until the cache key
  learns to track them (gh#260).
- **PGAS (and PMMH) require a prior on every estimated parameter.** A parameter
  with no `~` in the model and no prior in the fit toml is a **hard error** that
  names the offending parameters and the three remedies — not a silent fallback.
  Declare priors via `~` in the model, in `[estimate.<param>].prior`, or opt
  into flat priors explicitly. (IF2 and the NLopt optimizers ignore priors.)
- **The ODE backend does MLE via `nl-sbplx` / `nl-bobyqa`, and Bayesian via
  `mh`** (Metropolis-Hastings on the deterministic marginal likelihood). PGAS
  and PMMH are **chain-binomial only** — they need stochastic process variance
  the ODE backend doesn't have, so asking for them on `ode` is a hard error that
  points you at `mh`. Run `camdl fit methods` for the current matrix.
- **The PGAS trace's loglik column is named `log_complete_data_ll`** — the
  complete-data conditional value (it conditions on the full sampled latent
  path: initial + transition + observation density), a large-negative number,
  **not** a marginal/PF likelihood. (PMMH and `mh` report the marginal in their
  `log_likelihood` column.) Don't compare PGAS's `log_complete_data_ll` to a
  `camdl pfilter` loglik — they differ by orders of magnitude.
- **The bootstrap particle filter degenerates with many simultaneous observation
  streams.** Per-stream likelihoods multiply into one weight, so high
  observation dimension collapses ESS and more particles only buy
  $\exp(\tau^2/2)$ headroom. Measure it with `camdl pfilter --pf-health` before
  scaling N (§3a).
