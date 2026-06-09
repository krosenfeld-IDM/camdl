# Golden-file feature-coverage gaps & high-ROI additions

**Date:** 2026-06-08 **Type:** investigation / coverage audit (notes, not an
incident — no reproduction of a wrong answer, just a map of what the golden
surface does and does not exercise) **Scope:** the 37 positive model goldens in
`ocaml/golden/*.camdl` vs the language spec (`docs/camdl-language-spec.md`).
Question: where are the gaps, and which _small set_ of new goldens closes the
most spec surface per file?

All gap claims below are verified against the source by grep, not inferred from
docs; the commands and their output are in the **Verification appendix**. Two
doc-derived beliefs that turned out to be **stale** are flagged so they don't
get re-promoted: coupling sugar and the `date()` path (see "Not gaps").

---

## 1. What a golden actually buys (the ROI mechanism)

Adding `ocaml/golden/foo.camdl` + `make update-golden` automatically enrolls it
in three test paths — verified by reading the harnesses:

| Path                  | File                                        | What it exercises                                                                                                                                                                           |
| --------------------- | ------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| L2 IR round-trip      | `ocaml/test/` round-trip                    | compile → expand → serialize → deserialize → structural equality (OCaml side)                                                                                                               |
| L4 smoke + invariants | `rust/crates/sim/tests/smoke_all_golden.rs` | globs every `ocaml/golden/*.ir.json`, applies the first preset, **clears interventions** (`:76`), runs **gillespie + chain_binomial** (`:84–110`), asserts non-negative counts + invariants |
| L7 cross-language     | `tests/test_ocaml_to_rust.sh`               | `camdlc` compile → `camdl simulate` on **gillespie + chain_binomial** (`:45`)                                                                                                               |

So a new golden buys: _"this feature compiles, expands, survives the OCaml↔Rust
IR contract, and simulates without panicking or violating invariants on the two
stochastic backends."_ That is exactly the coverage that a feature used by
**zero** goldens has **none** of today.

Three things a golden does **not** buy automatically (so they can't be closed by
adding a golden alone — see §4):

- **ODE backend** execution — the smoke list is gillespie+chain_binomial only.
- **Intervention _firing_** — smoke calls `model.interventions.clear()`, and
  `camdl
  simulate` runs the baseline scenario (interventions disabled), so
  dynamics under a fired intervention are never hit by the auto-discovered
  golden paths.
- **Inference (PF/IF2/PGAS)** — no auto-discovery; only
  `seir_spatial_5_inference` carries inference params, picked up by dedicated
  tests.

---

## 2. Feature → coverage matrix

Legend — **Sev** (gap severity / ROI of closing it): ★★★ high, ★★ medium, ★ low,
— none. "Covered by" lists representative model goldens (not exhaustive).
"Corner-only" = exists only in `tests/fixtures/corner_cases/` (hash-regression
fixtures with baked params, _not_ the L2/L4/L7 model surface, and not behavioral
oracles).

### 2a. Features with NO model-golden coverage (the gaps)

| Feature                                                | Spec       | Importance                                                        | Status today                                                                                         | Sev |
| ------------------------------------------------------ | ---------- | ----------------------------------------------------------------- | ---------------------------------------------------------------------------------------------------- | --- |
| **`if…then…else` (Cond expr)**                         | §9.7       | CORE — div-by-zero guard in Gillespie; threshold/behavioral rates | **0 goldens.** Smoke harness even _recommends_ Cond guards (`smoke_all_golden.rs:67`) yet none exist | ★★★ |
| **`events {}` block** (`add`/`transfer`/`set`)         | §13.5–13.6 | COMMON — cohort entry, importation; inference-relevant            | **0 model goldens**; corner-only (4 fixtures)                                                        | ★★★ |
| **`balance {}` constraint**                            | §13.8      | COMMON — population conservation; chain-binomial-only capability  | **0 model goldens**; corner-only (2 fixtures)                                                        | ★★★ |
| **`interpolated` forcing** (read file + interp method) | §7         | COMMON — data-driven β(t), covariate forcing                      | **0 goldens.** Entire file-read + interpolation path unexercised by goldens                          | ★★★ |
| **Likelihood `poisson`**                               | §12.2      | COMMON                                                            | **0 model goldens**; corner-only                                                                     | ★★  |
| **Likelihood `normal`**                                | §12.2      | COMMON                                                            | **0 anywhere positive** (prior greps matched `log_normal`)                                           | ★★  |
| **Likelihood `beta_binomial`**                         | §12.2      | NICHE — overdispersed test positivity                             | **0 positive**; only the `e304_*` error fixture                                                      | ★★  |
| **Intervention `add()` action**                        | §13.1      | COMMON — importation/cohort entry                                 | implemented (lexer `ADD`, parser 563–572); **0 model goldens** (`transfer()`=7); corner-only         | ★★  |
| **Intervention `set` action (`ASet`)**                 | §13.1      | NICHE                                                             | **spec/code mismatch**: the spec's `set(COMP, value=…)` call form does **not** parse (no `SET` token); only a bare `COMP = expr` block form maps to `ASet` (parser 622–624, "simplified"), and it's untested everywhere | ★   |
| **`at_day` recurring schedule**                        | §13.7      | COMMON — annual SIA cadence                                       | corner-only (`all_lifecycle`)                                                                        | ★★  |
| **`unchecked_dim` escape**                             | §2.2.2     | NICHE — phenomenological α-mixing; documented escape hatch        | **0 goldens.** Ships through the golden surface untested                                             | ★★  |
| **`deterministic()` rate wrapper**                     | §9.8       | NICHE                                                             | implemented (`EFuncCall→DrawDeterministic`, `expander.ml:2467`, backend `chain_binomial.rs:472`); **0 goldens** | ★   |
| **scenario `scale` / `disable`**                       | §17.1      | COMMON — `scale` drives the CRN-coupling path                     | `scale`=0, `disable`=0 goldens (`set`=37, `enable`=7)                                                | ★   |
| **`timepoints {}` user block**                         | §14        | NICHE                                                             | **0 goldens**                                                                                        | ★   |
| **Typed let (`let x : kind = …`)**                     | §8.4       | NICHE                                                             | **0 goldens**                                                                                        | ★   |

### 2b. Features that ARE well covered (don't reinvest)

| Feature group                                                                                                         | Spec         | Covered by (representative)                                                                  |
| --------------------------------------------------------------------------------------------------------------------- | ------------ | -------------------------------------------------------------------------------------------- |
| Compartments, transfer/inflow/outflow transitions                                                                     | §3, §9.1     | sir_basic, sir_demography, sir_five_age, ross_macdonald                                      |
| Stratification (1×, 5×, partial), indexed transitions, `where` guards                                                 | §5, §9.2–9.3 | seir_age, sir_patches_5, polio_spatial_5, seir_erlang, seir_defines_adj                      |
| `consecutive()` iterator, compartment iteration                                                                       | §9.4–9.5     | sir_five_age, seir_erlang, seir_erlang_staged                                                |
| Multi-source (`A+B-->`) + catalyst, probabilistic branching                                                           | §9.1.1–9.1.2 | bimolecular, ross_macdonald, malaria_two_species, branching_si_symp_asym                     |
| `sum()` reduce, table lookup                                                                                          | §9.7         | seir_age, sir_spatial_sum, seir_erlang_staged                                                |
| Tables: inline 1D/2D, unit-annotated, parameterized, `read()` file, sparse `default`, multi-value cols, dim-from-data | §6           | seir_age, seir_age_table_rates, sir_init_table, seir_defines_patch, seir_defines_adj         |
| Sinusoidal + periodic-step forcing, indexed forcing                                                                   | §7           | seir_vaccine_seasonal, seir_seasonal_patch, seir_spatial_5_inference, sirv_anchored_calendar |
| `overdispersed()`                                                                                                     | §9.8         | sir_overdispersion, sir_two_overdispersed, seir_spatial_5_inference                          |
| Real compartments + `ode {}` block (compile/round-trip only)                                                          | §3, §11      | sir_reservoir, sir_reservoir_mixed                                                           |
| Observations: incidence/prevalence/derived projections; neg_binomial, bernoulli, diagnostic_test sugar; indexed obs   | §12          | seir_observations, seir_age_incidence_sum, ross_macdonald, seir_spatial_5_inference          |
| Intervention `transfer()`; `at`/`every` schedules; indexed interventions; instance-level `enable`                     | §13          | seir_vaccine, polio_spatial_5, sia_instance_enable                                           |
| Priors (`~`), bounds, indexed params, dim annotations                                                                 | §4, §20      | sir_priors, sir_dim_annotated, sir_two_patch                                                 |
| `date()`/`origin`/calendar arithmetic, instant tables, `date_range`, `add_calendar_*`                                 | §2.3         | sia_anchored_dates, sirv_anchored_calendar                                                   |
| `dt` in rate                                                                                                          | §14          | sir_dt, corner/dt_rate                                                                       |

### 2c. Not gaps — verified, do **not** add (stale doc beliefs)

- **Coupling sugar `coupling[dim = M]`** — **removed from the language.**
  `sir_coupling.camdl:3`: _"The old coupling[] sugar has been removed; this is
  the recommended explicit pattern."_ No token in the lexer/parser. The explicit
  `sum(b in age, C[a,b]*…)` form is canonical and is covered.
- **`date()` / `origin` / calendar arithmetic** — **covered** by
  `sia_anchored_dates` and `sirv_anchored_calendar`. An archived proposal called
  this "untested"; that predates these two goldens. The conversion path is
  exercised today.

---

## 3. Ranked high-ROI new goldens

Tight set (6 files), ranked by (spec surface closed × importance) ÷ cost. Each
is a _realistic_ model (the DSL is human-first; a coverage golden should still
read like epidemiology) and bundles features that naturally co-occur. Neutral
illustrative scenarios only — no external collaborators/projects named.

**This table reflects two review passes** (adversarial-technical + ID-modeler) —
see §5 for what changed and why. The headline revisions: the Cond model uses the
**div-by-zero guard** (the canonical, harness-recommended use) rather than a
prevalence-threshold behavioral switch (which chatters near the threshold);
`events` and `balance` are **split** (a fixed-total balance fights a population-
changing `add()` — they conflict in one model); likelihood families moved to
contexts where each is **defensible**; and `set()` was dropped from every
proposal (the documented call form doesn't parse — §2a, §5).

| #     | Proposed golden                            | Gaps closed (matrix rows)                                              | Imp.     | Cost | Rationale (post-review)                                                                                                                                                                                                                            |
| ----- | ------------------------------------------ | ---------------------------------------------------------------------- | -------- | ---- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| **1** | `sir_guarded_foi.camdl`                    | **Cond / if-then-else** (+ scenario `scale`/`disable` fold-in)         | ★★★ CORE | low  | Highest-ROI gap: a CORE expr node, the Gillespie div-by-zero guard, _recommended by the smoke harness itself_, in zero goldens. Realistic + canonical: a metapopulation where a small patch can empty, `@ if N[q] > 0 then w*I[q]/N[q] else 0`. No stiffness/chatter (unlike a prevalence-threshold switch — §5). |
| **2** | `seir_pop_balance.camdl`                   | **`balance {}`** (+ time-function-as-total, typed let)                 | ★★★      | low  | Spec §13.8's own canonical use: reconcile to a known population trajectory, `balance { R = pop(t) - S - E - I }`. Forces chain-binomial dispatch on a model golden for the first time. Coherent (no `add()` to fight the conserved total).        |
| **3** | `seir_seasonal_importation.camdl`          | **`events {}`** + **`add()` action** + **`at_day` schedule**           | ★★★      | low  | Three gaps in one realistic model: annual reseeding from importation, `events { reseed : add(I, seed) every 365.25'days at_day D }` (the exact shape `all_lifecycle` proves compiles). Open population, so no balance conflict.                    |
| **4** | `flu_data_forcing.camdl` (+ `data/*.tsv`)  | **`interpolated` forcing** (file read + interp method)                 | ★★★      | med  | Whole data-driven-forcing path is golden-dark. Realistic + non-circular: school-term or temperature covariate driving β(t), weekly cases `~ neg_binomial` (the standard surveillance likelihood — §5). One companion data file.                  |
| **5** | `surveillance_likelihoods.camdl`           | **`poisson`** + **`normal`** + **`beta_binomial`** likelihoods         | ★★       | low  | All three uncovered families, each in a _defensible_ slot: seroprevalence `~ beta_binomial` (overdispersed positivity), rare well-ascertained deaths `~ poisson`, large-count weekly stream `~ normal` (the count-normal-approx the spec intends). |
| **6** | `phenom_mixing_unchecked.camdl`            | **`unchecked_dim`** (+ `deterministic()` wrapper)                      | ★        | low  | Pins the documented dimensional escape (α-mixing FOI `unchecked_dim((I+iota)^alpha, dim=…, reason=…)`), shipping with no golden. `deterministic()` _is_ implemented (§2a) — folds in cheaply.                                                      |

**Fold-ins** (ride along, not their own files): scenario `scale`/`disable` → a
scenario block on #1; typed let → #2; `timepoints {}` → #2 or #3.

**Net:** 6 files close every ★★★ and ★★ row plus most ★ rows. Commit **#1–#4
now** (all ★★★, low/med); #5–#6 fast-follow. Two ★ rows are intentionally left
open: the `set`/`ASet` intervention action (blocked on a spec/code fix — §2a) and
`timepoints {}` (parsed-but-not-usable-in-expressions per the spec).

---

## 4. Harness gaps — _cannot_ be closed by adding a golden alone

These need a test-harness change, not (only) a new model. Flagging so they
aren't mistaken for golden gaps:

- **H1 — ODE backend has zero golden coverage.** `smoke_all_golden.rs:84–110`
  and `test_ocaml_to_rust.sh:45` run gillespie+chain_binomial only.
  `sir_reservoir{,_mixed}` declare real compartments + `ode {}` but the ODE
  _backend_ never runs on them. Fix: add ODE to the smoke backend list (guarded
  by capability), so existing real-compartment goldens start exercising it.
  Recommend a follow-up issue.
- **H2 — interventions are cleared before every smoke sim**
  (`smoke_all_golden.rs:76` — only `model.interventions.clear()`; `events {}` and
  `balance {}` are **not** cleared, so goldens #2/#3 _do_ fire those effect paths
  in smoke). The auto-discovered golden path therefore never simulates a _fired
  intervention_; intervention dynamics are pinned only by corner-case hash
  fixtures + dedicated lifecycle tests. Fix: run at least one
  enabled-intervention scenario per intervention-bearing golden (e.g. add a
  scenario with `enable = […]` to `seir_vaccine` and don't clear it).
- **H3 — inference is not auto-discovered.** Only `seir_spatial_5_inference`
  carries inference params. Consider a tagged subset that PF/IF2/PGAS
  smoke-tests run over.

(Secondary, out of scope here: per the agent sweep, the bulk of the
E2xx/E4xx/E5xx error codes lack negative fixtures — `ocaml/golden/errors/`
covers E267 and E300–E304 only. That's a separate negative-coverage audit, not a
positive feature-golden gap.)

---

## 5. Adversarial review outcomes

Two reviewers were run against the draft: an adversarial-technical reviewer
(re-verify every claim, find missed gaps, falsify proposals) and an ID-modeler
reviewer (scientific suitability of the proposals). Both findings were
independently re-checked against source before acting — a reviewer's claim is no
more load-bearing than the author's.

**Technical reviewer — confirmed (note updated):**

- The documented intervention **`set(COMP, value=…)`** call form **does not
  parse** — there is no `SET` token (`grep '"set"' lexer.mll` → none) and the
  only path to `ASet` is a bare `IDENT = expr` inside an intervention block
  (`parser.mly:622–624`, comment: "action hint -- simplified"), untested
  everywhere. → Split the old "set()/add() actions" gap row; dropped `set()`
  from all proposals; filed it as a spec/code mismatch (§2a).

**Technical reviewer — _wrong_ (claim rejected after verification):**

- Claimed `deterministic()` is "documented but not implemented (no token, no
  rule)." **False.** Like `overdispersed()`, it is a function-call form, not a
  keyword, so a token search misses it: `expander.ml:2467` maps
  `EFuncCall("deterministic", …) → Ir.DrawDeterministic`; round-tripped in
  `serde.ml:265`; executed in `chain_binomial.rs:472` (`Deterministic =>
  mean.round()`). Proposal #6 stands.
- Claimed `add()` "is implemented but" implied uncertainty. Confirmed
  implemented and _exercised in compiling fixtures_ (`all_lifecycle.camdl:27`,
  `event_intervention_agree.camdl:38`). Kept as a real, low-risk gap (proposal
  #3).

**ID-modeler reviewer — accepted with one correction:**

- **#1 (Cond):** reviewer wanted the hard prevalence-threshold β-switch replaced
  with a smooth Hill/logistic response (chattering/stiffness near the
  threshold). Correct on the hazard — **but a smooth Hill uses no conditional
  and would delete the very Cond coverage that is the point.** Reconciled by
  switching to the _canonical_ Cond use instead: the div-by-zero guard
  `if N > 0 then … else 0` in a model with emptyable strata (exactly what
  `smoke_all_golden.rs:67` recommends). Keeps the coverage, drops the hazard.
- **#2 balance vs births:** reviewer showed a fixed-total `balance` _fights_ a
  population-changing `add()` (the conserved total forces a residual down by the
  cohort size). Correct — so **events and balance were split**: #2 uses the
  spec's own canonical `balance { R = pop(t) - S - E - I }` (reconcile to a known
  total, no `add()` conflict); #3 carries `events`/`add()` in an open population.
- **#3/#5 likelihoods:** Poisson is weak for overdispersed flu cases; reviewer
  recommended neg_binomial for case counts and reserving Poisson/normal for slots
  where they're defensible. Adopted: case counts `~ neg_binomial`; the
  coverage-required Poisson/normal/beta_binomial moved into a surveillance model
  (#5) where each family is genuinely appropriate (deaths, large-count stream,
  serosurvey positivity).
- **#5 SIA actions:** `set()`/`add()` are contrived for vaccination (real SIAs
  `transfer()`); converged with the technical finding above — `set()` dropped,
  `add()` relocated to importation (#3) where it's the right idiom.
- **#6 (unchecked_dim):** "keep as-is — scientifically sound." Unchanged.

**Missed-gap sweep:** `piecewise` forcing (§7) is also golden-dark (it _is_ in
the verification appendix but was not elevated to the §2a table). It is a
non-repeating step function — lower-value than `interpolated` (which subsumes the
data-driven case) but a genuine omission; fold a `piecewise` lockdown multiplier
into #4 if cheap, else leave as a known ★ gap.

**Net effect of review:** 0 proposals invalidated, 1 reframed (#1), 1 split
(#2→#2+#3), likelihood slots corrected, `set()` removed. The set count stayed at
6.

---

## Verification appendix

Counts and claims above, reproduced from source on 2026-06-08 (worktree
`goldens`, `main` @ f423b4c0). Model goldens = top-level `ocaml/golden/*.camdl`;
corner = `tests/fixtures/
corner_cases/*.camdl`.

```
# Inventory: 37 positive model goldens, 11 negative error fixtures, 5 data files
$ ls ocaml/golden/*.camdl | wc -l                      → 37
$ find ocaml/golden/errors -name '*.camdl' | wc -l     → 11

# Zero model-golden coverage (corner-only or absent):
$ grep -rl 'events'  ocaml/golden/*.camdl              → (none)   # corner: 4 files
$ grep -rl 'balance' ocaml/golden/*.camdl              → (none)   # corner: 2 files
$ grep -rln 'if .* then' ocaml/golden/*.camdl          → (none)   # Cond
$ grep -rln 'unchecked_dim' ocaml/golden/*.camdl       → (none)
$ grep -rln 'deterministic(' ocaml/golden/*.camdl      → (none)
$ grep -rln 'interpolated' ocaml/golden/*.camdl        → (none)
$ grep -rln 'piecewise'    ocaml/golden/*.camdl        → (none)
$ grep -rln 'at_day' ocaml/golden/*.camdl              → (none)   # corner: all_lifecycle
$ grep -rlF 'timepoints' ocaml/golden/*.camdl          → (none)

# Likelihood families (word-boundary, '<fam>(' — excludes log_normal etc.):
poisson:0  neg_binomial:4  beta_binomial:0  bernoulli:1  normal:0  diagnostic_test:1
plain binomial:1 (ross_macdonald, base of diagnostic_test)

# Intervention/event action verbs (literal -F):
transfer(:7   add(:0   set(:0   scale(:0
# seir_vaccine.camdl:29 → sia_round_1 : transfer(fraction = vacc_frac, from = S, to = V) at [180,545,910]

# Scenario ops:
set = { :37   scale = { :0   enable = :7   disable = :0

# Harness (read, not inferred):
smoke_all_golden.rs:76  → model.interventions.clear(); // baseline: no interventions
smoke_all_golden.rs:84  → backends: gillespie + chain_binomial only (no ODE)
smoke_all_golden.rs:67  → comment recommends "explicit Cond guards" (but 0 goldens have one)
test_ocaml_to_rust.sh:45 → for backend in gillespie chain_binomial; do

# NOT gaps (stale doc beliefs corrected):
sir_coupling.camdl:3 → "The old coupling[] sugar has been removed; this is the
                        recommended explicit pattern."  (no coupling token in lexer/parser)
$ grep -rln 'date(' ocaml/golden/*.camdl → sia_anchored_dates.camdl, sirv_anchored_calendar.camdl

# Review-driven re-verifications (§5):
$ grep -nE '"set"' ocaml/lib/compiler/lexer.mll        → (none)   # no SET token; set(...) call form can't lex
parser.mly:622-624  → `IDENT EQ expr` → ASet (bare-assignment block form only, "simplified")
expander.ml:2467    → EFuncCall("deterministic", …) → Ir.DrawDeterministic   # deterministic() IS implemented
chain_binomial.rs:472 → DrawMethod::Deterministic => mean.round() as u64     # …and executed
all_lifecycle.camdl:27 → importation : add(I, 5) every 5 'days at_day 5      # add()+at_day compile (corner)
smoke_all_golden.rs:76 → model.interventions.clear()  ONLY (events{}/balance{} survive → they fire in smoke)
```

**Method:** built a feature inventory from `camdl-language-spec.md` (§§2–14) +
cheatsheet/user-features/IR-spec, cataloged the 37 model goldens' feature usage,
then verified every gap claim by grep against source (above). Doc-only claims
from the survey were re-checked against code; the coupling-sugar and `date()`
items were corrected as a result.
