# Concepts: the reasoning behind the fit workflow

The runbook (`camdl docs workflow`) gives the order of operations. This explains
_why_ — the identifiability logic, what priors are actually for, and the
epistemic stance that runs through all of it: **a failing diagnostic is
information, not a thing to tune away.** Read once at onboarding; the runbook is
enough for the per-fit loop.

## Synthetic recovery is necessary, not sufficient

Fitting synthetic data generated from _known_ truth tests the **pipeline**: did
the optimizer and sampler find the parameters you put in? It does **not** test
whether your model describes the **world**. A model can recover its own
synthetic data perfectly and still be wrong about reality — the synthetic data
came from that same wrong model.

So recovery is a gate you must pass _before_ real data, but passing it proves
only that the machinery works. Whether the model is _right_ is a separate
question, answered by real-data checks (predictive checks and external oracles,
below).

## Identifiability: what the data can and cannot pin

Some parameter combinations are not separable from the data alone. The WA-State
seeding example: the introduction time `τ` and the seed size `n_seed` trade off
along a **ridge** — a small _late_ introduction and a large _early_ one produce
the same observed growth curve. The data constrains their joint effect (the
early trajectory) but not the two parameters separately.

This is **structural non-identifiability**, and more sampling does not fix it —
the likelihood is genuinely flat along the ridge. You see it as a flat profile
likelihood, a posterior that pins to a bound, or chains that disagree (`R̂ ✗`).
The diagnostics in `camdl docs workflow` are how you detect it; the fix is
below.

## Why priors are load-bearing, not a nuisance

A prior is how you supply the information the _data_ lacks.

- On an **identified** parameter, a weak prior barely moves the posterior — the
  data dominates.
- On a **non-identified direction** (the ridge), the prior is what regularizes
  it: it selects the part of the ridge consistent with outside knowledge — a
  reference class ("how large are cryptic introductions, typically?"). This is
  legitimate, _as long as_ the prior is honest (grounded in knowledge, not data
  hindsight) and you report its influence with a prior-sensitivity sweep.

The trap is the mirror image: because the prior shows up in the posterior, a
prior chosen _to make the sampler converge_ launders your ignorance into a
false-confident answer. camdl refuses silent flat priors for exactly this reason
— the prior is a scientific claim, so it must be explicit and defensible, not a
knob you turn until the chains agree.

## A failing gate is information

Put the pieces together with the WA fit:

1. Uninformative priors → the `n_seed` posterior pins to its upper bound and
   `camdl fit summary` reports `max R̂ = 1.216 ✗`.
2. The instinct — "fix the convergence," run more sweeps, loosen the threshold —
   is backwards. The failing `R̂` correctly reported that the data do not
   identify `(τ, n_seed)` jointly. There was no well-defined posterior to
   converge _to_.
3. The fix was to add **structure** (weakly-informative priors), after which `R̂`
   passed _honestly_ — because the posterior was now actually well-defined and
   the ridge had collapsed to a bowl.
4. The result validated against an independent oracle: the `τ` posterior agreed
   with the Bedford genomic estimate (after the report-time-vs-infection-time
   correction).

Lowering the `R̂` threshold in step 2 would have produced a "converged" fit that
was meaningless. **Tuning a diagnostic away — wider tolerance, fewer chains, a
prior picked for convergence — is epistemic laundering: it manufactures the
_appearance_ of a good answer.** For software whose outputs inform public-health
decisions, that is the failure mode to fear most. A diagnostic that fails is
telling you something true about your model–data–prior triple; listen to it.

## `dt` and discretization bias

The `chain_binomial` and `tau_leap` backends discretize continuous-time dynamics
into steps of size `dt`. A coarse `dt` **systematically biases** the dynamics —
and therefore the estimates. The subtlety: synthetic recovery at the _same_
coarse `dt` will not reveal the bias, because it is baked identically into both
the data generation and the fit.

The post-fit Richardson check (re-score `θ̂` at `dt`, `dt/2`, `dt/4`) is what
catches it: if `θ̂` moves as you refine `dt`, your MLE is discretization-
dependent and you must refit at a finer step. Heuristic for a starting value: a
few steps per mean dwell time; halve and re-check the verdict when unsure.

## Validating against the world

Recovery and convergence say the machinery works and the posterior is
well-defined. They do **not** say the model is _right_. Three real-data checks,
in increasing strength:

- **Prior-predictive** — do the priors, _before_ seeing data, imply
  epidemiologically plausible epidemics? (If the 95% prior envelope sits far
  above the data, the priors are too loose.)
- **Posterior-predictive** — does the fitted model reproduce the observed data
  within its predictive bands?
- **External oracle** — the strongest test: agreement with an _independent_
  estimate (a genomic TMRCA, a serosurvey, a published result). When the oracle
  disagrees, that is a finding to surface — never a number to tune toward.
