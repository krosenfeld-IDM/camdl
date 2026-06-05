# Proposal: Stuck-Chain Diagnostics for Posterior-Sampling Stages

**Status:** draft for discussion. Addresses gh#71. **Scope:** detect the
specific failure mode in which a posterior- sampling stage (PMMH, PGAS) reports
converged on cross-chain R̂ while the chains have not actually explored the
posterior — typically because all chains were initialized at a common point with
a step size too small to escape it. The current diagnostic suite passes this
case silently; the goal is to make the failure unmistakably visible without
annoying users whose chains are mixing fine. **Primary application:** the
seed-timing v2 chapter in camdl-book documents an incident where a 4-chain PMMH
fit with `init_method = "single"` and `rw_sd = 0.1` on a parameter living on
tens of days reported `R̂ ≤ 1.1` across all params while the posterior median sat
at the _starting value_ and the truth fell outside the 95% CI. The same pattern
is the canonical "joint stuck-state" failure mode in the convergence-diagnostic
literature (Vehtari, Gelman, Simpson, Carpenter & Bürkner 2021, _Stat. Sci._
36(4)).

> **Provenance.** Filed against gh#71. The math and primary sources in §§1.1,
> 2.2, and 4.4 were verified against the original papers: Vehtari et al. 2021 §3
> (necessary-not-sufficient framing); Doucet, Pitt, Deligiannidis & Kohn 2015
> _Biometrika_ 102(2):295–313 (PMMH log-likelihood-noise variance tradeoff);
> Sherlock, Thiery, Roberts & Rosenthal 2015 _Ann. Stat._ 43(1) (the
> optimal-noise-variance ≈ 3.3 result that bounds achievable PMMH ESS/N).
> Citation accuracy matters: the upstream issue claims (correctly) that camdl's
> current diagnostic surface is structurally incapable of catching this case,
> and the proposal needs to be airtight before we start asking users to install
> warnings on top of an existing 15+-config rollout.

---

## Summary

We propose adding three complementary diagnostic checks to camdl's
posterior-sampling stages — none of them new mathematical results, all of them
straightforward to compute from data the system already produces — wired through
the existing `rust/crates/sim/src/inference/diagnostic.rs` surface as new
`DiagnosticKind` variants. The checks are:

- **A. Pre-flight warning** when `init_method = "single"` and `chains > 1` on a
  posterior-sampling stage. Fires at config-load, costs nothing, surfaces before
  any compute is spent.
- **B1. R̂–ESS conjunction warning** when `max R̂ < 1.1` _and_ `min(ESS/N) < 5%`
  on any parameter. Fires post-fit on observed chain behaviour, independent of
  init choice. Threshold sits at the boundary of well-tuned PMMH territory
  (Sherlock et al. 2015, §4.4 below) — but the conjunction with "R̂ looks fine"
  is much rarer than either marginal, so cry-wolf risk is materially lower than
  an ESS-only check.
- **B2. Drift–vs–step-size warning** when, for some parameter, all chains
  satisfy `within_chain_sd[c] / rw_sd < 3`. Fires when the chain's empirical
  scatter is within a few proposal-SDs of where it started — algorithm-native,
  no dependence on declared bounds. Catches "stuck at start" specifically;
  complementary signature to B1.

We do **not** propose hard refusal of `init_method = "single"` + `chains > 1`
(Option C of the gh#71 menu). Migration cost on existing TOMLs is real,
single-init is legitimate for some workflows (profiling at a fixed θ, diagnostic
sweeps), and the A + B1 + B2 combination provides two independent off-ramps
before a user trusts a wrong answer. We can escalate to C later if the warning
regime fails to prevent recurrence.

**Severity ladder.** A is a warning. B1 is a warning at `min(ESS/N) < 5%` and a
verdict-gating **error** at `< 1%`. B2 is a warning. Verdict-gating means the
fit's `verdict` flag flips from `PASS` to `FAIL`; the user cannot silently
report a "passed" fit that tripped a verdict-gating diagnostic.

**Suppression** is per-stage, per-named-diagnostic-code, via TOML. Honored for
warnings (documented self-injury, survives in the archived config), **not**
honored for verdict-gating errors (CLAUDE.md "never lower the bar to make
something pass" rule). A verdict override exists at a separate level
(`--allow-broken-fits` on the CLI, written to `run_meta.json`) for the
legitimate reproducing-a-known-broken-historical-fit use case.

---

## 1. The failure mode

### 1.1 Why R̂ ≈ 1 doesn't imply convergence

The Gelman–Rubin–Brooks R̂ (Gelman & Rubin 1992; Brooks & Gelman 1998; modernized
in Vehtari et al. 2021) is

> `R̂ = √( (W + B/n) / W )`

where `W` is the mean within-chain variance and `B` the between-chain variance
over `n` iterations. If all chains start at the same point and the step size is
too small to escape that point's local neighbourhood, `B → 0` _by construction_
— there's nothing for the chains to disagree about — and `W` stabilises at the
local-noise variance of the chain. `R̂ → 1` regardless of whether the chains have
covered the posterior.

Vehtari et al. 2021 §3 (verbatim from their published abstract and
introduction): R̂ is a _necessary_ condition for convergence, not a sufficient
one. It detects _disagreement_ between chains, not _joint failure_. The
complementary diagnostic — start-value sensitivity from multiple dispersed
initial points — is what makes the disagreement test informative.
`init_method = "lhs"` (camdl's default) provides exactly this;
`init_method = "single"` opts out of it.

### 1.2 The incident (from gh#71)

A 4-chain × 3000-iteration × 800-particle PMMH fit on an SEIR synthetic-recovery
problem with `init_method = "single"`, all chains starting at `τ = −10.0`. The
`pmmh_summary.json` reported:

```
rhat:  tau = 1.043   beta = 1.083   sigma = 1.029   gamma = 1.114
       rho_max = 1.017  t_rep = 1.066   w_rep = 1.110   k = 1.061
ess:   tau = 72       beta = 105     sigma = 189     gamma = null
       rho_max = 65    t_rep = 30      w_rep = null   k = 160
```

R̂ for τ at 1.04 — well under the 1.1 convention, looks converged. Truth was at τ
= −15; posterior median came in at τ = −10.07 with 90% CI [−12.6, −9.0]. **Truth
fell outside the 95% CI.** A 5-chain start-sensitivity sweep at dispersed starts
(`−50, −30, −20, −10, −5`) revealed every posterior median landing within 0.1
days of its start — the chains weren't converging, they were anchored.

The only diagnostic that fired was the existing acceptance-rate warning at ~51%
(the canonical RWM target is around 23%; PMMH's is closer to 7%, so 51% reads as
"too hot"). It was dismissed as "slightly hot." The ESS column tells the
load-bearing story — every parameter at ESS/N < 2% — but ESS sits next to R̂ in
the report and the eye averaged them out.

The chapter writeup with figures and tables is at
`guide/fitting/seed-timing/draft.qmd` in camdl-book.

### 1.3 Why this matters disproportionately for camdl

Camdl's outputs feed real public-health decisions. The class of bug "passes all
visible diagnostics, returns the wrong answer" is the highest-cost failure mode
for the project — higher than crashes, higher than wrong-units errors, higher
than performance regressions. The cost asymmetry is enormous:

- False positive (warning fires when chains are actually mixing fine): user
  reads the warning, dismisses it. Moderate annoyance.
- False negative (no warning, chains report converged at a wrong answer): silent
  wrong inference in a public-health setting. The failure mode the seed-timing
  incident exhibits.

So the calibration target is _not_ "minimise false positives." It is "minimise
false negatives, with false positives capped at the level where users still read
the warnings." A noisier warning surface that catches the failure mode beats a
quieter one that lets it through.

A second consideration: a meaningful fraction of camdl fits are constructed by
coding agents. Agents trust diagnostic summaries literally. The diagnostic
surface needs to be agent-resilient — the signature an agent will report is
"what's in `pmmh_summary.json`," so the signature of failure has to be inside
that file, prominently, structured for programmatic consumption (the existing
`DiagnosticKind` tagged-enum scheme is the right shape).

---

## 2. Three complementary checks

### 2.1 Check A — pre-flight: `single` + multiple chains

**Trigger:** at config-load, when a stage has `algorithm = "pmmh"` or
`algorithm = "pgas"`, `init_method = "single"`, and `chains > 1`.

**Diagnostic kind:** `SingleInitWithMultipleChains { stage, n_chains }`.

**Severity:** `Warning`.

**Message:**

```
warning: stage 'posterior' (algorithm = pmmh) is configured with
init_method = "single" and chains = 4. All chains will start at
identical parameter values, which makes cross-chain R̂ structurally
unable to detect chains stuck at their starting point (Vehtari et al
2021, "Rank-Normalization, Folding, and Localization", Stat. Sci.
36(4)). Consider init_method = "lhs" (camdl default) or "prior_draw"
for posterior-sampling stages. Override with init_method = "single"
explicitly if you want this behaviour for a known reason (e.g.
profiling at a fixed θ, replicating a historical fit).
```

**Cost:** zero compute, fires before the fit runs.

**Cry-wolf risk:** moderate. Single-init is sometimes deliberate. The message
names the legitimate use cases explicitly so the user can identify whether the
warning applies to them.

### 2.1a Check A' — pre-flight: `lhs` + posterior-sampling stage

**Trigger:** at config-load, when a stage has `algorithm = "pmmh"` or
`algorithm = "pgas"`, `init_method = "lhs"`, and `chains > 1`.

**Diagnostic kind:**
`LhsInitWithPosteriorSamplingStage { stage, n_chains, algorithm }`.

**Severity:** `Warning`.

**Message:**

```
warning: stage 'posterior' (algorithm = pmmh) is configured with
init_method = "lhs" and chains = 4. Latin-hypercube initialisation
is space-filling-random and is NOT scored against the likelihood —
the K chain starts are drawn from the prior bounds without any
preliminary evaluation. For PMMH/PGAS (especially PGAS+NUTS), this
typically produces at least one chain starting in a region with
pathological geometry: extreme rate-expression values, divergent
trajectories during NUTS step-size adaptation, or DivByZero in
the rate evaluator.

Recommended alternative: init_method = "survey_top_k" (gh#51).
Workflow:

  # Once: build the likelihood landscape (CAS-cached afterwards).
  camdl survey --fit fits/your_fit.toml --points 2000 \
      -o results/surveys/your_landscape

  # Then: chain starts come from the top-K landscape rows.
  [stages.posterior]
  algorithm      = "pmmh"
  init_method    = "survey_top_k"
  survey_path    = "../results/surveys/your_landscape"
  survey_top_k_n = 4    # defaults to chains

Override with init_method = "lhs" explicitly if you want this for a
known reason (teaching examples, deliberately-unfocused start
distributions, small synthetics where data dominates).
```

**Cost:** zero compute, fires before the fit runs.

**Cry-wolf risk:** moderate. Plain LHS is the camdl default and a substantial
fraction of fits use it acceptably (small models, strong data, IF2 stages —
IF2's perturb-and-anneal design tolerates LHS init fine and the warning is
scoped to PMMH/PGAS only). The message names the legitimate use cases
explicitly. The cost of _not_ warning, demonstrated by the gh#81 incident, was
hours of pathological NUTS adaptation followed by a DivByZero with no upfront
signal that init_method was the load-bearing issue.

### 2.2 Check B1 — empirical: R̂–ESS conjunction

**Trigger:** post-fit, when `max_p R̂[p] < 1.1` and there exists a parameter `p`
with `ESS[p] / total_samples < 5%`.

**Diagnostic kind:**
`LowEssWithConvergedRhat { stage, parameters:
Vec<{ name, rhat, ess_per_sample }> }`.

**Severity:** `Warning` when `min ESS/N` is in `[1%, 5%)`. `Error`
(verdict-gating) when `min ESS/N < 1%`.

**Message (warning band):**

```
warning: stage 'posterior' reports converged (max R̂ = 1.05) but the
parameters listed below have very low effective sample size:

  parameter   R̂        ESS/N
  tau        1.043    0.6%
  t_rep      1.066    0.25%
  w_rep      1.110    null

ESS/N < 5% on parameters whose R̂ ≤ 1.1 is consistent with chains that
have not explored the posterior — typically because rw_sd is too
small for the parameter's natural scale, or all chains started at the
same point with init_method = "single" and the step size is
insufficient to escape the start basin. R̂ alone is a necessary, not
sufficient, convergence criterion (Vehtari et al 2021 §3).

Possible remedies:
  (i)   Increase rw_sd[tau, t_rep, w_rep] to the order of the
        expected posterior SD.
  (ii)  Switch init_method to "lhs" or "prior_draw" for dispersed
        starts (current init_method: "single").
  (iii) Increase n_iterations to give the chain more time to mix.
  (iv)  Reduce the estimate set or tighten priors. If multiple
        parameters trip this warning together, the model may be
        over-parameterized for the data — chains are exploring a
        non-identifiable ridge, not converging to a point. Consider
        fixing weakly-identified parameters from external evidence
        or removing them from [estimate].
```

**Message (error band):** identical text plus

```
This is severe enough (min ESS/N = 0.25%) that the fit's verdict has
been set to FAIL. To override: pass --allow-broken-fits on the CLI
(documented in run_meta.json), or fix the underlying issue per the
remedies above.
```

**PMMH-aware reasoning for the 5%/1% thresholds:** see §4.4.

**Cry-wolf calibration:** the _conjunction_ is much rarer than either marginal.
ESS/N < 5% alone fires routinely on well-tuned PMMH (PMMH has an inherent ESS
ceiling from the PF noise — Sherlock et al 2015). R̂ < 1.1 alone is the standard
convergence check. The conjunction fires only when the chain looks converged
_and_ is barely moving, which is precisely the failure-mode signature.
Empirically the incident triggers comfortably (R̂ ≤ 1.114 and ESS/N ∈ {0.25%,
0.54%, 0.6%, 0.9%, 1.3%, 1.6%}), well clear of the warning threshold and inside
the error band on two parameters.

### 2.3 Check B2 — empirical: drift relative to step size

**Trigger:** post-fit, when there exists a parameter `p` such that for every
chain `c`,

> `within_chain_sd[c, p] / rw_sd[p] < 3`.

That is: the chain's empirical scatter is within ~3 proposal-SDs of where it
started. A healthy random-walk MH chain typically shows
`within_chain_sd / rw_sd > 10`.

**Diagnostic kind:**
`AllChainsLowDrift { stage, parameter, drift_
ratios: Vec<f64> }`.

**Severity:** `Warning`.

**Message:**

```
warning: parameter `tau` has within-chain SD comparable to its
proposal SD on all 4 chains (ratios: 1.8, 2.2, 1.9, 2.4; threshold
3.0). This is consistent with the chain not having escaped the local
basin of its starting point: each accepted step moves the chain by
~rw_sd, and the empirical variance reflects that step size rather
than the posterior width.

Remedies (in order of likelihood):
  (i)   rw_sd[tau] is too small relative to the posterior scale for
        tau. Typical heuristic: rw_sd ≈ posterior SD / 4. Increase by
        a factor of 5–10 and re-run.
  (ii)  init_method = "single" with all chains anchored at the same
        point: combined with small rw_sd, none of the chains can
        diffuse appreciably. Switch to init_method = "lhs".
  (iii) The parameter is unidentifiable from the data and the
        posterior is genuinely as narrow as a few rw_sd: in this
        case, the warning is spurious and the user can suppress it
        per stage (§6). Verify by widening rw_sd and confirming the
        chain still concentrates at the same value.
```

**Why this metric and not the gh#71-proposed
`|θ_final − θ_start| /
bound_range`:** the bound-range denominator couples the
metric to declared parameter bounds. A parameter with bounds `[1, 10000]` and a
tight posterior near 100 has a healthy chain that moves only fractions of a
percent of `bound_range` — the metric would mark it as stuck. Using `rw_sd` as
the denominator is algorithm-native and bound-free: "the chain has moved a few
proposal-SDs" is a meaningful statement regardless of where the parameter's
bounds are set.

**Complementarity with B1.** B1 catches "looks converged but isn't mixing fast"
— a broader class of pathologies including under-tuned particle counts and step
sizes. B2 catches "literally hasn't moved from start" — narrower but with a
sharper remedy. Both can fire on the incident; the fingerprint of which fires is
informative for the remediation message.

---

## 3. Fingerprints and remedies

The three checks fire on different signatures and point to different fixes. The
fingerprint of which check (or combination) fires is itself useful information.

| Signature                            | What it means                                                | Primary remedy                                                    |
| ------------------------------------ | ------------------------------------------------------------ | ----------------------------------------------------------------- |
| A only (pre-flight)                  | Config opts into `single`; fit hasn't run yet                | Confirm `single` is intended; otherwise switch to `lhs`           |
| B2 only (drift low)                  | All chains barely moved; step size too small for start basin | Increase `rw_sd` _or_ switch init                                 |
| B1 only (R̂+ESS)                      | Chains are moving but autocorrelated, ESS below PF floor     | Increase `n_iterations`; investigate over-parameterisation        |
| B1 + B2                              | Classic stuck-at-init: small rw_sd _and_ shared start        | Increase `rw_sd` _and_ switch init                                |
| A + B1 + B2                          | The incident                                                 | All three remedies; consider whether the estimate set is too rich |
| B1 on many parameters simultaneously | Likely over-parameterised / non-identifiable ridge           | Reduce `[estimate]` set or tighten priors                         |

The "over-parameterised" remedy in the last row deserves its own mention because
it's the case the seed-timing chapter taught us: a scout fit had R̂ ≈ 20 on `k`
and R̂ ≈ 18 on `t_rep`, and the right response wasn't "more iterations" — it was
"case-only data doesn't identify these parameters, fix them or drop them." The
B1 warning text needs to surface this remedy (remedy (iv) in §2.2) explicitly
when multiple parameters trip the conjunction at the same time.

---

## 4. Thresholds, severities, and the verdict gate

### 4.1 ESS/N: warning at 5%, error at 1%

The choice rests on three considerations:

1. **The PMMH floor.** Sherlock et al. 2015 show the optimal
   log-likelihood-estimator noise variance for PMMH is `σ² ≈ 3.3`. At that
   operating point the chain incurs an asymptotic-variance penalty over exact
   MH, and published PMMH applications (e.g. He, Ionides & King 2010, _J. R.
   Soc. Interface_ 7:271–283) report ESS/N in the single-digit-percent range
   when well-tuned. So `5%` is on the boundary of "well-tuned PMMH" territory.
2. **The incident's calibration.** The seed-timing incident sits at ESS/N ∈
   [0.25%, 1.8%] on every offender. 5% catches all of them comfortably; 1%
   (error band) catches the worst two.
3. **The conjunction.** Either threshold considered alone has a meaningful
   false-positive rate on well-tuned PMMH. The conjunction with R̂ < 1.1 reduces
   this — chains that legitimately sit at low ESS for PF reasons typically
   _don't_ present as converged on R̂, because the high autocorrelation surfaces
   as between-chain disagreement at finite chain length.

Both thresholds are configurable in TOML
(`[diagnostics.thresholds]
low_ess_warning = 0.05`, `low_ess_error = 0.01`) so
users with unusual PMMH operating points can tune them. The defaults are what
ships.

### 4.2 R̂ threshold

`R̂ < 1.1` is the conventional "looks converged" cutoff and is what the incident
report cited (every reported R̂ ≤ 1.114, with most ≤ 1.07). Vehtari et al. 2021
advocate the tighter `R̂ < 1.01` as a modern standard, but the failure mode we're
trying to catch is _specifically_ the case where R̂ falsely-passes — the looser
the cutoff for "looks converged," the more failure cases the conjunction
detects. We use 1.1 deliberately, not 1.01.

### 4.3 Drift threshold

`within_chain_sd / rw_sd < 3` is conservative. A healthy RWM chain explores at
posterior scale; if the posterior is well-resolved by the proposal
(`rw_sd ≈ posterior_sd / 4` is the standard heuristic), then
`within_chain_sd ≈ posterior_sd ≈ 4 × rw_sd` — well above 3. A chain that has
barely escaped its start has `within_chain_sd ~ rw_sd`. The threshold sits at
"moved a few steps but not exploring."

### 4.4 Severity = error means verdict FAIL

The fit-verdict gate at `Severity::Error` is _the_ load-bearing mechanism that
makes this proposal worth implementing. A verdict flag that flips PASS → FAIL is
materially different from a louder warning:

- It surfaces in `pmmh_summary.json` at the top of the file, as a field that
  downstream tooling (camdl-book CI, vignette regression tests, fit-report
  renderers) check programmatically.
- It survives copy-paste of `R̂` numbers from the summary into a paper draft,
  because the verdict line is the first thing a human or agent sees.
- It cannot be hand-waved as "slightly hot."

The error fires only on the conjunction `R̂ < 1.1 AND ESS/N < 1%`, which is the
_silent_ failure mode. ESS/N < 1% alone with R̂ = 50 is already caught by
existing R̂ gating and reported as "obviously broken."

### 4.5 What about Option C (hard refusal of `single` + multi-chain)?

Deferred. Reasons:

1. Single-init is legitimate for some workflows: profiling at a fixed θ,
   deliberate replication of a paper's setup, diagnostic sweeps.
2. Migration cost on the 15+ existing book TOMLs that propagate `single` by
   copy-paste is real. Forcing migration is heavy-handed when warning surfaces
   (A + B1 + B2) would surface the issue in a way that prompts the user to fix
   the TOMLs without breaking them.
3. The cry-wolf risk of refusal is higher than the cry-wolf risk of
   verdict-gating: a user whose fit produces a noisy posterior but is actually
   fine is less harmed by a "FAIL verdict" they can override than by a "won't
   run unless you change this" they can't override silently.

We revisit C as a follow-up if the A + B regime fails to prevent recurrence.

---

## 5. Plumbing: the existing diagnostic surface

Camdl already has the right scaffolding.
`rust/crates/sim/src/
inference/diagnostic.rs` defines
`Diagnostic { kind, severity,
message, stage, timestamp }` with a tagged
`DiagnosticKind` enum. Call sites push variants; the collector handles
rendering, severity, hints, and serialization. Designed for programmatic
consumption by camdl-book, vignettes, and CI.

### 5.1 New `DiagnosticKind` variants

Three additions:

```rust
pub enum DiagnosticKind {
    // ... existing variants ...

    /// Check A: pre-flight warning when single init + multi-chain
    /// on a posterior-sampling stage.
    SingleInitWithMultipleChains {
        stage: String,
        n_chains: usize,
        algorithm: String, // "pmmh" or "pgas"
    },

    /// Check B1: post-fit warning/error when R̂ looks converged but
    /// ESS is low — the conjunction signature.
    LowEssWithConvergedRhat {
        stage: String,
        parameters: Vec<LowEssParam>,
        max_rhat: f64,
    },

    /// Check B2: post-fit warning when all chains have within-chain
    /// SD comparable to proposal SD on some parameter.
    AllChainsLowDrift {
        stage: String,
        parameter: String,
        drift_ratios: Vec<f64>,  // within_chain_sd[c] / rw_sd, per chain
        threshold: f64,
    },

    /// Check A': pre-flight warning when `init_method = lhs` is used
    /// on a PMMH or PGAS stage with multiple chains. LHS is
    /// space-filling-random and unscored; PMMH/PGAS need better
    /// chain starts to avoid pathological NUTS adaptation
    /// (gh#81 incident).
    LhsInitWithPosteriorSamplingStage {
        stage: String,
        n_chains: usize,
        algorithm: String,
    },
}

pub struct LowEssParam {
    pub name: String,
    pub rhat: f64,
    pub ess_per_sample: f64,
}
```

The collector renders these to `pmmh_summary.json` (and the PGAS analogue), to
`diagnostics.tsv`, and to the CLI summary automatically. No new output surfaces.

### 5.2 CLI surfacing: the actual fix

This is the smallest change with the largest UX impact, and it deserves its own
callout. Today's CLI summary prints diagnostics in a table, where the
acceptance-rate warning sat next to the R̂ table in the incident and was
dismissed. After this proposal:

- **`Severity::Error` diagnostics print at the top of the CLI output**, before
  the R̂ / ESS tables, with a clear visual delineator (a banner line, not just a
  coloured `error:` prefix).
- **`Severity::Warning` diagnostics print after the tables**, but with the count
  surfaced in a one-line header (`2 warnings — see
  below`).
- **The fit verdict** (PASS / WARN / FAIL) prints as the very last line of CLI
  output, so a user grepping for it lands on it immediately.

Concretely: `rust/crates/cli/src/fit/runner.rs` and the markdown/TeX renderers
in `rust/crates/cli/src/fit/method_result.rs` need to learn to reorder by
severity. This is two well-scoped changes.

---

## 6. Suppression

Per-stage, per-named-diagnostic-code, via TOML:

```toml
[stages.posterior.diagnostics]
suppress = ["low_ess_with_converged_rhat", "all_chains_low_drift"]
```

The suppression list is matched against the `DiagnosticKind` variant name in
snake_case (matching the `serde(tag = "type", rename_all =
"snake_case")`
convention already in `diagnostic.rs`).

**Warnings: suppression honored.** The user typing out the diagnostic code by
name is "documented self-injury": it survives in the archived fit config,
reviewers can see what was waived. This is the appropriate friction level — high
enough to prevent accidental suppression, low enough to allow deliberate
suppression.

**Errors (verdict-gating): suppression NOT honored.** CLAUDE.md "never lower the
bar to make something pass" applies. The `low_ess_with_converged_rhat` check
fires at `Error` severity only in the `< 1%` band, which is unambiguously
broken. The legitimate "reproducing a known-broken historical fit" case is
handled at a different level: `--allow-broken-fits` on the CLI, written into
`run_meta.json` next to the seed and the git commit, where it's visible to every
downstream consumer.

The asymmetry — suppress at warning level but not at error level — is deliberate
and matches camdl's existing convention for the acceptance-rate diagnostic
(warning-suppressible) vs. the required-parameter-bounds diagnostic (error, not
suppressible).

---

## 7. Validation plan

### 7.1 Replay the incident

A test `single_init_pmmh_replays_seed_timing_incident` in
`rust/crates/cli/tests/`:

- Reproduce the incident's fit cell: SEIR synthetic recovery, 4 chains × 3000
  iterations × 800 particles, `init_method = "single"`, `rw_sd = 0.1` on a
  parameter living on tens of days, all chains starting at `τ = −10`.
- Run the fit through the diagnostic pipeline.
- Assert that **A + B1 (at error severity) + B2** all fire, with the expected
  parameter names in B1 and B2.
- Assert the fit `verdict` is `FAIL`.
- Snapshot-test the rendered CLI output so the prominence of the Error block is
  pinned.

If any of the three checks doesn't fire on the incident, the proposal is wrong
somewhere.

### 7.2 Synthetic stuck-chain cell

A standalone synthetic test that's faster than the full incident:
linear-Gaussian state-space model with a known posterior, set
`init_method = "single"` at a far-from-mode start, `rw_sd` at 0.001 of the
posterior SD. All three checks must fire.

### 7.3 False-positive guard on well-tuned PMMH

A test `well_tuned_pmmh_does_not_trip_stuck_chain_diagnostics` that runs the He,
Ionides & King 2010 measles fit (or a downsized version fast enough for CI) with
sane defaults and confirms **none** of the three checks fire. This is the "we
don't break users doing the right thing" guard.

If false positives show up on real fits in this regression test, the thresholds
need re-tuning — _not_ by raising them silently, but by re-examining whether the
conjunction logic in B1 needs additional gating (e.g., require all chains' ESS/N
to be low, not just one parameter's).

### 7.4 Verdict-gate regression

A test that explicitly checks the verdict flips PASS → FAIL when B1 fires at
error severity, and that `--allow-broken-fits` is the only way to override at
the CLI level. No TOML-suppression path should override the verdict.

### 7.5 Suppression honored for warnings

A test that explicitly suppresses `low_ess_with_converged_rhat` in the warning
band (ESS/N ∈ [1%, 5%)) and confirms the warning is not emitted but the
suppression decision is recorded in `run_meta.json` for traceability.

---

## 8. Caveats and non-goals

### 8.1 We are not fixing the underlying convergence problem

This proposal makes the failure mode _visible_. It does not fix chains that are
stuck — that's the user's job, with the remedies listed in the warning text. The
value is "users now know they're stuck" rather than "users no longer get stuck."

### 8.2 Threshold tuning will be iterative

The 5%/1%/3.0 numbers are educated calibration based on the incident and the
Sherlock et al. 2015 PMMH theory. Real-world fit corpora will reveal whether
they're well-calibrated. The TOML-configurable threshold knobs in §4.1 are for
power users now and the tuning hook for us if the defaults turn out to need
adjustment.

### 8.3 IF2 stages are out of scope

IF2 uses a perturb-and-anneal design around a single warm start;
`init_method = "single"` is the _correct_ default for IF2 (as the gh#71 issue
notes). The pre-flight check A is correctly scoped to posterior-sampling stages
only. The empirical checks B1 and B2 are not meaningful for IF2 either: the
post-IF2 parameter swarm is not a Markov chain in the convergence-diagnostic
sense.

### 8.4 PGAS+NUTS

PGAS's NUTS path has different mixing properties from random-walk MH (HMC
achieves much higher ESS/N at the same dimensionality), so the 5%/1% thresholds
may be miscalibrated for it. v1 applies the checks uniformly to PMMH and
PGAS-with-symmetric-MH; PGAS-with-NUTS needs its own threshold calibration based
on observed ESS/N distributions. This is an open empirical question deferred to
follow-up.

### 8.5 What we explicitly do _not_ implement

- **Hard refusal** of `single` + multi-chain (Option C). Migration cost >
  benefit at this stage. Reconsider if A + B regime fails.
- **Auto-fix.** No automatic `rw_sd` retuning or init-method switching. The
  diagnostics flag the issue; the user owns the fix.
- **Per-chain warnings.** All three checks aggregate across chains. Per-chain
  diagnostics exist elsewhere in camdl and are noisier; not the right surface
  for this failure mode.

### 8.6 Follow-ups

- **C as v2 if A + B fails.** Track recurrence in the gh tracker. If a similar
  incident slips through after this lands, escalate to hard refusal.
- **Auto-tune `rw_sd` from a pilot chain.** Mentioned as a remedy in the warning
  text; could be a separate feature where the user opts in to a pilot chain that
  calibrates `rw_sd` to ~`posterior_sd / 4` before the main run.
- **Per-NUTS calibration.** When the PGAS+NUTS code path is exercised in
  practice, calibrate B1's threshold for it specifically.
- **Diagnostic prominence audit.** §5.2 reorders the CLI output by severity; a
  broader pass on the diagnostic surface across all stages would be welcome but
  is out of scope here.

---

## 9. Decision points for RFC review

Items worth explicit sign-off before any code lands:

1. **5% / 1% ESS/N threshold values.** Conservative for PMMH per §4.4; reviewers
   from the PMMH-applications community (e.g. the Ionides group) may have
   empirical experience that argues for tighter or looser values.
2. **Error band gates verdict (§4.4).** Confirm that flipping PASS → FAIL on the
   conjunction signature is the right strength. Alternative is "stays at WARN
   but with a top-of-output banner."
3. **Suppression mechanism (§6).** Honored for warnings, not for verdict-gating
   errors. Confirm this asymmetry matches existing camdl conventions for other
   diagnostics.
4. **CLI re-ordering (§5.2).** Print Error diagnostics before tables, Warning
   diagnostics after, verdict on the last line. Confirm this is the right
   surface change vs. a more conservative "highlight Error in red" tweak.
5. **Drift metric (§2.3) uses `rw_sd`, not declared-bound range.** Cleaner
   mathematically, but `rw_sd` is per-parameter configuration — make sure all
   algorithm paths expose it for the diagnostic to read.

---

## References

Primary sources cited above were verified against the original publications.
Provenance:

- **Brooks, S. P. & Gelman, A. (1998).** General methods for monitoring
  convergence of iterative simulations. _J. Comput. Graph. Stat._ 7(4):434–455.
  The multivariate R̂ extension; cited for the standard convergence-diagnostic
  framing.
- **Doucet, A., Pitt, M. K., Deligiannidis, G. & Kohn, R. (2015).** Efficient
  implementation of Markov chain Monte Carlo when using an unbiased likelihood
  estimator. _Biometrika_ 102(2):295–313. https://doi.org/10.1093/biomet/asu075.
  Cited for the PMMH log-likelihood-noise variance tradeoff that underlies the
  5% ESS/N warning threshold.
- **Gelman, A. & Rubin, D. B. (1992).** Inference from iterative simulation
  using multiple sequences. _Stat. Sci._ 7(4):457–472. The original R̂. Cited for
  the formula in §1.1.
- **He, D., Ionides, E. L. & King, A. A. (2010).** Plug-and-play inference for
  disease dynamics: measles in large and small populations as a case study. _J.
  R. Soc. Interface_ 7:271–283. Cited for the empirical ESS/N range observed in
  real PMMH fits.
- **Sherlock, C., Thiery, A. H., Roberts, G. O. & Rosenthal, J. S. (2015).** On
  the efficiency of pseudo-marginal random walk Metropolis algorithms. _Annals
  of Statistics_ 43(1):238–275. https://projecteuclid.org/euclid.aos/1418135621.
  The optimal-noise- variance ≈ 3.3 result that bounds achievable PMMH ESS/N;
  cited in §4.4.
- **Vehtari, A., Gelman, A., Simpson, D., Carpenter, B. & Bürkner, P.-C.
  (2021).** Rank-Normalization, Folding, and Localization: An Improved R̂ for
  Assessing Convergence of MCMC. _Stat. Sci._ 36(4):667–718.
  https://projecteuclid.org/journals/statistical-science/volume-36/issue-4/Rank-Normalization-Folding-and-Localization--An-Improved-R%CB%86-for/10.1214/20-STS842.full.
  The modern R̂ reference; §3 establishes the necessary-not-sufficient framing
  this proposal builds on. Cited in §1.1 and §2.1.

Citations for the broader stochastic-epidemic and PMMH literature
(Andrieu–Doucet–Holenstein 2010; Britton & Pardoux 2019; etc.) are in
`docs/dev/proposals/2026-05-23-survival-conditioned-likelihood.md` and
`docs/methods/survival-conditioning.md`; cross-referenced rather than duplicated
here.
