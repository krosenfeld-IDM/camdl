# Proposal: Warm-Chain Init Mode for `camdl profile`

**Status:** draft for discussion. Addresses gh#74 Option A. Option B
(per-cell diagnostic columns) is being implemented in parallel and is
out of scope here.
**Scope:** add a new `--init warm-chain` mode to `camdl profile` that
processes the focal-parameter grid sequentially — each cell uses the
previous successful cell's MAP parameters as its warm start. Default
behaviour (independent-cell init from `--params`) is unchanged. The
classical profile-likelihood literature treats this as the canonical
traversal (Murphy & van der Vaart 2000 *JASA* 95(450):449–465; Pawitan
2001, *In All Likelihood* §3.4).
**Primary application:** seed-timing book chapter and similar
profile-PMMH workflows where the focal-parameter axis produces a
visibly jagged profile under independent-cell init because each cell
re-discovers the same nearby ridge independently. Warm-chain replaces
the per-cell re-discovery with a small local adjustment from the
previous cell's optimum, producing both faster per-cell convergence
and a smoother surface.

> **Provenance.** Filed against gh#74. The design questions in §3 are
> the ones the issue body itself left open ("simplest is left-to-right
> … better is bidirectional from a seed cell" etc.) — this RFC's
> contribution is settling each of them with a defensible default. The
> per-cell diagnostic columns from gh#74 Option B (the
> `loglik_spread_starts` / `loglik_rhat_starts` etc. landing
> separately) are what makes A's smoothness improvement measurable,
> so A and B compose; B is not a prerequisite for A but A's
> validation hinges on B.

---

## 1. The current behaviour and why it's noisy

`camdl profile` processes the focal-param grid as a parallel array
of independent per-cell inferences. For each cell, the per-cell
inference (PMMH or IF2) starts from the same warm point — typically
the global MLE supplied via `--params` — and tries to optimise the
nuisance parameters at the fixed focal-param value for that cell.

Three concrete consequences:

1. **Wasted compute.** Each cell re-discovers the same nearby ridge.
   The nuisance parameters' optimum changes only by `O(δψ)` between
   adjacent cells (where `δψ` is the focal-param step size), but the
   per-cell inference doesn't know that — it runs the full per-cell
   budget regardless.

2. **Surface noise.** Each cell's optimisation has independent
   stochastic error. The output `loglik(ψ)` therefore inherits
   cell-by-cell jitter on top of any genuine surface structure.
   Distinguishing "the profile is jagged because of MCMC noise" from
   "the profile is jagged because of genuine multimodality at this ψ"
   requires either many more samples per cell (expensive) or external
   knowledge of the surface.

3. **Misleading shape.** Adjacent cells can land in slightly different
   local optima of the nuisance-parameter ridge — even when there's
   only one global ridge — because each cell's optimisation is a
   short stochastic search. The resulting `loglik(ψ)` curve has
   spurious wiggles that aren't features of the actual profile.

The seed-timing chapter's incident (`guide/fitting/seed-timing/draft.qmd`):
PMMH over τ ∈ [−35, −1] with 30 cells, `--starts 3`, `--particles 800`,
`--pmmh-steps 1500`. Adjacent cells fluctuate by 1–2 nats. Even after
gh#73 fixed the priors-aren't-honored bug, the surface stays jagged —
this is the residual noise warm-chain is meant to remove.

## 2. The warm-chain construction

Process the focal-parameter grid in a deliberate order, with each
cell's per-cell inference initialised from the previous successful
cell's MAP parameters rather than from `--params`. The classical
references frame this as walking along the profile-likelihood ridge:
each cell only needs to perturb the nuisance parameters by `O(δψ)`
relative to its neighbour, so per-cell convergence is much faster
and the resulting `loglik(ψ)` curve inherits the smoothness of the
chain rather than the noise of independent re-optimisation.

### 2.1 Mathematical setup

At each focal value `ψ_i` the profile likelihood is

$$
L_{\mathrm{prof}}(\psi_i) \;=\; \max_{\theta} L(y \mid \theta, \psi_i),
$$

where `θ` is the nuisance-parameter vector. Under mild regularity
(Murphy & van der Vaart 2000 thm. 1), the maximising `θ̂(ψ)` is a
smooth function of `ψ` in a neighbourhood of any non-boundary point.
That smoothness is exactly what warm-chain exploits: `θ̂(ψ_{i+1})` is
close to `θ̂(ψ_i)` for small `δψ = ψ_{i+1} − ψ_i`, so initialising
the per-cell inference at the previous cell's `θ̂` puts it within
the convergence basin from the start.

Under PMMH (or any MCMC per-cell) the same idea applies to the chain
rather than to the optimisation per se — the chain at `ψ_{i+1}`
starts in the right region of `(θ, ψ_{i+1})`-space and explores the
local ridge cheaply, rather than burning iterations on the initial
transient. We use "warm-chain" instead of "warm-start" deliberately
to acknowledge this is fundamentally about chain continuity, not
optimisation initialisation per se.

### 2.2 The seven design questions, settled

Each is non-obvious; the issue body raises them or implies them. This
RFC's job is pinning the defaults.

#### Q1: Traversal order

**Decision: bi-directional from a seed cell, forward + reverse,
take max-loglik per cell.**

Rationale: a single left-to-right pass is susceptible to hysteresis —
the chain that walks the grid in one direction gets trapped in a
particular basin and the resulting profile reflects the trap, not
the true `L_{\mathrm{prof}}(\psi)`. Bi-directional traversal runs the
grid forward and reverse from the seed cell and takes the best
loglik per cell, which catches the hysteresis case at the cost of
roughly doubling compute. The cost is acceptable because warm-chained
per-cell budgets can be ~halved (see Q5) without losing accuracy.

The seed cell is the centre of the bi-directional fan; cells are
processed `seed → max(ψ)` (forward) and `seed → min(ψ)` (reverse) in
parallel. Within each direction the cells are sequential; across
directions they're embarrassingly parallel.

#### Q2: Seed cell selection

**Decision: precedence chain `--seed-cell <ψ_value>` > nearest grid
cell to `--params`'s focal value > middle of grid.**

Rationale: the user may know which cell is best-resolved (e.g., from
a prior `camdl fit run` whose MAP sits at a specific ψ); they get
explicit override via `--seed-cell <ψ>`, snapped to the nearest grid
cell with a Warning if not exact. Without that flag, snap to whatever
`--params` declares the focal-param value as. Without `--params`'s
focal value, use the middle of the grid (heuristic — works on smooth
profiles, suboptimal on multimodal ones; the user gets a Warning
suggesting they supply `--seed-cell` if they hit the multimodal case).

The seed cell gets the **full per-cell budget** (no warm-chain
reduction) and ideally uses the LHS-init / multi-start path so its
MAP is well-optimised. Down-grid cells trust it; if the seed is bad,
everything downstream inherits the badness.

#### Q3: Per-start parallelism

**Decision: each of the K `--starts` chains independently warm-chains
across the grid. K parallel warm-chains are processed.**

Rationale: warm-chain preserves rather than collapses the K-start
parallelism. Each start has its own grid-walking trajectory; the K
trajectories may diverge (one chain gets trapped in a bad basin
going forward, another in a different bad basin going reverse), and
gh#74 Option B's per-cell `loglik_spread_starts` and
`loglik_rhat_starts` columns are what tells the user this happened.

If all K chains agree (small `loglik_spread_starts`), the per-cell
output is a clean profile-MAP. If they disagree, the user sees it in
the diagnostic columns and knows to investigate. Without B's
diagnostics, A's per-cell output would silently average over
disagreement; with B in place, A's smoothness improvement is
quantitatively visible.

#### Q4: Failure handling

**Decision: skip cells where the per-cell inference diverges, use
the last-successful cell's MAP as the warm start for the next cell;
emit a Warning naming every skipped cell.**

Rationale: a diverged chain (e.g., `loglik = −inf`, NaN, or
acceptance rate < 1%) is real information — it tells the user this
ψ value is genuinely outside the posterior support. Failing the
whole profile is overkill; restarting from the original `--params`
loses the warm-chain benefit and reintroduces independent-cell
noise. Skip + use-last-successful is the right shape:

- The diverged cell's row gets `loglik = NaN` in the output TSV
  (consumers can detect and exclude).
- The Warning lists every skipped cell with its ψ value.
- The next cell's warm start is the most-recent successful cell's
  MAP (which may be two or more cells back if multiple divergences
  clustered).

If more than ~30% of cells diverge, the Warning escalates to
"profile reliability questionable; consider re-running with
`--init lhs` independent-cell mode to diagnose the underlying
issue."

#### Q5: Per-cell budget tuning

**Decision: uniform budget in v1 (per-cell budget = `--pmmh-steps`
unchanged); document the "you can typically reduce `--pmmh-steps` by
~50% under warm-chain" guidance. Auto-reduction is a v2 follow-up.**

Rationale: warm-chained cells DO need less budget — the chain starts
in the right basin and doesn't need to burn iterations on the
transient. But the right reduction factor depends on `δψ` and the
local curvature, neither of which we can know automatically without
a calibration step that itself costs compute. Forcing a reduction in
v1 risks under-budgeting genuinely-hard cells; leaving it to the
user is honest and matches the rest of camdl's per-cell-budget
conventions.

V2 follow-up: auto-calibrate the reduction from a pilot run on the
seed cell. Out of scope here.

#### Q6: TSV ordering with bi-directional traversal

**Decision: output rows are always sorted by focal-param value
ascending, regardless of processing order.**

Rationale: the TSV is a programmatic data surface — a plot consumer
expects rows in `ψ`-order. Bi-directional traversal processes cells
out-of-order internally (seed first, then forward+reverse fans), but
the writer collects all rows before writing and sorts on `ψ`.
Trivial, but easy to get wrong if the writer is streaming.

#### Q7: IF2 vs PMMH symmetry

**Decision: PMMH passes the per-cell chain's MAP `θ` (max-loglik
sample) as the warm start; IF2 passes the swarm's best-loglik
particle's `θ`.**

Rationale: PMMH's per-cell output is a chain of `(θ, x)` samples;
the MAP `θ` is the natural point estimate to propagate. IF2's
per-cell output is a swarm of perturbed parameter vectors; the
swarm's best-loglik particle is the IF2 analogue of MAP. NLopt
already returns a point estimate so warm-chain there is trivial.

The CLI surface (`--init warm-chain`) is algorithm-agnostic; the
warm-start extraction logic dispatches per `--algorithm`.

### 2.3 The CLI surface

Extend the existing `--init {single | uniform | lhs}` set with a new
variant:

```
--init warm-chain
```

(Snake-case TOML form: `init_method = "warm_chain"` in fit/profile
configs.)

Two associated flags, both optional:

```
--seed-cell <ψ_value>     # explicit seed-cell override (Q2)
--warm-chain-direction {bidirectional | forward | reverse}
                          # default: bidirectional (Q1)
```

When `--init` is anything other than `warm-chain`, the new flags are
errors (clap mutual-exclusion). When `--init warm-chain` is set:

- `--seed-cell` defaults per Q2's precedence chain.
- `--warm-chain-direction` defaults to `bidirectional`. The `forward`
  and `reverse` variants run a single pass each (cheaper but
  susceptible to hysteresis); useful for testing and for the
  smoothness/cost-comparison validation in §4.

## 3. Composition with other in-flight work

### 3.1 gh#73 (profile priors honored)

Just landed on main. Warm-chain composes naturally — the per-cell
inference still resolves priors via the gh#73 precedence chain
(`--fit > model-IR > Flat-with-warning`). Warm-chain only changes
where the chain *starts*; it doesn't touch priors. The seed-timing
chapter's `t_rep = −40` and `n_seed = 1000` failure modes are fixed
by gh#73 (priors active) before warm-chain even applies. Warm-chain
then smooths the remaining `loglik(ψ)` jitter that gh#73 alone
doesn't fix.

### 3.2 gh#74 Option B (per-cell diagnostic columns)

Implementing in parallel; see the gh#74 issue. The columns
`loglik_spread_starts`, `loglik_rhat_starts`,
`starts_n_completed`, `acc_rate_min` are what make warm-chain's
benefit measurable: the §4 validation compares
`loglik_spread_starts` between `--init lhs` and `--init warm-chain`
on the same cell. Without B's columns, A's "is it smoother?"
question can only be answered visually.

### 3.3 gh#71 (stuck-chain diagnostics)

The R̂+ESS conjunction warning from gh#71 fires per stage, not per
profile cell. Conceptually similar to gh#74 B's
`loglik_rhat_starts` column, but lives on the inference-stage
diagnostic surface, not the per-cell TSV. They don't overlap; they
share the spirit of "make the failure mode visible."

### 3.4 gh#72 (unified diagnostic surface)

The Warnings emitted by warm-chain (skipped-cell warnings, seed-cell
fallback warnings, escalation warnings when divergence rate > 30%)
all want to flow through the unified surface when gh#72 lands. v1 of
this RFC ships through the existing `eprintln!("warning: …")`
pattern matching gh#73's convention; retrofit to typed
`DiagnosticKind` variants is a gh#72 follow-up.

## 4. Validation

The acceptance bar is concrete and quantitative: warm-chain must
produce smoother profiles than independent-cell init on smooth
surfaces, with comparable or smaller compute.

### 4.1 Smoothness regression on synthetic SIR

Build a small SIR model with a smooth, well-resolved 1-D profile
along some focal param (e.g., `R₀` with everything else fixed at
reasonable values; the marginal posterior on `R₀` from a noise-free
synthetic dataset is approximately quadratic at the MLE). Run
`camdl profile` over a 30-cell grid:

- **Arm A (independent)**: `--init lhs --pmmh-steps 1500 --starts 3`.
- **Arm B (warm-chain)**: `--init warm-chain --pmmh-steps 1500
  --starts 3 --warm-chain-direction bidirectional`.

Acceptance criteria:

1. **Adjacent-cell loglik differences**: median |Δlog L| between
   adjacent cells in Arm B is at most half that of Arm A.
2. **Per-cell `loglik_spread_starts`** (from gh#74 B): mean
   across-starts spread in Arm B is at most that of Arm A. (We
   expect smaller; "at most" is the no-regression bar.)
3. **No-skip on smooth surfaces**: Arm B's
   `starts_n_completed = K` on every cell. If cells get skipped on
   this regression test, something's wrong with warm-chain's
   convergence on a model that's known well-behaved.

### 4.2 Hysteresis catch on a known-bimodal surface

Build a model with a deliberate bimodal nuisance-parameter ridge at
one focal-param value (e.g., a SEIR with confounded β and σ;
flipping the basin produces visibly different MAP for the same
loglik). Run:

- **Arm A**: `--init warm-chain --warm-chain-direction forward`.
- **Arm B**: `--init warm-chain --warm-chain-direction reverse`.
- **Arm C**: `--init warm-chain --warm-chain-direction bidirectional`.

Acceptance: at the bimodal cell, Arm A and Arm B produce different
nuisance-parameter MAPs (catches the forward-vs-reverse hysteresis);
Arm C's loglik is at least max(A, B) (the bidirectional merge gets
the best of both). Without B's diagnostic columns this is hard to
assert programmatically; with B in place, asserting on
`loglik_spread_starts` per cell works.

### 4.3 Failure-handling regression

Force divergence by pushing the focal-param grid past the prior
support at one end. Acceptance: skipped cells get `loglik = NaN`,
the Warning fires naming each skipped cell, the un-skipped cells
still produce a valid profile.

### 4.4 Seed-timing chapter re-run

The motivating application. Re-run the seed-timing chapter's PMMH
profile under warm-chain. Acceptance: profile is visibly smooth
enough to read shape (adjacent-cell median |Δlog L| < 0.5 nats);
the chapter narrative updates to use warm-chain as the recommended
mode for posterior-sampling profiles. Document this in the chapter
itself rather than the RFC validation.

## 5. Caveats and non-goals

### 5.1 Warm-chain isn't a fit for all profiles

The smoothing benefit assumes `θ̂(ψ)` is a smooth function of `ψ`.
At posterior-support boundaries (e.g., the corner where `R₀ → 1` in
the survival-conditioning §3.4 regime), `θ̂(ψ)` can be
discontinuous and warm-chain's "small step from previous" assumption
breaks. The failure mode is benign (cells get skipped or land at
diverged values; B's diagnostics surface it), but users should know
that warm-chain at near-boundary ψ values gives noisier-not-smoother
results.

### 5.2 We are not auto-tuning per-cell budget

Q5 settled this: uniform budget in v1, with documented guidance to
reduce `--pmmh-steps` by ~50% under warm-chain. Auto-reduction is a
v2 follow-up requiring per-cell budget calibration from the seed
cell.

### 5.3 We are not removing the independent-cell default

Backwards-compatible by construction. Existing profile invocations
keep using `--init lhs` (or whatever they specify); warm-chain is
opt-in. We may revisit the default in a future round once warm-chain
has empirical track record across user workflows.

### 5.4 We are not implementing Option B here

gh#74 Option B (per-cell diagnostic columns) is being implemented in
parallel and is a hard dependency for A's validation (§4). This RFC
assumes B is in place by the time A's implementation tests are
written.

### 5.5 What we are explicitly not doing

- *Auto-ordering* the grid (e.g., walking outward from the predicted
  ridge minimum). Sticking to the bi-directional-from-seed-cell
  convention; "outward walks" are more complex and don't obviously
  beat bidirectional for typical surfaces.
- *Cross-cell parameter coupling beyond the warm start* — no
  Gaussian-process priors on `θ̂(ψ)`, no analytic Hessian-based
  extrapolation. Just the previous-cell MAP as init.
- *Ensemble warm-chain* — feeding the entire per-cell posterior
  (not just MAP) to the next cell as a prior. Conceptually cleaner
  but materially harder to implement and the gain is unclear at v1
  budgets.

### 5.6 Decisions for RFC review

The seven Q-numbered decisions above. Each has a default; reviewers
can push back on any. Specifically worth confirming:

- **Q1 default = bidirectional**: roughly doubles compute. Some
  workflows might prefer forward-only with explicit acceptance of
  hysteresis risk; the CLI flag covers this but the *default*
  matters.
- **Q4 escalation threshold = 30% divergence rate** before
  "questionable" Warning. Arbitrary; reviewers may want a different
  threshold.
- **Q5 uniform budget**: leaving this user-managed in v1 is the
  conservative call; aggressive auto-reduction would speed up
  smooth-surface fits noticeably.

## 6. Implementation cost estimate

- Per-cell warm-start extraction (PMMH MAP / IF2 best-particle /
  NLopt point estimate): ~50 LoC per algorithm, ~150 total.
- Bi-directional traversal driver in `profile.rs`: ~150 LoC
  (sequential within direction, parallel across directions, merge
  by max-loglik per cell).
- CLI flags (`--init warm-chain`, `--seed-cell`,
  `--warm-chain-direction`): ~30 LoC of arg-parsing + mutual-
  exclusion validation.
- Tests (§4 validation): ~200 LoC integration test + small
  synthetic-SIR fixture.
- Docs: CLI help + `docs/inference.md` warm-chain subsection.

Total: ~2 days focused work, gated on gh#74 Option B landing first.

---

## References

Primary sources for this RFC's framing:

- **Murphy, S. A. & van der Vaart, A. W.** (2000). On profile
  likelihood. *Journal of the American Statistical Association*
  95(450):449–465. Theorem 1 establishes the smoothness of `θ̂(ψ)`
  under regularity; the standard reference for the profile-likelihood
  construction warm-chain implements.
- **Pawitan, Y.** (2001). *In All Likelihood: Statistical Modelling
  and Inference Using Likelihood*. Oxford University Press. §3.4 on
  profile likelihood, including the practical advice that adjacent
  cells should be optimised from the previous cell's optimum.

Camdl-internal:

- gh#74 — the parent issue this RFC addresses; Option A specifically.
- gh#73 (closed) — priors-honored fix that lands before warm-chain
  composes with it.
- gh#71 (RFC) — stuck-chain diagnostics; shares the spirit but lives
  on the per-stage surface, not per-cell.
- gh#72 (RFC) — unified diagnostic surface; warm-chain's Warning
  emissions retrofit through this once it lands.
- `docs/dev/proposals/2026-05-07-survey-top-k-init.md` — the
  `init_method` machinery `--init warm-chain` extends.

---

## Notes on this document

- The seven Q-numbered design decisions in §2.2 are the load-bearing
  content; everything else is supporting context. Reviewers should
  push back on any of them.
- Compute-cost framing assumes warm-chain reduces per-cell budget by
  ~50%, so bidirectional compute roughly equals independent-cell
  compute. That's the v1 hypothesis; §4.1 validation tests it
  empirically and the result determines whether the §2.2 Q5
  "uniform budget" default holds up.
- Murphy & van der Vaart 2000 is the primary reference; the verbatim
  Theorem 1 statement (which underpins the smoothness assumption) is
  worth quoting in the doc when this RFC matures into landed code.
  Out of scope for the draft; flagged as a §4 follow-up before
  closure.
