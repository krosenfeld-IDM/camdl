# Alpha issue triage — Katherine, Dan, Ayrie

Date: 2026-06-04 Context: triaging recent GH issues from alpha testers to find
the major _modeling_ blockers and joinable overlaps, while juggling cleanup +
features. Authors: Katherine Rosenfeld, Daniel Klein, Ayrie (`avoorman`).

---

## VERIFIED root causes (subagent code-traces) — supersedes the preliminary triage below

The triage below grouped issues by _symptom_. Tracing the code corrected it:

### #175 — PGAS+NUTS divergence: ROOT-CAUSED, and it is NOT stratification

The trigger is the **`Prior::Hierarchical` leaf prior**, hard-stubbed to `-inf`
in the NUTS gradient (`pgas.rs:1222`:
`Prior::Hierarchical(_) => return (NEG_INFINITY, 0.0)`). The idiomatic
stratified prior `R0[patch] ~ normal(mu = R0_mu, sigma = R0_sd)` compiles each
leaf to a hierarchical prior → log-posterior `-inf` everywhere → 100% divergent,
0% acceptance, step→2e-6, chain frozen. **Reproduced:** same 5-patch model with
literal-constant priors (`Normal` leaf) mixes at 83%; flip to `Hierarchical` →
0%/100%. Reconciles the reporter's confusion: `[fixed]` hyperparameters don't
help (leaf stays `Hierarchical`). This is deferred "Gate 3b" —
**disabled-by-infinity**, so a frozen posterior warm-started at truth looks like
a tight well-mixed one (silent-wrong, public-health-dangerous). PMMH does it
right (env path, `pmmh.rs:276/364`).

**Gate 3b lift (the real fix Ayrie wants) — moderate, ~2–4 careful days.** The
infra exists: the density (`hierarchical::hierarchical_log_density`), the
`ParamEnv`/`NamedParams` machinery, and PMMH as a working template. New work:
(a) thread the env into `prior_log_density_and_grad_z` (mechanical — the closure
already has the params); (b) the analytic hierarchical gradient per supported
dist (normal etc.) — standard, but must FD-validate against the existing
`gradient_check.rs` pattern; (c) **the crux:** restructure the gradient
accumulation — a hierarchical leaf's prior gradient contributes to the grad
slots of the **hypers** (`R0_mu`, `R0_sd`) too, not just the leaf, so the
current per-param-independent loop (`pgas.rs:1740-1750`) needs a leaf→hyper
cross-term via a name→z-index map; (d) fix the MH fallback (`pgas.rs:1882`, also
`-inf`); (e) a `run_pgas`-on-hierarchical-stratified integration test asserting
acceptance ∈ [15%,90%] (none exists today). Immediate stopgap regardless: PGAS
hard-errors on hierarchical priors ("use `algorithm = pmmh`") using the
`matches!` guard `pmmh.rs:276` has.

### A (#160/#164/#165) — FOUR distinct defects, not one root

- **A — strata-sum unimplemented.** Un-indexed
  `incidence(<stratified
  transition>)` emits a bare
  `CumulativeFlow "infection"` (no sum) → dangling name post-expansion → E507
  (`expander.ml:3955-3957`). Spec §25.4 promises the strata-sum; the spec's _own
  worked example_ ships uncompilable. Prevalence got the symmetric fix
  (`CurrentPopSum`, `expander.ml:3932-3945`); incidence didn't. (#160 cited a
  _dead_ `ProjIncidence` branch — the parser only emits `ProjDerived` — the dead
  AST variant misled the reporter.)
- **A′ — let-bindings in `projected` not resolved.** Bare-identifier projection
  branch doesn't consult `let_tbl` (`expander.ml:3967-3974`), re-implementing
  identifier classification instead of using `resolve_ident_name`.
- **B — check/fit divergence.** `camdl check` skips `run_validate`
  (`inspect.ml:1086-1102`) which the full compile runs (`compiler.ml:325`), so
  E507 is invisible to `check`. A recurrence of gh#9 (which added _dimcheck_ to
  check but not _validate_). The unified `collect_diagnostics`
  (`compiler.ml:369-386`) already runs the real pipeline incl. validate;
  `run_check` should be a thin renderer over it. **This is the gh#170
  front-end-unification finish.**
- **C — `incidence`/`prevalence` not composable.** Valid only as the whole
  projection head; `incidence(x)+incidence(y)` → E100 _during expansion_, so
  `check` DOES catch it (NOT a check/fit divergence — the #164/#165 "fails only
  at fit" framing is wrong here). Bigger feature (make them expression
  operators) or a targeted diagnostic.

Map: #160 = A+B. #164 = A+A′+B. #165 = A+A′+B+C. Shared headline = A+B; A′/C are
extra.

### B-the-dt-cluster (#161/#173) — DIFFERENT roots, confirmed

- **#161** — parser has no `dt` in `simulate {}` (no `DT` token, `simulate_decl`
  has no field). **doc-vs-code:** `dates.md` shows `dt` in a `simulate {}` block
  in 3 places (lines 302, 327-328, 338).
- **#173** — `fit.toml`'s `FitConfigV2` lacks `#[serde(deny_unknown_fields)]` →
  a top-level `dt` is silently dropped (honored only as `[config].dt`). And
  `grep deny_unknown_fields rust/crates` → **zero** — _every_ fit.toml key is
  typo-vulnerable (misspell `particles` → silent default). A bespoke
  `detect_legacy_init_keys` hack catches 2 renamed keys; never generalized.
- **DESIGN DECISION (Vince):** `dt` should be a **model knob** (add to the
  `simulate {}` grammar) and **preferred in docs**, because models _are_
  sensitive to `dt` (discretization error; Richardson-extrapolation diagnostics
  deliberately vary it). So #161's fix is _add `dt` to the grammar_ (model
  default) + keep `--dt` as the override; #173's fix is `deny_unknown_fields`
  (never silently drop).

## How these shipped, and the systems (beyond TDD) that would catch them

Every one of these is a **silent-wrong** bug, and they cluster on **two failure
modes the project already has named cures for, applied unevenly**:

1. **Parallel reimplementation that drifts.** check vs compile (Defect B —
   drifted _twice_); incidence vs prevalence expansion (Defect A — only
   prevalence got the fix). _System:_ single source of truth — route `run_check`
   through `collect_diagnostics` (it already exists); share one strata-expansion
   helper. Architectural, not a test.
2. **Unsupported configs disabled-by-degradation, not refused.** #175's `-inf`
   stub; the silent config drops. _System:_ the project's **own patterns**,
   applied where missing — the `Capabilities` bitflags (backends declare
   support, dispatch hard-errors) extended to inference algorithms (PGAS
   declares "no hierarchical priors"); `deny_unknown_fields` everywhere (strict
   config); "make illegal states unrepresentable."
3. **No spec-as-tests.** §25.4's example and the `dt`-in-`simulate` doc both
   shipped uncompilable/false. _System:_ a literate-doctest tool that compiles
   every spec/book code block in CI (the GH issue below).
4. **Tests assert liveness, not quality.** #175 would've been caught by an
   inference test asserting acceptance > 0, not "runs without panic." The PF has
   differential oracles already (`CAMDL_EVAL_UNRESOLVED`); inference needs
   quality gates (acceptance, divergence rate, recovers-truth).
5. **Parity/differential meta-tests + symmetry audits.** A "compile every
   fixture via check AND compile, assert equal error-sets" test kills the whole
   Defect-B class. When two things are siblings (incidence/prevalence,
   check/compile, sim/fit), a divergence in handling is the smell.

**The through-line to beta:** these aren't new principles — they're the
project's _stated_ values (no-loose-semantics, make-illegal-states-
unrepresentable, Capabilities, differential oracles, single diagnostics surface)
applied unevenly. Beta-hardening = enforce your own invariants _uniformly_,
encoded as meta-tests so they can't drift again. Proper fixes, not safety guards
— the guards are only the stopgap that buys time.

---

## The one-line read

**The spatial/stratified modeling path is where every tester is blocked.**
Inference for stratified (multi-patch / age) models is broken in several
independent ways at once — that's the theme, and prioritizing it unblocks the
most people.

## Major modeling blockers (priority order)

1. **#175 (Ayrie) — PGAS+NUTS: 100% divergent / 0% acceptance for ANY stratified
   model.** The θ-chain freezes at its init; the identical single-patch model
   mixes fine (~83% accept). Only difference is `stratify(by=patch)`. An earlier
   `-inf` reference-density bug was fixed in `856ecec`; this NUTS divergence is
   the remaining blocker. **This is the top blocker — inference is dead for
   stratified models.** High-risk (inference math:
   `pgas.rs`/`nuts.rs`/`pgas_grad.rs`); treat carefully.

2. **Stratified observation incidence — JOIN #160 + #164 + #165.** One
   underlying gap with three faces:
   - **#160 (Katherine, root cause):** un-indexed
     `incidence(<stratified
     transition>)` should expand to a _sum over
     strata_ per spec §25.4, but the expander emits a bare
     `CumulativeFlow "infection"` referencing a name that doesn't exist
     post-expansion → E507 at simulate/fit.
   - **#164 (Dan, symptom):** `camdl check` ACCEPTS obs expressions that
     `camdl fit` (camdlc) then rejects with E507 → check gives false confidence.
     (check/compile divergence.)
   - **#165 (Dan, use-case):** no strata-summed incidence, no summed
     `incidence()` terms, no let-bindings in `projected`; a state-total
     observable (reporting × Σ-strata incidence) is unwritable; fails only at
     fit-compile. → **One workstream:** make un-indexed `incidence()` over a
     stratified family strata-sum (per §25.4), and make `check` validate obs
     expressions exactly as fit does (kill the E507 divergence). This unblocks
     fitting stratified models to aggregate (state-total) data — a core epi
     need. Mostly compiler/expander + dimcheck work (lower-risk than #175).

3. **#174 (Katherine) — positive incidence obs at model time 0 → `-Inf`
   likelihood.** The t=0 incidence convention (incidence at the origin is 0)
   makes a positive first observation kill the fit. Workaround exists
   (drop/shift the first row), so it's a friction/UX blocker, not a hard wall —
   but it silently produces `-Inf` rather than a hint. Fix: detect + hint, or
   define the convention. Medium-high.

## Joinable overlaps

- **JOIN A — stratified obs incidence: #160 + #164 + #165** (above). The
  near-term unblock is the §25.4 strata-sum expansion + the check/fit parity.
  #165 is _also_ a facet of the bigger obs-model redesign (below).
- **JOIN B — dt placement footguns: #161 + #173.** `dt` is silently
  wrong/rejected in the two natural spots:
  - **#161 (Katherine):** `simulate { dt = ... }` is shown in `dates.md` but the
    parser rejects it (E001) — `dt` is CLI-only. Doc-vs-code.
  - **#173 (Dan):** a top-level `dt` in `fit.toml` is **silently ignored** (must
    live in `[config]`) — dt=1/2/5 gave byte-identical results, a silent-wrong
    loose-semantics bug that wasted a timing experiment. → One "dt ergonomics"
    fix: accept (or hard-error with a hint) `dt` in the natural places; never
    silently ignore. Aligns with the project's no-loose-semantics rule.
- **JOIN C — observation-model redesign umbrella: #172 ⊇ #165, #171, #169.** Dan
  explicitly frames **#172 (timeseries-first inference; want first-class
  summary-statistic / trajectory-reduction calibration targets)** as the
  umbrella over:
  - **#171:** observation streams can't express sparse/time-varying surveillance
    geometry (sentinel/ES sites in a _subset_ of strata, switching on at
    activation dates).
  - **#165:** stratified state-total observables (also in JOIN A).
  - **#169:** the CAS-filename bug (below) surfaced in the same workflow. → This
    is a DESIGN lift, overlapping the unified-observation-data work (gh#134,
    "obs = table over time×dims"). Connects to the malaria summary-fitting notes
    (`~/Downloads/camdl-malaria.md`): summary-statistic / probe-matching is a
    _different inference mode_ (ABC / synthetic likelihood) than the
    particle-filter likelihood. Bigger than a bugfix; schedule after the
    immediate blockers.

## Prior-predictive workflow cluster (Dan, while setting up `--draws prior`)

- **#169 — BUG:** `simulate --draws prior` + `observations {}` block → CAS
  auto-commit fails `File name too long (ENAMETOOLONG)` per draw (large derived
  per-draw seeds); `--output` succeeds but exit is nonzero. Real bug, blocks the
  workflow.
- **#156 — PARTIALLY ADDRESSED by b48c7bb.** Wants (1) output cadence/stride
  control and (2) flow-column suppression. The new
  `output { trajectories {
  every = … | at = [...] } }` (b48c7bb) gives cadence
  control (1); flow-column suppression (2) is still open.
- **#157** — emit a `draws.tsv` of the sampled θ per draw (predictive-check
  diagnostics + provenance).
- **#158** — docs: the `simulate --fit` config schema is undocumented + errors
  cryptically (`ModelRef`).
- **#155** — add `log_uniform` + `truncated_normal` priors (orders-of- magnitude
  scale params; bounded normals).

## Other (perf / CLI / cleanup)

- **#162 (Katherine) — Rayon uses all cores under PMMH.** Related to the
  `fit --parallel` work (856ecec) which caps _fit chains_, but the
  particle-filter–internal Rayon pool is still uncapped → respect
  `RAYON_NUM_THREADS` / a `--threads` cap globally. Likely partially-fixed,
  needs confirmation.
- **#166 (Dan)** — ODE backend is fixed-step RK4; add adaptive RK45 for
  long-horizon deterministic MLE (1107s/converged-MLE on a 12-yr cVDPV2 fit).
  Perf feature.
- **#131 (Katherine)** — `--progress` shows nothing for `camdl simulate`
  (Linux). CLI bug.
- **#127 (older, upstream-audit)** — runtime _panics_ on out-of-range table
  lookup. Relevant to the just-landed oob cleanup (`112d725`): `Error` is now
  the only policy and still `panic!`s; #127's point is that a
  user/particle-triggerable OOB should be a clean error, not a panic.

## Recommended sequencing (modeling-unblock first)

1. **#175** — stratified NUTS divergence (top blocker; careful, inference math).
2. **JOIN A (#160/#164/#165)** — stratified obs incidence §25.4 strata-sum +
   check/fit parity (compiler; high payoff, lower risk).
3. **#174** — t=0 `-Inf` hint.
4. **JOIN B (#161/#173)** — dt-placement no-silent-ignore (quick, aligns with
   no-loose-semantics).
5. **#169** — CAS ENAMETOOLONG bug (quick, unblocks prior-predictive).
6. Then the feature/design tier: #172 obs-model redesign (with #171/#165/
   gh#134), #156 flow-suppression, #157/#158/#155, #166 RK45, #162 Rayon cap,
   #131 progress.
