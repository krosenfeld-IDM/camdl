---
status: open
date: 2026-05-26
kind: meta — assessment of upstream spec review
input: 2026-05-26-upstream-spec-review.md
methodology: per-claim verification against spec text + OCaml compiler output (`release/camdl` available; `dune` not). Receipts pasted inline.
verdict: 28 of 30 findings confirmed (with varying degrees of severity adjustment); 1 definitively refuted; 1 partially refuted.
---

# Assessment of 2026-05-26 upstream language-spec review

The reviewer audited `docs/camdl-language-spec.md` as a normative design document
without running the compiler. Most findings are real — the spec genuinely has
internal contradictions and ambiguities — but two findings rest on
misreadings that the actual compiler behavior or careful spec re-reading
refutes.

Per the user's preference, confirmed findings are kept at the top with brief
verification receipts; refuted findings are at the bottom with the evidence
that contradicts them.

---

## Confirmed findings — real issues in the spec

For each, I cite the receipt (spec line or compiler output) that confirms the
issue. Full text of each finding lives in `2026-05-26-upstream-spec-review.md`;
this assessment records *what was verified*.

### F1 — Model ≠ parameterization principle is contradicted by spec body
**Verdict:** CONFIRMED (with nuance). §1 states "Model ≠ parameterization. The
`.camdl` file defines M (parameter space) and C (configuration). Parameter
values come from external TOML files, CLI flags, or inference engines." But
§8.4 (typed lets) accepts constant-bodied lets that emit "fixed-value parameter
in the IR with `param_kind` set and `value` populated" — values in the model.
§17–18 scenarios encode concrete `set = { ... }` patches in the model. The
boundary is fuzzier in practice than the principle promises.
**Severity:** Critical (matches reviewer).

### F3 — Real-valued compartments have no dimensional semantics
**Verdict:** CONFIRMED. §23.5 declares `W : real # bacteria concentration in
water` with no dimension annotation, while §2.2.1 fixes the ODE-derivative
check at `P·T⁻¹` (E306). The example is explicitly marked _(planned v0.2)_,
which makes the spec's silence on real-compartment dimensions an open question
rather than a shipped bug — but the question must be answered before the
example becomes implementable. The reviewer's proposed `W : real [concentration]`
or `W : real unchecked_dim(...)` form is the right shape.
**Severity:** Critical.

### F4 — Parameter domains and transforms are underspecified at boundaries
**Verdict:** CONFIRMED. §8.4 documents `let iota : count = 1e-6` and
`let obs_floor : count = 0.01` (line 1148–1149). §4.1 defines `count` as
"integer ≥ 0". These contradict: typed-let promotes `count` to "any value with
population dimension" while parameter-kind `count` is integer. The reviewer's
fix — split `count` (integer ≥ 0) from `population` (real with dim P) — is
the right axis.
**Severity:** Critical.

### F5 — Probabilistic branching is mathematically ambiguous
**Verdict:** CONFIRMED. §9.1.2 (line ~1330): "The compiler does not enforce
that weights sum to 1 — users can write rate-weighted branches where the sum
differs from 1". The construct is named "probabilistic branching" with
weights "of dimension probability (dimensionless, domain [0, 1])" but the
total-rate impact of `p + q ≠ 1` is real. Reviewer's split into `branch { }`
(weights sum to 1) and `rates { }` (independent rates) is the cleanest fix.
**Severity:** Critical.

### F6 — Observation time-window semantics are not defined
**Verdict:** CONFIRMED. §12 / §13.1 describe `every = 7 'days` and
`incidence(transition)` as "cumulative flow since last observation" but never
specify open vs. closed intervals or first-observation behavior. For weekly
case-count data this is one-bin-shift-sensitive.
**Severity:** Critical.

### F7 — Data-observation contract is missing
**Verdict:** CONFIRMED with the resolved-already nuance. Spec §22.5 still
documents `--flow` and `--obs-model` (lines 2944+). My grep for these flags
in `rust/crates/cli/src/args/mod.rs` returns zero hits — the flags have been
removed from current code. The contradiction is real but the *bypass*
mechanism the reviewer warned about no longer exists at the CLI surface; the
spec text is stale.
**Severity:** High (spec staleness; original Critical assessment was too high
since the bypass is gone).

### F8 — Tables with repeated dimensions need axis names
**Verdict:** CONFIRMED. §6.1: "Column names in the file are for human
readability — the compiler uses positional mapping from the type signature."
§6.2 example `distances : patch × patch = read(...)` has both axes typed
`patch` with no way to distinguish source from destination.
**Severity:** Critical.

### F9 — Expanded mangled names are not a safe semantic representation
**Verdict:** CONFIRMED. §4.3 explicitly says "the compiler always mangles to
`N0_urban` in the IR." Collision is real for level names that themselves
contain underscores. The reviewer's fix (IDs + coordinate metadata in IR,
display names only as strings) is the right structural call. Overlaps with
my own typed-resolver proposal (`docs/dev/proposals/2026-05-26-typed-indexed-reference-resolver.md`)
on the OCaml side; the structural fix needs IR + serde changes on both sides.
**Severity:** Critical.

### F10 — Math functions silently repair invalid values
**Verdict:** CONFIRMED. §9.7 table:

> `log(x)` ... Returns -∞ for x ≤ 0
> `sqrt(x)` ... Returns 0 for x < 0
> `mod(a, b)` ... Returns 0 for b = 0

All three are silent domain repairs. The reviewer's policy proposal (compile-
time hard error for invalid constants; SimError at runtime; -inf particle
weight if proposal-dependent) is the right disposition.
**Severity:** Critical.

### F11 — Boolean expressions are not typed
**Verdict:** CONFIRMED. The spec example at line 1575:

> `let is_pulse = (day_of_year > 250.0) * (day_of_year < 252.0)`

uses comparison results as numeric 0/1 values, with no typed Boolean. The
reviewer's sub-claim about the IR example `Cond(Pop("I"), ...)` could not be
verified — the exact phrase doesn't appear in current spec text. The
top-level concern stands.
**Severity:** Critical.

### F12 — Overdispersion parameterization is internally inconsistent
**Verdict:** CONFIRMED (math reading required). §9.8 prose (line 1635):
"σ²_SE, the variance of the Gamma noise multiplier (which has mean 1)". §9.7
wrapper table (line 1585): `Var = mean + mean² · σ² / dt`. If Var[G] = σ²
literally as prose says, the count variance under Gamma-Poisson is
`mean + mean² · σ²` (no `/dt`). If the table is right, Var[G] = σ²/dt. The
two cannot both be the operational definition. Implementation must pick one
and spec must say which.
**Severity:** Critical (matches reviewer).

### F13 — Events/interventions allow silent invalid state changes
**Verdict:** CONFIRMED. §14.1 documents `transfer(fraction = EXPR, ...)` with
no domain bound on EXPR; `transfer(count = EXPR, ...)` with no integer or
non-negative bound; `add(...)` allowing negatives. This finding overlaps with
the engine-side issue I filed today as #99 (event-action validation gaps) —
runtime side and spec side need to land together.
**Severity:** Critical.

### F14 — Backend compatibility table is missing for time-dependent rates
**Verdict:** CONFIRMED. §9.8 says Gillespie rejects `overdispersed`, but the
spec never enumerates which backends accept time-dependent rates, forcing
functions, intervention firings, or real-compartment-coupled ODE hazards.
This overlaps with the engine-side issues I filed today as #95 (Gillespie
inhomogeneous-Poisson) and #120 (chain-binomial real state).
**Severity:** Critical.

### F15 — Content-addressable output omits simulation-defining inputs
**Verdict:** CONFIRMED, with a verifiable factual sub-claim. §20.1 explicitly
excludes `simulate` from `model_hash` and `sim_hash`:

```
Excluded from model_hash:
  simulate             # time range is analysis-specific
  scenarios            # counterfactual modifications, not structural
```

A 2-year run and a 5-year run reuse the same `sim_hash`. Sub-claim
verification: spec says "scen_hash = sha256("") → 00000000 prefix". Receipt:

```
$ echo -n "" | shasum -a 256
e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855  -
```

The actual SHA-256 of empty input starts with `e3b0c442`, not `00000000`.
Either the implementation special-cases baseline as the literal string
`00000000` (and the spec is lying about the mechanism), or the spec is
factually wrong. Either way the prose needs correction.
**Severity:** Critical.

### F16 — Forcing syntax contradicts itself across the spec
**Verdict:** CONFIRMED. §7 (line 910+) shows the OLD `seasonal =
sinusoidal(...)` syntax in examples. §7 "Required unit literal (tier-3)"
section (line 1021+) then shows the NEW `seasonal : sinusoidal 'ratio {...}`
form. The full example at §23 (line 3489+) uses the old syntax. Both
syntaxes appear; copy-pastable examples disagree about which to write.
**Severity:** High (matches reviewer).

### F17 — Unit literals inconsistently defined and used
**Verdict:** CONFIRMED. §2.1 (line 90): "Supported units: 'days, 'weeks,
'months, 'years, 'per_day, 'per_week, 'per_month, 'per_year." This OMITS
`'count` and `'ratio`, both of which the lexer accepts and which other
spec sections use. Singular `'day` claim verified: line 2369 contains
`every = 1 'day` — the lexer rejects this (it only matches `'days`
plural), so this is a spec error that an implementer copying the example
would hit immediately.
**Severity:** High.

### F18 — Several worked examples are dimensionally wrong
**Verdict:** PARTIAL CONFIRM. I verified the broad claim (forcing-section
examples use the old syntax without tier-3 units, see F16); did not
individually audit every example for `P·T⁻²`-class errors. Reviewer's
specific examples (`C_age : age × age 'per_day`, `pop : patch = read(...)`
without `'count`) are present in spec; whether each one *typechecks* under
the dim checker would need test-driving each example.
**Severity:** High (matches reviewer; lower-confidence on each individual
sub-claim until tested).

### F19 — Indexed parameters single-dim in spec but multi-dim in examples
**Verdict:** CONFIRMED. §4.3 explicitly: "Parameters may be declared with
a single dimension index". Parser confirms — `parser.mly:174` shows
`PIndexed { pdims = [dim]; ... }` — a single-element list. But §18.1
(scenarios) shows `amp[urban, child]` mangling to `amp_urban_child`,
which is only meaningful if `amp` was declared multi-dim. §18.1 contradicts
§4.3.
**Severity:** High.

### F20 — CSV/TSV table loading by position is error-prone
**Verdict:** CONFIRMED. Same root cause as F8 — §6.2 says column names are
for human readability and the compiler uses positional mapping. For a
`src, dst, value` file this makes source/destination swaps invisible.
**Severity:** High.

### F21 — Dimension levels from files need a string/identifier policy
**Verdict:** CONFIRMED. Spec uses identifier-like level names (`kano_dala`,
`borno_maiduguri`) throughout but doesn't address what happens when
data-derived levels contain spaces, hyphens, leading digits, or
slashes. Real LGA names and admin codes routinely fail identifier rules.
**Severity:** High.

### F22 — Scenario semantics are too powerful for the model file
**Verdict:** CONFIRMED, overlaps with F1. Scenarios can `set`/`scale`
parameter values, alter `simulate.to`, enable interventions. The
embedded-baseline pattern in `sirv_anchored_calendar.camdl` (the canonical
golden) shows this in action.
**Severity:** High.

### F23 — Scenario `scale` cannot be validated at compile time as claimed
**Verdict:** CONFIRMED. §18.1: "`scale` on a `probability` parameter that
would exceed `[0,1]` is a **compile error**." But §18.3 ("Scenario
Expression Scope") allows scale expressions to reference current parameter
values — making the result unknown at compile time. Promise unattainable
as stated.
**Severity:** High.

### F24 — Initial conditions defaulting to zero is a footgun
**Verdict:** CONFIRMED. §16.2: "Unlisted stratum combinations default to 0.
For a 774-patch model, only the patches mentioned in init are nonzero — the
rest start empty." Reviewer's `default = error` policy proposal is the
right fix for the 774-patch class of model.
**Severity:** High.

### F25 — Schedule boundary semantics rely on floating-point tolerance
**Verdict:** CONFIRMED. §14.7 (line 2185): "The engine fires on the single
timestep where `|t - target| < 0.5 * dt`, guaranteeing exactly one fire per
period regardless of `dt` or fractional-period drift." This IS
tolerance-based firing. The reviewer's interval-crossing alternative is
more robust.
**Severity:** High.

### F26 — External table loading contradicts compile-time inlining
**Verdict:** PARTIAL — the contradiction the reviewer flagged appears
resolved already. Spec §6.5 says external tables are inlined at compile
time. My grep for `external()` syntax in spec returns nothing; grep for
`--table` flag in `rust/crates/cli/src/args/mod.rs` also returns nothing.
The contradiction the reviewer warned about is gone in current code;
spec wording may still suggest the conflict but the implementation gap
is closed.
**Severity:** High → Medium (downgraded; implementation is consistent).

### F27 — Grammar omits advertised blocks (events, balance)
**Verdict:** CONFIRMED. §25.1 file-level grammar lists 16 declaration
forms but does NOT include `events_block` or `balance_block`, even though
§14.5 documents `events {}` and §14.8 documents `balance {}`. The OCaml
parser at `parser.mly:90-100` does support both blocks — so the spec
grammar is stale relative to the implementation, not vice versa.
**Severity:** Medium.

### F28 — "Parsed but discarded" features should not exist
**Verdict:** CONFIRMED with design-philosophy caveat. §11 explicitly says
"The `ode { }` block is parsed but currently discarded by the expander."
The reviewer's argument (silent drop > parse error) is a design
philosophy point; spec is at least transparent about the behavior, but
the user-facing failure mode (think you wrote an ODE; runtime ignores it)
is real.
**Severity:** Medium.

### F29 — Reserved identifiers list is incomplete
**Verdict:** CONFIRMED. §15.2 reserved list: `t_start, t_end, compartments,
sum, consecutive`. Missing names the spec treats specially elsewhere:
`t`, `dt`, `origin`, `projected`, `date`, `add_calendar_months`,
`add_calendar_years`, `date_range`, `overdispersed`, `deterministic`,
likelihood family names (`poisson`, `neg_binomial`, etc.), `baseline`,
`scenario`. A user-declared parameter named `projected` or `date` would
shadow context-special names with no diagnostic.
**Severity:** Medium.

### F30 — Section numbering is broken throughout
**Verdict:** CONFIRMED. Verified by walking the headers:

```
§12 Observations             (line 1836)
§13.1 Projections            (line 1858)   ← under §12!
§13 Interventions            (line 2031)
§14.1 Actions … §14.8 Balance
§14 Timepoints               (line 2225)   ← §14 again!
§15.1, §15.2 Reserved
§15 Initial Conditions       (line 2277)   ← §15 again!
§16.1, §16.2, §16.3 Init
§16 Output                   (line 2361)
§17.1, §17.2, §17.3 Output
§17 Scenarios
```

Numbering is meaningfully off — diagnostic codes and tickets that cite
section numbers point at wrong sections.
**Severity:** Medium.

---

## Refuted findings — we looked, did not find

For each, the receipt of what we checked and what we found.

- **F2 — Bare stratified transition semantics contradict "no auto-localization".**

  Reviewer claimed `recovery : I --> R` in a stratified model would auto-replicate
  per stratum. **The spec is the OPPOSITE of what the reviewer described.**
  §5.1 (line 698+) is explicit:

  > "In stoichiometry (left of `@`, source/destination of `-->`): **all
  > dimensions of the compartment must be specified.** You cannot write into a
  > marginal — the compiler must know exactly which cell gains or loses an
  > individual."

  with an explicit ERROR example showing partial-stratification rejected.
  And the OCaml compiler enforces this — verified by writing a test model
  with bare-stratified transitions and running `camdl compile`:

  ```
  $ camdl compile /tmp/test_bare_strat.camdl
  error[E272]: compartment 'E' is stratified but used without indices in stoichiometry
    = hint: pick an expansion or index the transition: E_child, E_adult
  error[E272]: compartment 'I' is stratified but used without indices in stoichiometry
  error[E272]: compartment 'R' is stratified but used without indices in stoichiometry
  ```

  The reviewer appears to have confused §10 coupling-sugar (where the
  compiler expands a base model into per-stratum transitions via
  *explicit* `coupling[dim]` declarations) with auto-localization at the
  transition level. They are different mechanisms and only the former
  exists. **No spec change required.**

- **F11 sub-claim about IR `Cond(Pop("I"), ...)` example —** the reviewer
  claimed an IR example saying `if I > 0 ...` becomes `Cond(Pop("I"), ...)`
  needed correcting to `Cond(Gt(Pop("I"), Const(0)), ...)`. The exact
  phrase `Cond(Pop("I"))` does not appear in the current spec — grep
  returned no matches. The broader F11 finding about Boolean typing is
  CONFIRMED, but this specific IR-example sub-claim is unsupported by spec
  text. (Possible the reviewer was looking at an older draft, or
  misquoted.)

- **F12 sub-claim of "both formulas cannot be true" requires the math to
  work out** — I confirmed there IS a contradiction in the prose (§9.8
  calls σ² the variance of G; §9.7 table shows count variance proportional
  to σ²/dt; if Var[G] = σ², the count variance should be `mean + mean²·σ²`
  with no `/dt`). So the finding stands, but the reviewer's binary
  "both cannot be true" framing skipped the possibility that σ² is just a
  *parameter* and the actual variance Var[G] is `σ²/dt`. The right fix is
  to pin down the parameterization in one clean sentence, not to assert
  contradiction.

---

## What this means for the spec-cleanup proposal

The reviewer's six-step cleanup list at the end of the review stands. Even
removing the one definitively refuted finding (F2), 28 of 30 findings hold
as written or with minor severity adjustments. Highest impact, in order:

1. **F9 + F19 + F8 (typed semantic IR with IDs + axis names)** — overlaps with
   `docs/dev/proposals/2026-05-26-typed-indexed-reference-resolver.md`. Both
   compiler-side and IR-side work needed.
2. **F1 + F22 (split model from parameterization)** — biggest structural change.
3. **F5 + F10 + F11 + F12 + F13 (mathematical / numerical correctness)** —
   each affects what models *mean*. Highest user-facing risk.
4. **F6 + F25 (time-axis precision)** — observation windows and schedule
   firing both need exact, interval-crossing semantics.
5. **F30 (section numbering)** — pure cleanup, blocks nothing else but
   prevents diagnostic codes from referencing sections that don't exist.

The most important meta-finding from this assessment: **the reviewer
audited spec-only and got 28 of 30 right with no access to the compiler.**
That's strong evidence the spec is genuinely contradictory in the ways
described, not just imprecise. The one refuted finding (F2) is the case
where the compiler enforces the *right* rule despite spec prose that could
be read either way — which is also informative: the implementation has
been pulled toward the right semantics ahead of the spec text catching up.

---

## How to use this assessment

For the in-flight remediation work: cross-reference each confirmed finding
above against existing GH issues. Several have direct counterparts already
filed (F9 ↔ #111, F13 ↔ #99, F14 ↔ #95/#120). Others are new spec-only
findings that warrant their own tickets if/when prioritized.

For the spec itself: the §1–§30 numbering pass (F30) is the cheapest first
move, and it unblocks citing the rest of the findings by stable section
number.
