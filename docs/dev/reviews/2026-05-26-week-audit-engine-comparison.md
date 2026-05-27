---
status: open
date: 2026-05-26
kind: meta — comparison of two reviews (Rust engine layer)
inputs:
  - 2026-05-26-week-audit-findings.md  (internal, six-cluster sub-agent audit)
  - 2026-05-26-upstream-rust-engine-review.md  (external, Rust engine vs spec)
related:
  - 2026-05-26-week-audit-comparison.md  (OCaml-compiler companion comparison)
---

# Comparison — internal week-audit vs upstream Rust-engine review

Second upstream review landed today, this time the Rust runtime
engine layer against `docs/camdl-language-spec.md`. Same pattern as
the OCaml-compiler comparison: different scope, different
methodology, mostly non-overlapping findings, and the upstream
catches systemic correctness defects the diff-scoped internal
review did not look for.

## TL;DR

The reviews are again **complementary**, and the upstream's Rust-
engine pass is the **most severe of the three reviews completed so
far** — it identifies two construction-time defects (frozen
parametric forcing/tables; chain-binomial reading zero for real
compartments) that *silently invalidate inference* for entire
classes of model that camdl is being used to fit.

- **Internal review.** Last-week diff. Caught Gillespie bare-`t`
  residual (C3) and event negative-count guards (H1) on the
  engine side; rest of the engine surface unaudited.
- **Upstream review.** Spec-vs-engine static audit. 6 Critical / 9
  High / 2 Medium + 1 structural recommendation.

**Overlap is 2 of the upstream's 15 Critical+High findings:**

- Upstream Critical #3 (Gillespie nonhomogeneous Poisson) overlaps
  internal C3 (Gillespie bare-`t` residual). Upstream is **broader**
  — covers `TimeFunc` dependency and real-compartment dependency,
  not just bare-`t`. Internal C3 was scoped to commit `424b6a9a`.
- Upstream Critical #6 (events/interventions silent no-ops, fraction
  clamp, negative transfers) overlaps internal H1 (event-action
  negative-count guards). Upstream is **broader** — adds fraction
  clamping `> 1.0`, mixed-kind transfers, real-target events.

The other 13 upstream findings are entirely novel relative to the
internal review.

## Spot-verification of upstream claims

Five of the most devastating-if-true claims verified against the
code:

| # | Claim | Verified? | Receipt |
|---|---|---|---|
| 1 | Inline table values and time-function fields are evaluated once at construction with `default_params` | ✓ | `compiled_model.rs:569-585` (table cache) and `:591-+` (time-func cache) both call `eval_table_expr(expr, &param_index, &default_params)`. Propensity reads at `propensity.rs:186, 191` read frozen `time_func_cache` and `table_values_cache` with no param dependency. |
| 2 | Chain-binomial `step_one` ignores real state | ✓ | `chain_binomial.rs:261` signature: `pub fn step_one(model, counts: &mut [i64], flows, params, t, dt, rng, scratch, fire_steps)`. No `real_values` argument. `scratch.real_s` initialized in `StepScratch::new` (line 67) as `RealState::new(n_real)` — zeros — and never populated. `eval_propensities` at line 282 evaluates against `&scratch.real_s` = zeros. |
| 4 | `source_groups` picks first negative-stoich entry as the source | ✓ | `compiled_model.rs:539` — `stoich.iter().find(\|&&(_, d)\| d < 0)`. `A + B → C` groups by `A` only; `B`'s count is never used as a bound. |
| 5 | Deterministic source transitions silently skipped | ✓ | `chain_binomial.rs:331` — `if let ResolvedDraw::Deterministic = ... { handled[tr_idx] = true; continue; }` inside the source-group loop. Line 402 — `if scratch.handled[i] { continue; }` in the ungrouped loop. So a `S → I @ deterministic(...)` transition is marked handled in the first loop and skipped in the second. |
| 11 | Tau-leap passes `cfg.dt` instead of truncated substep `dt` | ✓ | `tau_leap.rs:110` computes `let dt = cfg.dt.min(next_boundary - t)` (truncated). Lines 137 + 143 then pass `cfg.dt` (not `dt`) into `eval_propensities` and `EvalCtx`. On boundary steps any rate expression referencing `Expr::Dt` sees the configured nominal step, not the actual one. |

All five hold up. The upstream review is rigorous.

## Severity calibration — why this review is the most serious of the three

Two of the upstream's Criticals silently invalidate **inference**, not just simulation:

### Upstream #1 — frozen parametric tables and forcing

Failure mode:

- User declares a sinusoidal forcing with parameterized `amplitude`,
  `phase`, `baseline`.
- User fits via IF2/PMMH/PGAS over those parameters.
- Each proposal evaluates the trajectory against the
  **construction-time defaults**, not the proposed values.
- The likelihood is artificially flat along the forcing parameter
  dimension. The posterior is the prior (plus whatever signal leaks
  through non-forcing parameters compensating).
- **The user has no signal.** Diagnostics look fine: chains mix,
  ESS is high, R̂ is good, the inference reports a "credible
  interval" for `amplitude`. The credible interval is meaningless.

This is the **worst class of bug** for a public-health inference
tool: produces a confidently-reported posterior for a question the
software is not actually answering. Every published or in-flight
analysis with parameterized forcing or parameterized table entries
is suspect until verified.

### Upstream #2 — chain-binomial reading zero real state

Failure mode:

- User writes a cholera / environmental-reservoir model:
  `infection : S → I @ beta_W * W / (K + W)`
- `W` is a real compartment with an ODE.
- Chain-binomial backend evolves `W` via RK4 — output shows `W`
  growing.
- But transition rates evaluate against zero `W`; **no infections
  ever occur**.
- Or rates that *use* `W` produce silently wrong numbers because
  `scratch.real_s` is zero.

Any environmental-reservoir, wastewater, vector-abundance, or
real-compartment-coupled model under chain-binomial inference is
silently wrong.

### Comparison to internal Criticals

The internal C1 (`survey_top_k` by likelihood) and C2 (profile-PMMH
retrospective MLE) are also "silent posterior bias" failures. Side
by side:

| Defect | Affects | Visible to user? |
|---|---|---|
| Internal C1 (survey_top_k) | Any chain initialized from survey_top_k | No — chains mix, but start in the wrong region |
| Internal C2 (profile-PMMH retrospective) | All profile-PMMH runs before 2026-05-24 | No — output looked like a posterior, was actually MLE |
| Upstream #1 (frozen forcing/tables) | Any inference of parameterized forcing/table entry | **No — credible interval reports flat-prior posterior; everything else "works"** |
| Upstream #2 (chain-binomial real state) | Any environmental-reservoir or real-coupled model under CB | **No — output shows real-state evolving, transitions silently use zero** |

Both classes belong to the same failure family. Upstream #1 and #2
are at least as serious as the internal Criticals on the same
axis, and they affect a *wider model class* (any user with
parameterized forcing/tables; any user with environmental reservoirs).

## Where the upstream beats the internal review

Eleven engine-side Critical+High findings the internal review did
not catch, all systemic:

- **#1 frozen forcing/table caches** — devastating, silent
- **#2 chain-binomial reads zero real state** — devastating, silent
- **#4 multi-source bounded by first source only** — vector-host,
  pair formation, chemistry-style models
- **#5 deterministic source transitions silently skipped** — silent
  zero-effect demographics/aging/waning
- **#7 Rust IR validator misses most refs** — the runtime is not a
  trust boundary
- **#8 initial conditions accept negative/fractional/NaN** — silent
  wrong-start-state
- **#9 chain-binomial output stamps future state at off-grid times**
  — biased incidence timing
- **#10 schedule/time-step validation debug-only** — release builds
  can hang on bad config
- **#11 tau-leap/ODE pass `cfg.dt` on truncated substeps** — `dt`-
  referencing expressions wrong at every boundary
- **#12 table lookup panic paths** — particle-triggered process kill
- **#14 Gillespie sparse updates clamp negative propensities to
  zero** — silent transition-off
- **#15 unknown `rate_grad` keys silently dropped** — biased
  gradients in NUTS

The pattern is again **diff-scope vs spec-scope**. My six sub-
agents looked at recent commits; none asked "audit the runtime
engine against the spec end-to-end." The upstream's single coherent
pass against the spec catches the systemic drift.

## Where the internal review beats the upstream

The upstream stated its scope as engine layer only and did not do
a full inference audit. Internal coverage the upstream does not
touch:

- **Inference-layer correctness.** `survey_top_k` (C1),
  profile-PMMH retrospective (C2), profile-PMMH params/loglik
  incoherence (C5), prior-precedence wiring (entire E cluster).
- **Recent-commit scrutiny.** gh#69 missing tests (C4),
  events_backend_parity release-binary skip (H13), CAMDL_TRACE
  coverage (H14).
- **Cross-language drift on calendar time** (C6/C7/M13/M14 / #98).
- **from_csv batch source hardening** (H3–H6 / #100).
- **Lineage subsystem** (H11+H12 / #101).
- **Profile diagnostics surfacing** (H7+H8+H17 / #103).
- **Always_active flag + param_kind enum** (H15+H16 / #107).

These are real correctness issues the upstream's scope excludes.

## Cross-review correlations and bundling opportunities

Several findings across the three reviews chain into single
remediation pieces:

- **Upstream Critical #3 (engine) + internal C3** — both Gillespie.
  Single remediation: implement thinning / modified-next-reaction,
  add real-compartment dependency tracking, refuse coarse-grid
  inhomogeneous models. One issue, one fix.
- **Upstream Critical #6 (engine) + internal H1 + internal H2** —
  all event-action semantics. Single remediation: validate every
  action target/domain at IR load, reject negative transfers and
  out-of-range fractions, align backend evaluation point (start-
  of-step canonical). One commit closes three findings.
- **Upstream Critical #4 (engine) + upstream OCaml Critical #2** —
  both about multi-source / table arity not enforced. The
  structural validators these need are siblings.
- **Upstream High #7 (engine validator) + upstream OCaml Critical
  #4 (stratified init both sides)** — the upstream OCaml review
  already noted that *neither* validator checks init names. The
  fix lands in both layers.
- **Upstream High #13 (engine, observation real state) + upstream
  Critical #2 (engine, chain-binomial real state)** — same root
  cause (real state not threaded through ParticleState); single
  refactor solves both.

## Methodology comparison (engine surface only)

| Dimension | Internal | Upstream engine |
|---|---|---|
| Scope | Last-week engine commits only (gh#67, gh#69, gh#73, gh#74, gh#75, …) | Full Rust engine surface vs language spec |
| Methodology | Sub-agents per cluster | Single coherent pass against spec |
| Test execution | Did not run; some sub-agents quoted test files | Stated upfront: cargo unavailable |
| Receipt rigor | grep/read output inline per CLAUDE.md | File:line citations; spot-check confirmed 5/5 |
| Architectural recommendations | Atomic-landing bundles by surface | One structural lift (`CompiledModel` → `structure + ResolvedModel` + `EvaluationContext`) that subsumes #1, #2, #13 |
| Cross-layer thinking | Some (the typed-time OCaml↔Rust bundle) | Extensive — explicitly bridges to the upstream OCaml review |

The upstream's structural recommendation is again the right read.
Splitting `CompiledModel` into immutable structure plus parameter-
keyed evaluation removes the entire class of "frozen at
construction" bugs, not just the two flagged. Combined with the
OCaml-side `resolve_indexed_ref` lift, the two structural fixes
cover ~12 findings between them.

## Are there more serious issues found?

Yes — **upstream #1 and #2 are the most severe single findings
across all three reviews so far**, by the criterion of "silently
returns wrong scientific answer for a wide class of model under
ordinary use." Comparable in damage to internal C2 (profile-PMMH
retrospective) but with a wider model-class footprint.

The internal review's Criticals are still real and load-bearing,
but the upstream engine review materially raises the severity
ceiling.

## Recommendation

**Both reviews stand.** File the upstream engine Criticals + Highs
as a parallel GH-issue cohort with the `upstream-audit` and a new
`engine` label.

Suggested *integrated* remediation order — merging all three
review sets:

1. **Upstream engine #1** (frozen parametric forcing/tables).
   Highest-damage single bug; affects ongoing inference. Audit
   every fit since the introduction of parametric forcing; flag
   anyone whose inference results may be invalidated.
2. **Upstream engine #2** (chain-binomial real state).
3. **Internal C2** (profile-PMMH retrospective incident doc).
4. **Upstream OCaml #3** (block transition rate=0.0 grammar fix).
5. **Upstream OCaml #5** (scenario validation).
6. **Upstream engine #4 + #5** (multi-source bounds + deterministic
   source skip). One commit.
7. **Upstream engine #6 + internal H1 + internal H2** (event-action
   semantics bundle).
8. **Upstream engine #3 + internal C3** (Gillespie correct nonhomo).
9. **Upstream OCaml #1+#2+#9+#10+#12 via resolve_indexed_ref**
   (proposal `docs/dev/proposals/2026-05-26-typed-indexed-reference-resolver.md`).
10. **Upstream engine #7 + upstream OCaml #4** (init validation
    both sides + complete IR validator).
11. **Internal C6+C7+M13+M14 + upstream OCaml #7** (typed-time
    OCaml↔Rust unification).
12. **Internal C1** (`survey_top_k` posterior ranking).
13. **Upstream OCaml #6** (likelihood dim-check).
14. **Internal C5** (profile-PMMH params/loglik).
15. **Upstream engine #8, #9, #10, #11, #12, #14, #15** (various
    runtime hardening; each modest).
16. **Upstream OCaml #8** (namespace uniqueness — prerequisite for
    the resolver in step 9, sequence ahead if not already in flight).
17. **Internal C3** subsumed by step 8.
18. **Internal C4** (gh#69 tests — pure test commit).

After 1–18, the remaining Mediums and Lows from all three reviews
can land opportunistically.

## What this teaches about review methodology (revisited)

The OCaml-compiler comparison earlier today concluded that diff-
scoped audits cannot see systemic drift. The Rust-engine
comparison strengthens that: **the most damaging defects in
camdl right now are systemic correctness gaps in pre-existing
code, not in the last week's commits.** The diff-scoped audit was
valuable — it found the gh#69 test gap, the profile-PMMH
retrospective, the prior-precedence holes, the from_csv issues —
but it missed the load-bearing engine bugs because they predate
the diff and live in surfaces no recent commit touched.

For the next audit cycle, run three methodologies in parallel:

1. **Diff-scoped** (catches new defects) — six sub-agents per
   recent cluster, as before.
2. **Spec-vs-OCaml-compiler** — one sub-agent against
   `camdl-language-spec.md`.
3. **Spec-vs-Rust-engine** — one sub-agent against the spec for
   runtime semantics.

(2) and (3) are the surfaces where systemic drift accumulates the
fastest because they are the "implementation of the contract"
layers. (1) catches recent regressions only.
