---
status: open
date: 2026-05-26
kind: meta — comparison of two reviews
inputs:
  - 2026-05-26-week-audit-findings.md  (internal, six-cluster sub-agent audit)
  - 2026-05-26-upstream-ocaml-compiler-review.md  (external, OCaml compiler vs spec)
---

# Comparison — internal week-audit vs upstream OCaml-compiler review

Two reviews landed today. Different scopes, different methodologies,
different findings. This doc evaluates them objectively against
`docs/dev/code-review.md`'s rubric and against verification of the
code itself.

## TL;DR

The reviews are **complementary, not redundant**. Together they
cover correctness terrain that neither covers alone.

- **Internal review.** Last-week diff (98 commits across six
  clusters: lineage, typed-time, events/sim, profile inference,
  prior precedence, batch from_csv). Heavy on the Rust runtime
  and recent-surface scrutiny. Counts: 7C / 19H / 22M / 9L.
- **Upstream review.** Full-codebase OCaml compiler audit against
  `docs/camdl-language-spec.md`. Static-only (no test runs). Rust
  inference explicitly out of scope. Counts: 8C / 6H / 2M + 1
  structural cross-cutting fix.

**Overlap is one finding.** The upstream's Critical #7 ("invalid
calendar dates compile as shifted real dates") overlaps the
internal C6 + M13 bundle ("OCaml vs Rust `parse_iso_date` accept
different sets of strings"). Both flag the missing validation in
`parse_iso_date`; the internal review additionally flagged the
dead `origin_rata_die` field (C7) and the missing cross-language
golden table (part of the C6 fix).

Everything else in each review is novel relative to the other.

## Spot-verification of upstream claims

Before judging severity, I verified four high-impact upstream
Criticals against the actual code:

| # | Claim | Verified? | Receipt |
|---|---|---|---|
| 1 | `index_item_to_str` discards named-index labels | ✓ | `expander.ml:926-931` — `INamed (_, EIdent (s, _))` matches the *underscore* in the label position; the named-index identifier is silently dropped |
| 2 | Table lookup uses `List.nth tdims i` without arity check | ✓ | `expander.ml:1505-1525` — `List.mapi` iterates over user-supplied `items`, not over `tdims`; under-indexing produces a partial-prefix linear index |
| 3 | Block-form transition rate defaults to `EConst 0.0` | ✓ | `parser.mly:404-405` — literally `let rate = ref (EConst 0.0)`; no `rate = …` entry → zero rate |
| 6 | `dimcheck` sets `permissive_dim <- true` around the entire likelihood pass | ✓ | `dimcheck.ml:718-737` — blanket `st.permissive_dim <- true` over the whole `List.iter (fun obs -> ...) m.observations` |
| 8 | Lookup tables use `Hashtbl.replace` (silent overwrite) | ✓ | `expander.ml:715-750` — every namespace uses `Hashtbl.replace`; no uniqueness check before |

All four hold up. The upstream review is rigorous.

## Where the upstream beats the internal review

The upstream caught a class of defects the internal review did
not even look for. Eight Critical findings — most of them
devastating for ordinary users writing real models, hit on day
one of usage, not edge cases:

- **#1 Indexed references via string concat.** `S[patch = p]`
  silently produces wrong output. Core public-health idiom.
- **#2 Table lookup no arity check.** `C_age[child]` against
  `age × age` returns wrong cell. Contact matrices drive force
  of infection.
- **#3 Block transition rate=0.0 default.** Missing `rate =` line
  yields a transition that never fires. Silent zero-transmission.
- **#4 Stratified init not validated.** `init { S = N0 }` for
  stratified `S` starts the model in an empty population.
- **#5 Scenario `enable`/`set`/`scale` typos silent.** Vaccination
  campaign scenario runs as baseline; wrong counterfactual.
- **#6 Likelihood dim-check blanket-permissive.** `binomial(p = I)`
  compiles with `I` a count, not a probability.
- **#8 Duplicate names silent.** `parameter N` plus `let N = S+I+R`
  resolves to the let; equations change while source looks right.

These are the kind of bugs that produce a wrong vaccination plan
from a syntactically valid model. Internal review missed all eight.

**Why the gap.** Internal review's six sub-agents were scoped to
recent commits (typed-time, events/sim, etc.) or to specific
subsystems (lineage, profile inference, prior precedence). None
of them performed a "compiler against spec, end-to-end" pass.
The typed-time sub-agent only caught the `parse_iso_date` drift
because it was in scope for that work; the rest of the OCaml
compiler surface was unaudited.

The upstream's single-pass methodology — read the spec, read the
compiler, find where they disagree — is materially better at
finding systemic spec/implementation drift than a diff-scoped
review.

## Where the internal review beats the upstream

The upstream stated its scope as OCaml compiler only; Rust
inference explicitly out of scope. The internal review covered
ground the upstream did not look at:

- **Inference correctness.** `survey_top_k` ranks by likelihood
  not posterior (C1). Profile-PMMH was silently MLE pre-`5f658a16`
  (C2 retrospective). Profile-PMMH `mle.toml` params/loglik
  incoherence (C5).
- **Numerical correctness in the Rust runtime.** Gillespie
  inhomogeneous-Poisson still biased after the bare-`t` fix (C3).
- **Recent-commit scrutiny.** gh#69 parametric `at [param]` has no
  regression tests (C4 — would only show up if you knew about
  this week's commits).
- **Cross-language contracts beyond `parse_iso_date`.** Dead
  `origin_rata_die` field, missing `ir/golden/caltime.tsv`
  fixture (inside the C6+C7+M13+M14 bundle).
- **The new from_csv batch source.** Path anchor inconsistency,
  inf/nan acceptance, BOM, malformed delimiter (H3–H6).
- **The new lineage subsystem.** Newtype hygiene gaps,
  monotonicity guards (H11+H12).

These are real correctness items that the upstream's scope
excludes by construction.

## Severity calibration

Both reviews call ~7-8 Criticals. Are they comparable in
severity?

Honestly: the upstream's Criticals are **more devastating per
occurrence** than several of the internal Criticals. A user
omitting `rate =` in a block-form transition (upstream #3) gets
zero transmission; a typo in `enable = sia_typo` (upstream #5)
runs the baseline. Either silently produces a wrong scientific
output for an ordinary user on first use.

The internal Criticals are higher-leverage in different ways:
`survey_top_k` (C1) and profile-PMMH retrospective (C2) affect
*every* user who used those surfaces; Gillespie inhomogeneous-
Poisson (C3) affects every user of seasonal/forced models on
that backend. They're less likely to trip on first use, but they
quietly bias whole classes of model the moment they trip.

The honest characterization: upstream Criticals are *more visible
first-time correctness*, internal Criticals are *less visible but
quieter cumulative bias*. Both classes matter for cVDPV2 work.

## Methodology and detail

| Dimension | Internal | Upstream |
|---|---|---|
| Scope | Last-week diff, six clusters | OCaml compiler full surface |
| Methodology | Six parallel sub-agents | Single coherent pass |
| Code receipts | File:line + grep output inline per CLAUDE.md | File:line citations |
| Fix specificity | Surgical (specific line edits) | Architectural (proposed type shapes) |
| Test execution | Did not run tests | Stated upfront: could not run dune |
| Cross-language analysis | Yes (C6/C7 + lineage IR + ir version) | Yes (#4 init validators, #7 calendar) |
| Structural recommendations | Atomic-landing bundles (C6+C7+M13+M14, H15+H16) | One cross-cutting `resolve_indexed_ref` proposal that subsumes 7 findings |

The upstream's structural fix at the end (`resolve_indexed_ref`
+ `resolved_ref` ADT) is the kind of architectural lift the
internal review didn't propose — and it is the right call. The
upstream notes that one change subsumes findings #1, #2, #4, #9,
#10, #12, and parts of #13. That's a sophisticated reading of
the failure surface.

The internal review's atomic-landing bundles (e.g. C6+C7+M13+M14
into one commit) are a sibling discipline — sequencing fixes so
the IR version bumps only once and the OCaml/Rust changes land
together. Different problem (release engineering), different
answer.

Code-receipt rigor: the internal review's CLAUDE.md-mandated
"paste-the-receipt" rule produced more inline grep output; the
upstream's claims are precise enough that spot-checking each
took one command. Both pass the discipline test.

## More serious issues found?

Yes — upstream found more numerically devastating
silent-numeric-bug class items in the compiler than the internal
review found in the same surface. Eight Critical OCaml-compiler
findings vs the internal review's two (C6, C7).

But the upstream is silent on:

- The recently-landed gh#69 / gh#67 / gh#73 / gh#75 / gh#74
  surfaces (out of scope)
- All Rust inference (out of scope)
- The from_csv batch source (out of scope)
- The lineage subsystem (out of scope)

So "more serious" depends on which surface you weigh. If the
relative cost of a public-health modeler writing a wrong-by-
silent-compilation model versus a wrong-by-silent-inference
posterior is comparable, the reviews are comparable in total
severity. If you weight "silent compilation produces wrong model"
as worse than "silent inference produces wrong posterior" — a
defensible call for an alpha tool whose first-time users are
exactly the failure mode — the upstream is the more serious set.

## Recommendation

**Both reviews stand.** File the upstream findings as a parallel
GH-issue cohort and prioritize them alongside the internal
Criticals.

Suggested order for the next remediation sprint, merging both:

1. **Upstream #3** (block transition rate=0.0): one-line grammar
   fix, blocks no one, prevents the cleanest silent-zero-
   transmission bug.
2. **Upstream #5** (scenario validation): the wrong-counterfactual
   class. Closed-grammar fix.
3. **Internal C2** (profile-PMMH retrospective incident): no code
   change, just documentation. File now.
4. **Upstream #1 + #2 + #9 + #10 + #12 via the `resolve_indexed_ref`
   structural fix**: large lift but subsumes seven Criticals/Highs.
5. **Internal C6 + C7 + M13 + M14 + Upstream #7** (typed-time
   OCaml↔Rust unification): atomic IR landing; the upstream's
   date-validation requirement folds into this commit cleanly.
6. **Internal C1** (`survey_top_k` posterior ranking): the
   silent-wrong-init.
7. **Upstream #6** (likelihood dim-check): remove blanket
   permissive_dim; isolate the He-et-al. relaxation.
8. **Internal C5** (profile-PMMH params/loglik): drop `.max(best_ll)`.
9. **Upstream #4** (init validation): hooks into the indexed-
   reference resolver from (4); land together.
10. **Internal C3** (Gillespie inhomogeneous-Poisson): document +
    guard; thinning fix later.
11. **Internal C4** (gh#69 missing tests): pure-test commit.
12. **Upstream #8** (duplicate names): namespace uniqueness pass.

After 1-12, the remaining Mediums and Lows from both reviews can
land opportunistically as their surfaces are touched.

## What this exercise teaches about review methodology

A diff-scoped review (mine) and a spec-vs-implementation review
(upstream) are different instruments. Both should be run; neither
substitutes for the other.

For the next audit, I would run *both* methodologies in parallel:
- A diff-scoped pass over the last N commits (catches new defects,
  recent-surface scrutiny)
- A spec-vs-implementation pass over each major surface area
  (catches systemic drift the diff-scoped pass cannot see because
  it predates the diff)

A single combined sub-agent prompt for the spec-pass would have
caught most of upstream #1, #2, #3, #5, #6, #8 — the internal
review's six-cluster decomposition siloed each sub-agent into a
narrow scope where the broader spec-vs-implementation question
was never asked.
