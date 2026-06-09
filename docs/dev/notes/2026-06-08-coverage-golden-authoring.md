# Authoring feature-coverage goldens — criteria & process

**Date:** 2026-06-08
**Type:** methodology note (how we generated the 2026-06 coverage goldens, so the
next author repeats the process instead of rediscovering it)
**Companion:** [`2026-06-08-golden-feature-coverage-gaps.md`](2026-06-08-golden-feature-coverage-gaps.md)
— the gap analysis that selected *what* to add. This note is *how* to add it.

## Three kinds of golden — don't confuse them

| Kind | Lives in | Pins | Failure means |
| --- | --- | --- | --- |
| **Coverage** (this note) | `ocaml/golden/*.camdl` | a language/IR feature compiles, round-trips, and simulates without panicking/violating invariants on both stochastic backends | a feature regressed or its surface changed |
| **Regression** | `tests/fixtures/corner_cases/` | exact forward trajectories (hash baseline) of tricky lifecycle/ordering cases | dynamics changed bit-for-bit |
| **Validation** | `tests/external/` | agreement with an external reference (pomp, closed form) | the math is wrong |

A coverage golden is **not** a behavioral oracle — smoke only checks finiteness +
non-negativity, not numerical correctness. Its value is breadth: every
`ocaml/golden/*.camdl` auto-enrolls in L2 (IR round-trip), L4
(`smoke_all_golden.rs`, gillespie + chain_binomial + invariants), and L7
(`tests/test_ocaml_to_rust.sh`). See the companion note §1 for what each buys.

## Selection criteria (what earns a new coverage golden)

A feature earns one when **all** hold:

1. **Implemented and shipping** — verify in the parser/expander, not just the
   spec. (The spec documents `set(COMP, value=…)` and `coupling[…]`; neither
   parses. Conversely `deterministic()` is implemented but spec-buried.)
2. **Used by zero existing model goldens** — grep `ocaml/golden/*.camdl`, not the
   corner cases (those are regression-only and don't run the L2/L4/L7 surface).
3. **Matters** — CORE/COMMON in the spec, or a documented escape hatch. Niche-but-
   cheap fold-ins ride along on a model selected for a bigger gap.

Then **bundle naturally-co-occurring features** into one realistic model (events
+ add + at_day → one importation model), and rank by (surface closed ×
importance) ÷ cost.

## Design principles (these are the tiebreakers)

- **Realism first.** The DSL is human-first; a coverage golden should read like
  epidemiology a health-ministry modeler would recognize, not a syntax fixture.
  This caught real problems in review (below).
- **Neutral scenarios.** No external collaborators/projects named; generic
  disease/setting framing (seasonal influenza, measles SIA, importation).
- **Header comment states the coverage purpose** — which feature, why this model,
  and any subtlety (e.g. why R is the balance target). The next reader shouldn't
  have to reverse-engineer intent.
- **Each model must satisfy the invariants on its own** — non-negativity is the
  one that bites (see gotchas). Pick initial conditions and parameters so the
  worst case stays ≥ 0.

## Process that worked (repeat this)

1. **Pick the gap** from the companion note's ranked table.
2. **Verify the surface syntax against `parser.mly` / `lexer.mll` / the expander
   _before writing_** — and find a *compiling* example (an existing golden or a
   corner-case fixture is ground truth; the spec can be stale). This step is
   where most time was saved/lost; see gotchas.
3. **Write the model** with a coverage-purpose header.
4. **Compile with the freshly-built compiler**, not `~/.local/bin`:
   `make build-ocaml` (generates `ir_version_generated.ml` — plain `dune build`
   fails without it), then
   `ocaml/_build/default/bin/camdlc.exe m.camdl > ocaml/golden/m.ir.json`.
   Compile one file; don't `make update-golden` (it rewrites all 37+).
5. **Run `cargo test -p sim --test smoke_all_golden`** — globs the new IR; runs
   gillespie + chain_binomial; checks invariants.
6. **Behavioral spot-check** with `camdl simulate … --scenario baseline --stdout`
   (or `--obs-only-dir` for observations) — confirm the feature actually does
   something (the guard's `else` fires, the event jumps the count, the obs
   streams sample), not just "didn't crash."
7. **Adversarially review** before trusting: re-verify each claim against source
   (a reviewer subagent's claim is no more load-bearing than the author's), and
   run the design past an ID-modeler lens for scientific suitability.

## Verified gotchas (the expensive-to-rediscover ones)

Each confirmed against source on 2026-06-08; cite when relevant.

- **`set(COMP, value=…)` does not parse.** No `SET` token; the only path to
  `ASet` is a bare `COMP = expr` inside an intervention block (`parser.mly:622`,
  "simplified", untested). Spec/code mismatch — don't author a `set()` golden
  until it's fixed. `add()` and `transfer()` are real.
- **`pop(t)` is not a builtin** — it's a user-declared forcing (`pop :
  sinusoidal/interpolated 'count {…}`); `pop(t)` just calls it. Forcing fields
  may reference parameters (`baseline = pop_mean`, `amplitude = alpha`).
- **`balance { R = pop(t) - S - E - I }` must target the accumulating remainder.**
  Balancing `S` is driven negative as recoveries pile up; `R` (the removed pool)
  grows and stays ≥ 0. Initialise the tracked compartments below the forcing's
  trough so the residual never goes negative. `balance` is **chain-binomial-only**
  (capability gate) → gillespie is *skipped* (not failed) in smoke.
- **`interpolated` forcing bakes the data into the IR at compile time** as
  `(times, values)` knots (`ir.ml:120`) — the runtime needs no file. **Make the
  data span the full sim range** (interpolation past the last knot is a hazard);
  `to = 2 'years` = 730.5 days, so data must reach ≥ 730.5. Loader picks the
  delimiter by extension: `.tsv` → tab, `.csv` → comma; columns matched by header
  name against `time_col`/`value_col`. Data files live in `ocaml/golden/data/`,
  referenced as `"data/<file>"`.
- **`deterministic()` and `unchecked_dim()` are function-call forms**
  (`EFuncCall`), like `overdispersed()` — recognized by name in the expander, no
  keyword token (so a token-grep falsely reports them "unimplemented"). Verified:
  `deterministic` → `expander.ml:2467`/`chain_binomial.rs:472`;
  `unchecked_dim(expr, dim = NAME, reason = "…")` with `dim ∈ {dimensionless,
  population, time, rate, population_rate, per_population}`, `reason` required.
- **`deterministic()` has no capability gate and gillespie ignores `draw_method`**
  entirely — so a `deterministic()`/`overdispersed()`-on-a-rate model still runs
  on gillespie (the rounding/overdispersion only affects chain-binomial counts).
  Only `OVERDISPERSION`, `REAL_COMPARTMENTS`, `BALANCE` gate the backend.
- **`Cond` (`if…then…else`) is lazy** (`propensity.rs:173`): predicate first, then
  only the taken branch. This is what makes `if N>0 then I/N else 0` safe — the
  divide in the taken-only branch never runs when N=0. A divide in the
  *predicate* (`if I/N > 0.3 …`) is **not** protected and hard-errors
  (`NumericalCollapse(DivByZero)`) by default if the denominator can be 0; the
  compiler does not catch this statically.
- **Autodiff differentiates `Cond` through the active branch, predicate left
  alone** (`autodiff.ml:185`); comparisons → 0. Gradient is exact a.e. but blind
  to a discontinuity whose location depends on a *fitted* parameter — so a
  guard whose predicate is over **state** (e.g. `N>0`) is gradient-safe, while a
  parameter-dependent hard threshold is an inference hazard. (`mod` over a fitted
  param is the one construct autodiff hard-refuses — via raw `failwith`, a known
  rough edge vs. the Diagnostics system.)

## What review changed (don't skip it)

The adversarial pass caught a conflated gap row and two "unimplemented" claims
that were false. The ID-modeler pass reshaped designs: the Cond model became a
state-predicate div-by-zero guard (not a chattering prevalence-threshold switch);
events and balance were split (a fixed-total balance fights a population-changing
`add()`); likelihood families moved to slots where each is defensible (Poisson →
rare deaths, normal → large-count stream, beta_binomial → serosurvey positivity).
Detail in companion note §5.

## Goldens added in this round (all green: compile + smoke + behavioral)

| Golden | Primary gap closed | Fold-ins |
| --- | --- | --- |
| `sir_guarded_foi` | `Cond` / `if-then-else` | scenario `scale`/`extends`, `BindingRef` |
| `seir_pop_balance` | `balance {}` | count forcing, typed let, forcing-field-references-param |
| `seir_seasonal_importation` | `events {}` + `add()` + `at_day` | — |
| `flu_data_forcing` | `interpolated` forcing (file read + interp) | neg_binomial |
| `surveillance_likelihoods` | `poisson` + `normal` + `beta_binomial` | derived projection, outflow `incidence()` |
| `phenom_mixing_unchecked` | `unchecked_dim` + `deterministic()` | sinusoidal `'per_day`/`'count` forcings |

Still open (deliberately): `set`/`ASet` action (blocked on the spec/code fix),
`timepoints {}` (parsed but unusable in expressions), `piecewise` forcing (★, a
known omission — fold into a future forcing golden).
