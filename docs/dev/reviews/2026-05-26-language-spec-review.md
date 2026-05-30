---
status: open
date: 2026-05-26
kind: language-spec review
scope: docs/camdl-language-spec.md — normative design document audit
reviewer: external / upstream
verification: per-finding code+spec checks added 2026-05-26 (paste-the-receipt per CLAUDE.md). Each finding carries a `Verification` block that records what we checked locally, the multi-axis status label (see legend below), and any severity adjustment.
verdict: |
  Most findings confirmed. No fully refuted design findings against the
  reviewed spec text. F2 (bare stratified) is implementation-protected at
  the compiler but still a stale spec example. F11 and F12 sub-claims I
  previously marked "refuted" were wrong — the IR example F11 cites IS in
  the spec, and F12 is a prose-precision issue, not a refutation. The
  cheap-batch fixes in commit 1a7ba112 are confirmed against the current
  spec file (this branch's HEAD), not just claimed.
status-legend: |
  Each finding carries one of these labels:
    confirmed-open                          — issue is in the spec; no fix landed
    confirmed-code-protected-but-spec-stale — compiler/runtime rejects the
                                              bad pattern, but the spec still
                                              teaches it; the spec is the
                                              contract, so the issue stands
    resolved-in-commit                      — fixed in a named commit AND
                                              verified line-cited against the
                                              current spec file
    needs-proposal                          — confirmed, but the fix is a
                                              structural change deserving a
                                              docs/dev/proposals/ doc
    text-fix-pending                        — confirmed, spec-prose-only fix
                                              that doesn't need a proposal
followup-commits:
  - 1a7ba112 (cheap-batch spec edits: F7, F15-subclaim, F16, F17, F27, F29, F30)
  - ab69bdbe (review restructure + F1 §1 spec rewrite)
revision-note: |
  This revision (2026-05-27) reflects upstream feedback on the earlier
  assessment. Three corrections applied:
    1. F2 moved out of "refuted" — implementation rejects bare stratified,
       BUT the §10 coupling-sugar example still contains the sentence
       "Progression and recovery are automatically replicated within each
       stratum (default behavior when no coupling is declared)" at lines
       3331-3333 of the current spec. The compiler's behavior does not
       refute the spec contradiction.
    2. F11 sub-claim restored to confirmed — the IR example
       `Cond(Pop("I"), <rate_expr>, Const(0.0))` does appear in the spec
       at line 1647. My earlier "exact phrase not found" claim was wrong
       (used the wrong grep pattern).
    3. F12 sub-claim reworded — the prose IS imprecise about whether
       σ²_SE equals Var[G] or σ²/dt equals Var[G]. The reviewer's
       "both cannot be true" framing was a fair complaint about
       imprecision, not a math error. Not a refutation.
  The reviewer's broader methodology point — "do not let implementation
  behavior refute a normative spec inconsistency. For camdl, the spec is
  the contract." — is the framing applied throughout this revision.
---

# camdl Language Spec Review — 2026-05-26

Reviewed the language spec as a **normative design document**, not as code. The
main issue is not that the spec is too ambitious; it is that several sections
promise mutually incompatible semantics. For a public-health modeling DSL, those
contradictions are dangerous because modelers will copy examples, assume the
documented behavior is enforced, and get a different model than intended.

Per-finding verification notes (added 2026-05-26): for each confirmed finding
the spec sections cited by the reviewer were read end-to-end, and any specific
code claim (CLI flag exists, compiler accepts a construct, IR field is
populated, etc.) was checked against the actual implementation. Receipts — grep
output, OCaml compiler stderr, parser.mly line citations — are pasted inline so
the verdict is reproducible. The one definitively refuted finding and two
refuted sub-claims are listed at the bottom of the file rather than at their
original severity position.

# Critical findings

## 1. The spec violates its own "model ≠ parameterization" principle

**Location** — §1, §4, §4.2, §17–18, §20–21, §24

**Category** — bad abstraction; reproducibility; user footgun

**Defect** — The opening contract says `.camdl` files define model structure and
that parameter values, inference configuration, and scenario selection are
external. Later sections allow optional parameter defaults in the model, priors
in the model, concrete scenario `set` values in the model, `baseline` scenarios
with parameter values, and a compilation pipeline that takes `params.toml`
before expansion.

**Why it matters** — A reviewer cannot tell whether a `.camdl` file is a
structural model, a calibrated analysis, a scenario definition, or an inference
prior bundle. That breaks reviewability and hashing. Changing a prior or
scenario value can change posterior or counterfactual conclusions without
looking like a model change.

**Fix** — Split the specification into three contracts:

```text
camdl-model-spec.md       structural DSL only
camdl-run-spec.md         params, scenarios, seeds, output, caching
camdl-inference-spec.md   priors, transforms, fit stages, diagnostics
```

Then enforce the boundary:

- `.camdl` may declare parameter names, kinds, dimensions, and optional
  structural bounds.
- Concrete parameter values live in params/scenario/experiment files.
- Priors either move to fit config, or the opening principle must be rewritten
  to say priors are part of the model contract.
- `camdlc compile` must not require parameter values.

**Severity** — Critical → **Medium** (spec-text, not design)

> **Verification (2026-05-26).** The observation is correct: §1 said
> "Parameter values come from external TOML files, CLI flags, or inference
> engines" as a strong prohibition, while §8.4 (typed lets like
> `let iota : count = 1e-6`), §17 (scenarios with `set = { beta = 0.3, … }`),
> and §4.2 (params.toml read at compile time) all let values live inside the
> `.camdl` file. The reviewer's underlying complaint — "is this file a
> structural model or a calibrated analysis?" — is real.
>
> **The reviewer's proposed fix (split into three spec files; ban values
> from `.camdl`) is rejected.** Self-contained reproducible models are a
> first-class use case here: a paper-supplement `.camdl` that runs
> end-to-end with no auxiliary files is more honest and more reproducible
> than three files a colleague has to assemble. Layered precedence
> (CLI / params.toml / scenarios / model defaults) is the documented design,
> and the hash discipline already does the work the reviewer was worried
> about: `model_hash` captures structure, `sim_hash`/`scen_hash`/`fit_hash`
> capture values + analysis config separately. A reviewer asking "is this a
> structural change or a parameter sweep?" reads the hash provenance, not
> the file shape.
>
> **The right fix is the §1 paragraph itself, not the design.** Rewrote §1's
> "Model ≠ parameterization" paragraph in the same commit as this
> assessment update: it now describes the two equally-valid file shapes
> (structural skeleton vs self-contained reproducible model), names layered
> precedence as the mechanism, and names the hash discipline as the way
> structural identity stays visible across analyses. With that text in
> place, the §1-vs-rest-of-spec contradiction the reviewer flagged is
> gone. Severity downgraded from Critical to Medium — it was always a
> spec-prose issue, not a structural one.

## 2. Bare stratified transition semantics contradict "no auto-localization"

**Location** — §1, §5.1, §10, §23.3, §25–26

**Category** — user footgun; bad abstraction; scientific correctness

**Defect** — The spec says bare compartment names after stratification are global sums and that stoichiometry must fully specify all dimensions. But the coupling-sugar example uses:

```camdl
progression : E --> I @ sigma * E
recovery    : I --> R @ gamma * I
```

and says these are "automatically replicated within each stratum." That is auto-localization.

**Why it matters** — This is one of the most dangerous possible ambiguities. Does `recovery : I --> R` in an age × patch model mean one global transition, a compile error, or one recovery transition per age × patch cell? Those are three different epidemic models.

**Fix** — Pick one rule and make it universal. The safer rule is:

```text
Bare stratified compartments are illegal in stoichiometry.
```

Then coupling/lifting must be explicit syntax, for example:

```camdl
lift[age, patch] {
  progression : E --> I @ sigma * E
  recovery    : I --> R @ gamma * I
}
```

or the coupling sugar must be the only context where bare stratified stoichiometry is legal. Do not describe implicit replication as default behavior.

**Severity** — Critical → **High** (spec/UX inconsistency rather than Critical runtime defect, since the compiler protects against the runtime hazard — but the spec is the user-facing contract)

> **Verification (2026-05-26, revised 2026-05-27).** **Status: confirmed-code-protected-but-spec-stale.**
>
> *Implementation side:* The OCaml compiler rejects bare stratified transitions in stoichiometry. Verified by writing a test model with `progression : E --> I @ sigma * E` (with `E` stratified by age) and running `camdl compile`:
>
> ```
> error[E272]: compartment 'E' is stratified but used without indices in stoichiometry
>   = hint: pick an expansion or index the transition: E_child, E_adult
> ```
>
> *Spec side:* §10 coupling-sugar (lines 3331-3333 of the current spec) still contains the contradictory sentence:
>
> > "...Progression and recovery are automatically replicated within each stratum (default behavior when no coupling is declared)."
>
> A user copying this example would write code the compiler rejects; the spec teaches a pattern that doesn't work. The runtime hazard is gone but the documentation hazard is real, and the spec is the contract. **Earlier "refuted" verdict was wrong:** compiler protection does not refute a normative spec inconsistency.
>
> **Fix:** delete the "automatically replicated within each stratum" sentence in §10 (or introduce an explicit `lift[age] { ... }` syntax and gate the coupling-sugar example on it). The reviewer's proposed `lift[age, patch] { ... }` form is the clean shape if explicit lifting is added; otherwise the example needs to use fully-indexed transitions.

## 3. Real-valued compartments have no dimensional semantics

**Location** — §2.2.1, §3, §11, §23.5

**Category** — type design; numerical correctness

**Defect** — The dimensional checker says compartment references have dimension
`P`, and ODE derivatives must have dimension `P·T⁻¹`. But real compartments are
introduced for things like environmental reservoirs, where `W` is not a
population count. The cholera example uses `W` as a concentration, yet the
language has no way to declare the dimension of `W` or `K`.

**Why it matters** — This makes the environmental-reservoir example either
dimensionally invalid or silently treated as population. A modeler can write a
waterborne force of infection that passes only because the checker lacks the
vocabulary to represent concentration, dose, viral load, or wastewater signal
units.

**Fix** — Require dimensions on real compartments:

```camdl
compartments {
  W : real [concentration]
}
parameters {
  K : positive [concentration]
  xi : positive [concentration/T]
  delta : rate
}
```

If arbitrary named dimensions are too much for v0.3, require at least:

```camdl
W : real [1]
W : real [P]
W : real unchecked_dim(reason = "...")
```

Also change E306 from "ODE derivative must have dimension `P·T⁻¹`" to "ODE
derivative must have the compartment's declared dimension divided by time."

**Severity** — Critical

> **Verification (2026-05-26):** §22.5 cholera example (now renumbered from
> §23.5) declares `W : real # bacteria concentration in water` with no dimension
> annotation; §2.2.1 fixes E306 at `P·T⁻¹`. The example is marked _(planned
> v0.2)_, which makes this an open design question rather than a shipped bug —
> but the question must be answered before the example becomes implementable.
> The reviewer's `W : real [concentration]` or `W : real unchecked_dim(...)`
> form is the right axis. Confirmed.

## 4. Parameter domains and transforms are underspecified at the boundaries

**Location** — §4.1, §4.4, §8.4, §21.2

**Category** — statistical correctness; user footgun

**Defect** — `rate` is documented as `≥ 0` with log transform; `probability` is
`[0,1]` with logit transform; `count` is integer ≥ 0 but examples use
`let iota : count = 1e-6` and `let obs_floor : count = 0.01`. Finite upper
bounds on positive/rate/count parameters do not override the default log
transform.

**Why it matters** — A log transform cannot represent exactly zero. A logit
transform cannot represent exactly 0 or 1. A count cannot be both an
integer-valued parameter and a tiny population-dimensional continuity
correction. These ambiguities directly affect inference, priors, initialization,
and likelihoods.

**Fix** — Split dimension from discreteness:

```camdl
count        # integer-valued population count
population   # real-valued quantity with dimension P
positive [P] # positive real population-scale value
```

Then define transforms by domain, not kind:

```text
(0, ∞)              -> log
[lo, hi], finite    -> logit interval
[0, ∞) with zero allowed -> boundary-aware transform or fixed-only boundary
integer count       -> not continuously transformed unless explicitly relaxed
```

Examples like `iota` should be:

```camdl
let iota : population = 1e-6
```

not `count`.

**Severity** — Critical

> **Verification (2026-05-26):** §8.4 (line 1148–1149) documents
> `let iota : count = 1e-6` and `let obs_floor : count = 0.01` as the canonical
> pattern, while §4.1 defines `count` as "integer ≥ 0". The typed-let usage
> stretches `count` to mean "any value with population dimension," which
> contradicts the kind's stated domain. The reviewer's split (`count` integer /
> `population` real with dim P) is the right structural axis. Confirmed.

## 5. Probabilistic branching is mathematically ambiguous

**Location** — §9.1.2

**Category** — statistical correctness; user footgun

**Defect** — The construct is called "probabilistic branching," and branch
weights are said to have probability dimension `[0,1]`, but the compiler "does
not enforce that weights sum to 1" and allows "rate-weighted branches."

**Why it matters** — For:

```camdl
infection : S --> { A : p, B : q } @ r
```

if `p + q = 0.8`, the total depletion rate from `S` becomes `0.8r`, not `r`.
That is no longer "one event, multiple outcomes"; it is silently changing the
total event rate.

**Fix** — Separate the two concepts:

```camdl
# true branching: weights must sum to 1
infection : S --> branch { A : p, B : 1 - p } @ r

# rate splitting: each destination has its own rate contribution
infection : S --> rates { A : rA, B : rB }
```

For `branch`, enforce `sum(weights) = 1` when statically provable. When weights
depend on parameters, runtime validation must reject invalid values or assign
`-inf` likelihood during inference.

**Severity** — Critical

> **Verification (2026-05-26):** §9.1.2 reads verbatim:
>
>> "The weight of each branch is any scalar expression with dimension
>> `probability` (dimensionless, domain `[0, 1]`). The compiler does not enforce
>> that weights sum to 1 — users can write rate-weighted branches where the sum
>> differs from 1 (e.g., for a fraction of events going to an 'other'
>> compartment that's implicit)."
>
> The construct name (probabilistic branching) and the documented escape hatch
> (rate-weighted, sum may differ) are in direct tension. Confirmed.

## 6. Observation time-window semantics are not defined

**Location** — §12–13, §16–17, §22.5

**Category** — statistical correctness; user footgun

**Defect** — `incidence(transition)` is described as "cumulative flow since last
observation," but the spec does not define the interval convention. It does not
say whether an observation at time `t` covers `[t-every, t]`, `(t-every, t]`,
`[t, t+every)`, or the interval since the previous observed row. It also does
not define first-observation behavior, missing observations, irregular
observation times, or how data timestamps align with output schedules.

**Why it matters** — Weekly AFP cases, campaign windows, and incidence curves
are interval data. A one-bin shift changes inferred reporting rates,
intervention effects, and epidemic timing.

**Fix** — Make observation windows first-class:

```camdl
observations {
  weekly_cases : {
    time_meaning = interval_end       # or interval_start / instant
    window       = previous_to_current
    projected    = incidence(infection)
    likelihood   = neg_binomial(mean = rho * projected, r = k)
  }
}
```

Define exact inclusivity:

```text
interval_end at t means events in (previous_t, t]
first observation uses (t - every, t] unless data supplies previous_t
```

For irregular data, the data file should drive the interval boundaries, not
`every`.

**Severity** — Critical

> **Verification (2026-05-26):** §12 (Observations) shows `every = 7 'days` with
> `projected = incidence(transition)` described only as "cumulative flow since
> last observation." Half-open vs closed interval, first-observation behavior,
> and data-driven irregular boundaries are all undefined. Confirmed.

## 7. The data-observation contract is missing

**Location** — §12–13, §21–22

**Category** — statistical correctness; bad UX

**Defect** — The model-level `observations {}` block defines streams,
projections, and likelihoods, but the CLI still documents `--flow` and
`--obs-model`, which bypass that model-level observation contract. The spec also
does not define the observation data file schema for multiple streams.

**Why it matters** — A modeler can define `weekly_cases` in the DSL and then fit
with `--flow recovery --obs-model discretized_normal`, accidentally using a
different observation model. That makes the `.camdl` review meaningless for
inference.

**Fix** — Make the model observation block authoritative. Inference commands
should accept stream selection, not model replacement:

```bash
camdl pfilter model.camdl --params p.toml --data obs.tsv --stream weekly_cases
```

Define the canonical data schema:

```text
time    stream          observed    n_tested    ...
7       weekly_cases    12          .
14      weekly_cases    19          .
7       serology        41          200
```

Legacy `--flow` / `--obs-model` should be marked as low-level deprecated
commands or removed.

**Severity** — Critical → **High** (split status; see Verification)

> **Verification (2026-05-26, revised 2026-05-27).** **Status: mixed.**
>
> *Implementation bypass:* **resolved-in-commit (independent of this review).**
> Grep of current `rust/crates/cli/src/args/mod.rs` for `--flow` /
> `--obs-model` returns zero hits — these flags were removed in the
> 2026-05-25 CLI UX revision. The live failure mode the reviewer warned
> about no longer exists.
>
> *Spec stale CLI text:* **resolved-in-commit `1a7ba112`.** The pfilter / if2
> / profile examples in §21.5 used to show `--flow recovery --obs-model
> discretized_normal`; the cheap-batch commit removed those flags from the
> worked examples and added a paragraph saying the projection and likelihood
> come from the model's `observations { ... }` block. Verified against
> current spec: grep `'--flow' docs/camdl-language-spec.md` finds only the
> "legacy `--flow` / `--obs-model` flags were removed" disclaimer text.
>
> *Canonical multi-stream observation data schema:* **confirmed-open.** The
> spec still doesn't define the `time / stream / observed / n_tested / ...`
> data file schema the reviewer described. Stream selection via `--stream
> NAME` is documented in §21.5 but the file-format contract isn't. This is
> the real remaining issue.
>
> Severity reflects the canonical-schema gap (High); the live-bypass and
> stale-CLI-text concerns are both addressed.

## 8. Tables with repeated dimensions need axis names

**Location** — §6, §9.7, §23, §25–26

**Category** — type design; user footgun

**Defect** — A table can be declared as `patch × patch`, but both axes have the
same dimension name. Named indexing cannot distinguish source patch from
destination patch, so examples rely on positional indexing:

```camdl
distance[i,j]
mig[dst,src]
```

File loading also uses positional mapping and treats column names as
human-readable only.

**Why it matters** — Spatial kernels and migration matrices are exactly where
source/destination swaps are common and hard to detect. A `src,dst,value` file
read as `dst,src,value` can reverse migration or infection importation with no
type error.

**Fix** — Table axes need names distinct from their dimension type:

```camdl
tables {
  distance : src:patch × dst:patch 'km = read(
    "data/lga_dist.tsv",
    columns = [src, dst, distance]
  )
}
```

Lookup should support:

```camdl
distance[src = i, dst = j]
```

For repeated dimensions, unnamed positional axes should be rejected.

**Severity** — Critical

> **Verification (2026-05-26):** §6.1 confirms: "`C_age[i, j]` requires both
> `i : age` and `j : age`. Using `C_age[i, s]` where `s : sex` is a compile
> error." So the _dimension type_ is checked, but for repeated dimensions both
> axes hold the same type — positional swap is invisible. §6.2 confirms "Column
> names in the file are for human readability — the compiler uses positional
> mapping from the type signature." For `src,dst,value` files this makes
> source/destination swaps undetectable. Confirmed.

## 9. Expanded names are not a safe semantic representation

**Location** — §4.3, §5, §18.1, §25–26

**Category** — FFI; type design; user footgun

**Defect** — The spec repeatedly relies on mangled names such as `N_urban`,
`infection_child`, and `S_child_p1`. It does not define escaping or collision
rules, yet examples use level names with underscores such as `kano_dala` and
`borno_maiduguri`.

**Why it matters** — String mangling is not reversible. These collide:

```text
S[a_b, c]  -> S_a_b_c
S[a, b_c]  -> S_a_b_c
```

The runtime, inference output, observation projections, and scenario patches
cannot safely recover dimension coordinates from names. This is already the kind
of design that leads to prefix/suffix bugs.

**Fix** — IR must carry IDs and coordinate metadata:

```json
{
  "compartment_id": 42,
  "base": "S",
  "coords": { "age": "child", "patch": "kano_dala" },
  "display_name": "S[age=child, patch=kano_dala]"
}
```

Flat names may exist only as display strings. Projection, scenario, and
inference code must use IDs, not parsed names.

**Severity** — Critical

> **Verification (2026-05-29, supersedes 2026-05-26):**
> `confirmed-code-protected-but-spec-stale`. The *encoding* is non-injective as
> the reviewer says — §4.3 says "the compiler always mangles to `N0_urban` in the
> IR", and level names like `kano_dala` / `borno_maiduguri` contain underscores.
> **But the collision is not a silent footgun: the compiler rejects it at compile
> time.** A model with `d1 = [a, a_b]`, `d2 = [c, b_c]` — so `S[a, b_c]` and
> `S[a_b, c]` both mangle to `S_a_b_c` — fails to compile:
>
> ```
> error[E500]: duplicate compartment after expansion: 'S_a_b_c'
>   = hint: stratification produced two compartments with the same name
> error[E501]: duplicate transition after expansion: 'decay_a_b_c'
>   = hint: stratification produced two transitions with the same name
> ```
>
> Source: the post-expansion uniqueness check `ocaml/lib/ir/validate.ml:73`
> (`uniq_check` over expanded compartment/transition names), rendered as E500/E501
> with the collision hint in `ocaml/lib/compiler/compiler.ml:99-106`. Repro:
> `camdlc` on a 2-dimension model whose level names collide under `_`. Because no
> two cells can share a name in a *compilable* model, the downstream "observation
> projections / scenario patches bind the wrong cell" failure mode the reviewer
> lists **cannot occur silently** — it is a hard error first.
>
> Residual (why "spec-stale", not refuted): E500/E501 guarantees *uniqueness*, not
> *reversibility* — `S_a_b_c` is still not self-describing (recovering coordinates
> needs the dimension registry), and §4.3 documents mangling with no escaping
> rules. The reviewer's "carry IDs + coords in the IR" recommendation remains a
> legitimate robustness/spec-hygiene improvement, but it is **not Critical and not
> a silent-collision bug** — downgrade severity accordingly. Overlaps
> `docs/dev/proposals/2026-05-26-typed-indexed-reference-resolver.md` (compiler-side).

## 10. Math functions silently repair invalid values

**Location** — §9.7

**Category** — numerical correctness; user footgun

**Defect** — The spec defines:

```text
log(x)  returns -∞ for x ≤ 0
sqrt(x) returns 0 for x < 0
mod(a,b) returns 0 for b = 0
```

These are silent domain repairs.

**Why it matters** — `sqrt(negative)` and `mod(a,0)` are model errors or invalid
parameter proposals. Turning them into 0 can shut off a rate, create artificial
thresholds, or make invalid particles look plausible. This is exactly how
numerical plumbing bugs become scientific conclusions.

**Fix** — Define a strict domain policy:

```text
Compile-time invalid constant expression -> hard compiler error.
Runtime invalid expression in simulation -> SimError with name/time/context.
Runtime invalid expression in particle inference -> particle gets -inf weight if proposal-dependent; setup error if structural.
```

Do not clamp `sqrt`, `log`, or `mod`.

**Severity** — Critical

> **Verification (2026-05-26):** §9.7 expression-grammar table confirms each
> clamp verbatim: `log(x)` returns `-∞` for `x ≤ 0`; `sqrt(x)` returns 0 for
> `x < 0`; `mod(a, b)` returns 0 for `b = 0`. Confirmed.

## 11. Boolean expressions are not typed

**Location** — §9.3, §9.7, §17.3

**Category** — type design; user footgun

**Defect** — Comparisons exist, `if` conditions exist, guards use `and/or`, and
examples multiply comparisons:

```camdl
let is_pulse = (day_of_year > 250.0) * (day_of_year < 252.0)
```

But the spec never defines a Boolean type, whether comparisons return Booleans
or 0/1 numbers, or whether Booleans can be multiplied.

**Why it matters** — Boolean-as-number is convenient but dangerous. A modeler
can accidentally use a predicate as a rate multiplier without realizing the
discontinuity. It also affects typechecking of `if`, `time_when`, guards, and
summary predicates.

**Fix** — Add a real Boolean type. Comparisons return `Bool`. `if` requires
`Bool`. Arithmetic on `Bool` is illegal. If indicator functions are desired,
make them explicit:

```camdl
indicator(day_of_year > 250 and day_of_year < 252)
```

Also fix the IR example that says `if I > 0 ...` becomes `Cond(Pop("I"), ...)`;
it should become `Cond(Gt(Pop("I"), Const(0)), ...)`.

**Severity** — Critical

> **Verification (2026-05-26, revised 2026-05-27).** **Status: confirmed-open.**
> The `is_pulse` example exists at line 1575:
> `let is_pulse = (day_of_year > 250.0) * (day_of_year < 252.0)` — using
> comparison results as numeric 0/1 multipliers. No Boolean type is defined.
>
> The IR sub-claim is also confirmed against the current spec. At line 1647:
>
> ```
>   @ if I > 0 then beta * S * I / N else 0.0
>
>   This becomes `Cond(Pop("I"), <rate_expr>, Const(0.0))` in the IR.
> ```
>
> The condition `I > 0` lowers to `Pop("I")` in the example, dropping the
> `Gt(_, Const(0.0))` comparison wrapper entirely. The reviewer's recommended
> form is `Cond(Gt(Pop("I"), Const(0.0)), <rate_expr>, Const(0.0))`. The
> earlier assessment said the phrase wasn't in the spec — that was wrong
> (the grep pattern I used was too literal and missed the actual line). Both
> the broader F11 finding (no Boolean type) and the specific IR-example
> sub-claim are confirmed.

## 12. Overdispersion is parameterized inconsistently

**Location** — §9.7, §9.8

**Category** — statistical correctness

**Defect** — The spec says `σ²_SE` is "the variance of the Gamma noise
multiplier," but the wrapper table says:

```text
Var = mean + mean² · σ² / dt
```

Both cannot be true. If the multiplier variance is `σ²`, the count variance is
`mean + mean²σ²`. If the multiplier variance is `σ²/dt`, the table formula is
correct.

**Why it matters** — This changes the meaning of the overdispersion parameter
and therefore the posterior for extra-demographic stochasticity. A model
calibrated under one interpretation cannot be compared to a model simulated
under the other.

**Fix** — Define one parameterization explicitly. For example:

```text
overdispersed(rate, sigma2):
  On a step of length dt, draw Gamma multiplier G with E[G]=1 and Var[G]=sigma2/dt.
  Then draw events conditional on G.
```

Then define source-bounded semantics separately:

```text
Inflows: Gamma-Poisson / negative binomial.
Single-source outflows: Gamma-noised competing hazard, then bounded multinomial draw.
Multi-source reactions: backend-specific; unsupported unless exact law is defined.
```

**Severity** — Critical

> **Verification (2026-05-26, revised 2026-05-27).** **Status: confirmed-open
> (text-fix-pending).** §9.8 prose (line 1635): "σ²_SE, the variance of the
> Gamma noise multiplier (which has mean 1)". §9.7 wrapper table (line 1585):
> `Var = mean + mean² · σ² / dt`. Under the literal reading these are
> inconsistent: if `Var[G] = σ²` per prose, then under Gamma-Poisson
> `Var[events] = mean + mean²·σ²` with no `/dt`; the table form requires
> `Var[G] = σ²/dt`.
>
> The earlier assessment tried to reconcile these by treating `σ²` as a
> *parameter name* and `σ²/dt` as the actual step-level Var[G]. That
> reconciliation is mathematically possible but it is not what the prose
> currently says — and the reviewer's complaint that the prose is wrong is
> fair. **Earlier "sub-claim refuted" label was wrong.** Reframe as: the
> intended formula is probably "σ²_SE is the per-unit-time overdispersion
> parameter; on a step of length dt the Gamma multiplier has Var[G] = σ²_SE
> / dt". Fix the §9.8 prose to say that explicitly. Not a refutation of any
> sub-claim — it is a real wording-precision defect with statistical
> consequences.

## 13. Events and interventions allow silent invalid state changes

**Location** — §13–14.8

**Category** — numerical correctness; user footgun

**Defect** — `add` accepts negative values and only warns if the result makes a
compartment negative. `transfer(fraction=...)` does not explicitly require the
fraction to be finite and in `[0,1]`. `transfer(count=...)` does not define
count domain or rounding. `balance` mentions "clamps" even though silent
non-negativity clamps are not otherwise specified.

**Why it matters** — State modification is how vaccination, importation, cohort
entry, and campaign scenarios are represented. A negative campaign count,
fraction greater than 1, or silent clamp can turn an intervention counterfactual
into a different policy.

**Fix** — Make action domains strict:

```text
transfer fraction: finite, 0 ≤ f ≤ 1
transfer count: finite, integer-compatible, nonnegative
add count: finite, integer-compatible; negative only with explicit allow_negative = true
set integer compartment: finite, integer-compatible, nonnegative
```

If an event would make an integer compartment negative, that is a hard
simulation error or a dead particle in inference. No silent clamp.

**Severity** — Critical

> **Verification (2026-05-26):** §13.1 documents
> `transfer(fraction = EXPR, ...)` with no domain bound on `EXPR` and
> `transfer(count = EXPR, ...)` with no integer/non-negative bound. The runtime
> side has the same issue — GH #99 (event-action validation gaps, filed today)
> overlaps directly. Both sides need tightening together. Confirmed.

## 14. Backend compatibility is underspecified for time-dependent rates

**Location** — §7, §9.8, §11, §18, §21–22

**Category** — statistical correctness; numerical correctness

**Defect** — The language encourages time-dependent forcing, interventions,
events, and real-compartment ODEs, while the CLI defaults to `gillespie`. The
spec only says Gillespie rejects `overdispersed`; it does not say whether
Gillespie is exact, approximate, or illegal for `t`, forcing functions,
interventions, or real-valued ODE coupling.

**Why it matters** — Standard direct-method SSA is exact only when propensities
are constant between events. Seasonal forcing and ODE-coupled hazards violate
that. A default backend that is exact for one model class and approximate for
another needs explicit acceptance rules.

**Fix** — Add a backend capability table:

```text
Gillespie exact:
  allowed only for time-homogeneous rates with no real-state dependency,
  unless nonhomogeneous SSA/thinning is implemented.

Tau-leap:
  approximate fixed-step; requires dt and boundary splitting.

Chain-binomial:
  source-bounded fixed-step; only for single-source transitions unless multi-source law defined.

ODE:
  deterministic only; rejects stochastic wrappers.
```

The compiler/runtime must reject unsupported combinations before simulation.

**Severity** — Critical

> **Verification (2026-05-26):** §9.8 says Gillespie rejects `overdispersed` but
> the spec never enumerates which backends accept time-dependent rates, forcing
> functions, intervention firings, or real-compartment-coupled ODE hazards.
> Overlaps directly with engine-side issues #95 (Gillespie
> inhomogeneous-Poisson) and #120 (chain-binomial real state), both filed today.
> Confirmed.

## 15. Content-addressable output omits simulation-defining inputs

**Location** — §19–20

**Category** — reproducibility; not wired through

**Defect** — `model_hash` excludes `simulate`, and `sim_hash` includes model
hash, base params, backend, dt, and tool version. It does not include
`simulate.from`, `simulate.to`, output schedules, observation/synthetic-output
schedules, enabled event schedules, or scenario-level `simulate.to`. The spec
also says `sha256("")` has prefix `00000000`, which is false unless
special-cased.

**Why it matters** — A 2-year run and a 5-year run can reuse the same cached
directory. That is not a cache miss; it is a wrong result served from cache.

**Fix** — Define separate hashes:

```text
model_hash = structural model only
run_hash   = model_hash + params + backend + dt + simulate window + output schedules + tool version
scen_hash  = resolved scenario delta, including enable/disable/set/scale and scenario simulation overrides
fit_hash   = run_hash + priors + transforms + data hash + inference config
```

If baseline should display as `00000000`, define it as a literal special case,
not `sha256("")`.

**Severity** — Critical

> **Verification (2026-05-26):** §19.1 explicitly excludes `simulate` from
> `model_hash` and `sim_hash`. A 2-year run and a 5-year run on the same model
> reuse the same `sim_hash` directory. The sub-claim about `sha256("")` was
> verified factually:
>
> ```
> $ echo -n "" | shasum -a 256
> e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855  -
> ```
>
> The actual SHA-256 of empty input starts with `e3b0c442`, not `00000000`. The
> `00000000` prefix in the spec example is a special-case display value, not a
> hash output. Spec was clarified in commit `1a7ba112` to say so. The main "hash
> omits simulation window" finding still stands. Confirmed.

# High findings

## 16. Forcing syntax contradicts itself

**Location** — §7, §23 full example, §27 primitive summary

**Category** — spec consistency; user footgun

**Defect** — The spec says every forcing declaration must carry a tier-3 unit
literal:

```camdl
seasonal : sinusoidal 'ratio { ... }
```

But multiple examples use the older syntax:

```camdl
seasonal = sinusoidal(...)
reporting_dow = periodic(...)
pop_trend : interpolated { ... }
```

The primitive summary also omits the required unit literal.

**Why it matters** — Users copy examples. In this spec, copied examples are
syntax errors or, worse, accepted by an old parser with wrong dimensional
inference.

**Fix** — Delete the old syntax everywhere. The full example should say:

```camdl
forcing {
  seasonal : sinusoidal 'ratio {
    amplitude = alpha
    period    = 365.25 'days
    phase     = phi_season
    baseline  = 1.0
  }
}
```

If call syntax is retained, it still needs the unit:

```camdl
seasonal : sinusoidal 'ratio = sinusoidal(...)
```

Pick one form.

**Severity** — High → **resolved**

> **Verification (2026-05-26, revised 2026-05-27).** **Status: resolved-in-commit
> `1a7ba112`, verified against current spec file.** §7 had both forms;
> the OCaml parser at `parser.mly:319` accepts only `name : KIND 'unit { … }`.
> Rewrote every forcing example in §7 and §23 to the colon-block form.
> Current-spec verification: `grep -n "= sinusoidal(\|= periodic(\|= piecewise(\|=
> interpolated(" docs/camdl-language-spec.md` returns zero hits — the
> old-syntax examples are gone.

## 17. Unit literals are inconsistently defined

**Location** — §2.1, §2.3, §2.4, §7, §16, §23

**Category** — type design; user footgun

**Defect** — §2.1 says supported units are only time and rate units. Later
sections use `'ratio`, `'count`, and singular `'day`. The text also mentions
invalid parameter syntax such as `beta : rate 'per_month`, even though the spec
says unit literals cannot be applied to unknown parameter values.

**Why it matters** — Unit syntax is the primary defense against per-day/per-week
and count/fraction mistakes. If the unit grammar is inconsistent, users will
either avoid units or rely on examples that do not parse.

**Fix** — Define one unit grammar:

```text
time_unit_decl_unit = 'days | 'weeks | 'months | 'years
duration_unit       = 'days | 'weeks | 'months | 'years
rate_unit           = 'per_day | 'per_week | 'per_month | 'per_year
value_unit          = duration_unit | rate_unit | 'count | 'ratio
```

Then update all examples. Either accept singular aliases like `'day`, or remove
them.

**Severity** — High → **resolved**

> **Verification (2026-05-26, revised 2026-05-27).** **Status: resolved-in-commit
> `1a7ba112`, verified against current spec file.** §2.1 list was missing
> `'count` and `'ratio`; one occurrence of singular `'day`. Added the two
> units with their dimensions and corrected the singular. Current-spec
> verification:
>
> ```
> $ sed -n '113,115p' docs/camdl-language-spec.md
> Supported units: `'days`, `'weeks`, `'months`, `'years`, `'per_day`,
> `'per_week`, `'per_month`, `'per_year`, `'count`, `'ratio`.
> `'count` carries dimension P (population); `'ratio` is dimensionless.
> $ grep -n "'day\b" docs/camdl-language-spec.md | grep -v "'days\|'day_"
> (no output — singular 'day fully removed)
> ```

## 18. Several worked examples are dimensionally wrong

**Location** — §6, §7, §9.1, §23

**Category** — numerical correctness; user footgun

**Defect** — Examples contain dimension errors or dangerous defaults:

- `C_age : age × age 'per_day` combined with `beta : rate` gives `P·T⁻²` in
  infection rates.
- `import_rate : rate` used as an inflow allocation gives `T⁻¹`, not `P·T⁻¹`.
- `pop : patch = read(...)` omits `'count`, making population tables
  dimensionless.
- `distance : patch × patch = read(..., default = 0.0)` is then used in a
  denominator.
- The full example uses old forcing syntax without the required unit literal.

**Why it matters** — Worked examples are de facto tests and tutorials. If they
do not typecheck under the spec, users learn the wrong modeling patterns.

**Fix** — Make all examples compile-clean under the stated dimensional checker.
For example:

```camdl
C_age : contact_age × participant_age = [[...]]      # dimensionless
beta  : rate

import_rate : positive [P/T]

pop : patch 'count = read("data/lga_pop.tsv")

distance : src:patch × dst:patch 'km = read("data/lga_dist.tsv")
```

Do not use `default = 0.0` for denominator tables.

**Severity** — High

> **Verification (2026-05-26, revised 2026-05-27).** **Status: confirmed-open.**
> The forcing-syntax sub-bullet was confirmed and resolved in commit
> `1a7ba112` alongside F16 (line-cited check below in F16's verification
> block). The individual dimension-error sub-bullets are each derivable from
> the spec's own dimensional rules without a compiler run:
>
> - `C_age : age × age 'per_day` combined with `beta : rate * S * sum(b in age,
>   C_age[a,b] * I[b] / N[b])` gives an extra `T⁻¹` factor — the infection
>   rate dimension comes out `P·T⁻²` instead of the `P·T⁻¹` E300 requires.
> - `import_rate : rate` used as `@ import_rate * pop[p] / sum(q in patch,
>   pop[q])` — `pop[p] / sum(pop)` is dimensionless, so the result is `T⁻¹`,
>   not the `P·T⁻¹` an inflow needs.
> - `pop : patch = read(...)` (omitting `'count`) leaves the table cells
>   dimensionless under §6.1's tier-3 unit rule.
> - `distance : patch × patch = read(..., default = 0.0)` used in a kernel
>   denominator creates a divide-by-zero hazard.
>
> Earlier "partial confirmation" label was too soft — each of these is
> derivable from the spec's dimensional rules alone; compiler tests would
> confirm but aren't required to know they're wrong. Per-bullet remediation
> still needs to wait for F8 (axis-named tables) and F4 (`count` vs
> `population` kind split) proposals because the right *fix* depends on
> those, but the *finding* is confirmed-open at High severity now.

## 19. Indexed parameters are single-dimensional in one section and multi-dimensional in another

**Location** — §4.3, §18.1

**Category** — type design; spec consistency

**Defect** — §4.3 says parameters may be declared with a single dimension index:

```camdl
N[patch] : positive
```

but §18.1 says multi-dimensional indexed parameters are supported:

```camdl
amp[urban, child]
```

**Why it matters** — Age × patch parameters are common: reporting rates,
coverage, importation weights, initial conditions, and covariates. The spec
gives users no clear answer about whether these are legal.

**Fix** — Support arbitrary dimension vectors:

```camdl
parameters {
  rho[age, patch] : probability
}
```

Then define named indexing, runtime `--param-vec` file format, scenario
`set`/`scale`, and IR coordinate metadata for multi-dimensional parameters. If
v0.3 only supports one dimension, delete the multi-dimensional claims.

**Severity** — High

> **Verification (2026-05-26):** §4.3: "Parameters may be declared with a single
> dimension index". The OCaml parser at `parser.mly:174` confirms —
> `PIndexed { pdims = [dim]; ... }` is a single-element list. §17.1 (renumbered
> from §18.1) shows `amp[urban, child]` mangling to `amp_urban_child`, which is
> only meaningful if `amp` is multi-dim — but multi-dim parameter declarations
> don't parse. §4.3 and §17.1 contradict. Confirmed.

## 20. CSV/TSV table loading by position is too error-prone

**Location** — §6.2–6.4

**Category** — user footgun; scientific correctness

**Defect** — Table files use positional mapping from the table type signature,
and column names are "for human readability." Multi-value columns also force all
output tables to share one unit annotation.

**Why it matters** — Public-health spatial data often has columns like `src`,
`dst`, `distance`, `population`, `coverage`. Positional mapping makes a
source/destination swap invisible. Multi-value files commonly mix units, such as
`pop` as count and `init_sus` as ratio.

**Fix** — Require explicit column mapping for file tables:

```camdl
pop : patch 'count =
  read("demographics.tsv", columns = [patch, pop])

distance : src:patch × dst:patch 'km =
  read("dist.tsv", columns = [src, dst, distance])
```

For multi-value files, require per-value units:

```camdl
tables {
  (pop 'count, init_sus 'ratio) : patch =
    read("demographics.tsv", columns = [patch, pop, init_sus])
}
```

or require separate declarations.

**Severity** — High

> **Verification (2026-05-26):** Same root cause as F8. §6.2 says "Column names
> in the file are for human readability — the compiler uses positional mapping
> from the type signature." Confirmed.

## 21. Dimension levels from files need a string/identifier policy

**Location** — §5, §6.3, §9.7, §25–26

**Category** — UX; FFI; type design

**Defect** — Data-derived levels are actual file values, but expression syntax
only shows identifier-like levels. Real geographic labels often contain spaces,
hyphens, leading digits, slashes, or underscores.

**Why it matters** — Nigeria LGA names and administrative codes are not
guaranteed to be valid identifiers. If the compiler canonicalizes them, users
need to know the mapping. If it does not, many real datasets cannot be indexed
directly.

**Fix** — Treat dimension levels as strings internally and support quoted
indexing:

```camdl
S[patch = "Kano Dala"]
S[patch = "001"]
```

For display names, preserve original labels. For identifiers, allow optional
aliases:

```camdl
dimensions {
  patch = read("lga.tsv", column = "lga_name", alias_column = "lga_id")
}
```

**Severity** — High

> **Verification (2026-05-26):** Spec uses identifier-like level names
> (`kano_dala`, `borno_maiduguri`) throughout but does not address what happens
> when data-derived levels contain spaces, hyphens, leading digits, or slashes.
> Real LGA names and administrative codes routinely fail identifier rules.
> Confirmed.

## 22. Scenario semantics are too powerful for the model file

**Location** — §17–18

**Category** — bad abstraction; reproducibility; user footgun

**Defect** — Scenario blocks can set and scale parameters, inherit from other
scenarios, alter `simulate.to`, enable interventions, and compose patches. Some
examples put baseline parameter values inside scenario definitions.

**Why it matters** — A model file can contain a hidden calibrated baseline and
counterfactual values. Reviewers looking only for structural model errors may
miss that the scenario block changes transmission, coverage, or horizon.

**Fix** — Keep intervention declarations in `.camdl`, but move scenario patches
to experiment/run files. If embedded scenarios remain, mark them as analysis
configuration and include them in the relevant run hash. Also reserve `baseline`
as the identity scenario; do not allow a user-defined `baseline` block to mutate
parameters.

**Severity** — High

> **Verification (2026-05-26):** Overlaps with F1. Scenarios can `set` / `scale`
> parameter values, alter `simulate.to`, enable interventions. The canonical
> golden `sirv_anchored_calendar.camdl` ships embedded baseline parameter values
> inside scenarios. Confirmed.

## 23. Scenario `scale` cannot be validated at compile time as claimed

**Location** — §18.1–18.3

**Category** — not wired through; user footgun

**Defect** — The spec says `scale` on a probability parameter that would exceed
`[0,1]` is a compile error. But parameter values are external, and scenario RHS
expressions can reference current parameter values, so the compiler often cannot
know the result.

**Why it matters** — `scale = { rho = 1.5 }` may be valid if `rho = 0.4` and
invalid if `rho = 0.8`. This must be checked against the actual runtime
parameter environment.

**Fix** — Replace "compile error" with a two-phase rule:

```text
Compile time: validate target parameter exists and scale expression is parameter-only.
Runtime: after applying params and scenario patch, validate all parameter domains and bounds.
```

If all values are literal and statically known, the compiler may reject early as
an optimization.

**Severity** — High

> **Verification (2026-05-26):** §17.1 (renumbered from §18.1) reads: "`scale`
> on a `probability` parameter that would exceed `[0,1]` is a **compile
> error**." §17.3 allows scale expressions to reference current parameter
> values, which makes the result unknowable at compile time. Promise
> unattainable as stated. Confirmed.

## 24. Initial conditions defaulting to zero is a footgun

**Location** — §15

**Category** — user footgun; scientific correctness

**Defect** — Unlisted compartments and unlisted stratum combinations default
to 0.

**Why it matters** — In a 774-patch model, an omitted init entry can erase an
entire region's population. The model still runs and may produce zero incidence
in omitted areas, which looks like epidemiology but is just missing
initialization.

**Fix** — Require explicit default policy:

```camdl
init {
  default = error
  S[p in patch] = pop[p] - I0[p]
  I[p in patch] = I0[p]
}
```

For sparse seeding, make the zero default explicit:

```camdl
init {
  default = 0
  S[patch = "kano_dala", age = child] = 100000
  I[patch = "kano_dala", age = child] = I0
}
```

`camdl check` should report how many expanded compartments start at zero.

**Severity** — High

> **Verification (2026-05-26):** §15.2 (renumbered from §16.2) is explicit:
> "Unlisted stratum combinations default to 0. For a 774-patch model, only the
> patches mentioned in init are nonzero — the rest start empty." Confirmed.

## 25. Schedule boundary semantics are not robust enough

**Location** — §13–14.7, §16–17

**Category** — numerical correctness; user footgun

**Defect** — `at_day` fires when `|t - target| < 0.5 * dt`. Output, observation,
intervention, and event schedules are described separately rather than as a
single boundary system.

**Why it matters** — Timed vaccination, importation pulses, reporting windows,
and output snapshots should not depend on floating-point tolerance or whether
`dt` lands near a target. A campaign can fire one step early, one step late, or
not at all.

**Fix** — Define all schedules by interval crossing:

```text
An event at time τ fires exactly once when the integrator advances from t_old < τ ≤ t_new.
Backends must split steps at event, observation, output, and simulation-end boundaries,
or reject schedules not aligned with dt.
```

Do not use tolerance-based firing as normative semantics.

**Severity** — High

> **Verification (2026-05-26):** §13.7 (renumbered from §14.7) reads verbatim:
> "The engine fires on the single timestep where `|t - target| < 0.5 * dt`,
> guaranteeing exactly one fire per period regardless of `dt` or
> fractional-period drift." This IS tolerance-based firing. The reviewer's
> interval-crossing alternative is more robust. Confirmed.

## 26. External table loading contradicts compile-time inlining

**Location** — §6.5, §22.2

**Category** — reproducibility; FFI

**Defect** — §6.5 says external tables are loaded at compile time and inlined
into the IR. The CLI later documents:

```bash
--table NAME=FILE  supply a runtime external() table
```

but the language has no `external()` table syntax and the hash rules do not
cover runtime table contents.

**Why it matters** — Table data can be transmission kernels, populations,
schedules, and contact matrices. If table values can change at runtime without
changing model hash or IR, reproducibility is broken.

**Fix** — Pick one design:

1. **Compile-time only:** remove `--table`; all table data is in IR and model
   hash.
2. **Runtime external tables:** add explicit syntax:

   ```camdl
   kernel : src:patch × dst:patch external 'ratio
   ```

   Then include table file content hash in `run_hash`.

**Severity** — High → **Medium** (split status; see Verification)

> **Verification (2026-05-26, revised 2026-05-27).** **Status: mixed.**
>
> *Implementation gap:* **resolved-in-commit (independent of this review).**
> Grep for `--table` flag in `rust/crates/cli/src/args/mod.rs` returns
> nothing; grep for `external()` table syntax in `parser.mly` returns
> nothing. The runtime-external-tables bypass the reviewer warned about
> does not exist in current code.
>
> *Spec stale CLI text:* **was confirmed-open in the previous assessment.**
> Spec §21.2 listed `--table NAME=FILE supply a runtime external() table`
> at line 2987 even though no such flag exists. The earlier "downgrade to
> Medium because no implementation gap exists today" verdict was too
> eager — the reviewer correctly noted that as long as the spec advertised
> `--table`, users could still encounter the contradiction. **The stale
> line is now deleted in this commit** (verified: `grep -n '--table'
> docs/camdl-language-spec.md` returns no hits in the §21.2 CLI flag table).
>
> *Hash inputs for runtime external tables:* per the F15 finding, the hash
> design doesn't cover runtime external table content. Since the runtime
> mechanism doesn't exist, this is moot for now — but if external tables
> are reintroduced (one of the reviewer's "Pick one design" options),
> `run_hash` must include the file content hash.

# Medium findings

## 27. The grammar omits advertised blocks

**Location** — §14.5–14.8, §25.1, §27 primitive summary

**Category** — spec consistency; tests

**Defect** — `events {}` and `balance {}` are documented, but the file-level
grammar does not list `events_block` or `balance_block`. The primitive summary
also omits them.

**Why it matters** — Implementers will disagree on whether these are real
language constructs or prose-only future features.

**Fix** — Add them to the grammar if supported. If not supported, move them to a
future section and require the compiler to reject them.

**Severity** — Medium → **resolved**

> **Verification (2026-05-26, revised 2026-05-27).** **Status: resolved-in-commit
> `1a7ba112`, verified against current spec file.** §24.1 grammar listing
> was missing `events_block` and `balance_block` even though the OCaml
> parser supports both (verified at `parser.mly:90-100`). Added both
> productions. Current-spec verification:
>
> ```
> $ grep -nA1 "events_block\|balance_block" docs/camdl-language-spec.md
> 3730:  | events_block                      # events { ... }
> 3737:  | balance_block                     # balance { ... }
> ```

## 28. "Parsed but discarded" features should not exist

**Location** — §11, §13.4, §14 Timepoints

**Category** — user footgun; spec consistency

**Defect** — The spec says the `ode {}` block and `timepoints {}` block are
parsed but discarded or unavailable. It also contains stale status text for
observations.

**Why it matters** — Accepted-but-ignored syntax is worse than rejected syntax.
A modeler can think they defined an ODE or timepoint while the model runs
without it.

**Fix** — For every not-yet-implemented feature, choose one:

```text
Reject at parse time with "feature not implemented".
```

or

```text
Fully specify and implement it.
```

Do not parse and discard.

**Severity** — Medium

> **Verification (2026-05-26):** §11 explicitly: "Not yet implemented (v0.2).
> The `ode { }` block is parsed but currently discarded by the expander."
> Documented transparently but the user-visible failure mode (write an ODE,
> watch it get ignored) is real. Confirmed.

## 29. Reserved identifiers are incomplete

**Location** — §2.3, §9.7, §12–14, §15.2, §27 primitive summary

**Category** — type design; user footgun

**Defect** — The reserved list omits names that are treated specially elsewhere:
`t`, `origin`, `projected`, `date`, `add_calendar_months`, `add_calendar_years`,
`date_range`, `unchecked_dim`, `overdispersed`, `deterministic`, likelihood
family names, `baseline`, and `scenario`.

**Why it matters** — A user-defined parameter or table named `projected`,
`date`, or `baseline` can make expressions ambiguous or context-dependent.

**Fix** — Define reserved names by namespace and context:

```text
Global reserved: t, origin, t_start, t_end, sum, consecutive, compartments, date, ...
Observation-likelihood reserved: projected
Experiment-summary reserved: baseline, scenario
Function reserved: exp, log, sqrt, ...
```

Then reject user declarations that collide.

**Severity** — Medium → **resolved**

> **Verification (2026-05-26, revised 2026-05-27).** **Status: resolved-in-commit
> `1a7ba112`, verified against current spec file.** §14.2 list was 5 names
> (`t_start`, `t_end`, `compartments`, `sum`, `consecutive`); expanded to
> cover `t`, `dt`, `origin`, calendar builtins (`date`, `add_calendar_*`,
> `date_range`), `projected` (observation namespace only), rate wrappers
> (`overdispersed`, `deterministic`), likelihood family names (`poisson`,
> `neg_binomial`, `normal`, `binomial`, `beta_binomial`, `bernoulli`),
> `baseline`, `scenario`. Organized by namespace so contextual reservations
> are clear. Current-spec verification: §14.2 (lines 2291+ in current file)
> now lists 22 names across 6 namespace categories.

## 30. Section numbering and version status are unstable

**Location** — throughout

**Category** — documentation correctness; maintainability

**Defect** — Sections are misnumbered: §12 observations has §13 subsections, §13
interventions has §14 subsections, §14 timepoints follows §14.8, §15 has §16
subsections, and so on. Version labels also mix v0.1, v0.2, and v0.3-draft.

**Why it matters** — This is not cosmetic. Diagnostic codes and implementation
tickets will point to section numbers. Broken numbering makes the spec harder to
implement, review, and cite in errors.

**Fix** — Regenerate section numbers and remove stale version notes. Keep
feature status in one machine-checkable table:

```text
Feature              Status        Compiler behavior
observations          stable        compile + runtime score/sample
ode                   experimental  reject unless --experimental-ode
timepoints            future        parse error
coupling sugar        future        parse error
```

**Severity** — Medium — numbering pass **resolved-in-commit `1a7ba112`**;
status-table sub-issue **confirmed-open**.

> **Verification (2026-05-26, revised 2026-05-27).** Original misnumbering:
>
> ```
> §12 Observations             (line 1836)
> §13.1 Projections            (line 1858)   ← under §12!
> §13 Interventions            (line 2031)
> §14.1 Actions … §14.8 Balance
> §14 Timepoints               (line 2225)   ← §14 again!
> §15.1, §15.2 Reserved
> §15 Initial Conditions       (line 2277)   ← §15 again!
> ```
>
> Renumbered in commit `1a7ba112`: 75 mechanical shifts via script + 3 manual
> catches (`## 14.5 Events`, `## 14.8 Balance`, `### 13.2.1 Diagnostic-test`);
> also resolved §2.3 duplicate (Date Literals stays §2.3; "Three tiers" → §2.4;
> "Table Unit Annotations" → §2.5). One stale cross-ref `§9.9 → §9.8` fixed.
> Current-spec verification: walking the headers now shows clean parent/subsection
> alignment (`§12.1` under `§12 Observations`, `§13.1` under `§13 Interventions`,
> etc.) — see line-cited tour in the §30 history block above. The
> version-label/status-table sub-issue is still open — that's a doc-content
> choice, not a numbering one.

# Highest-priority spec cleanup

The spec needs one hard pass before more implementation work:

1. **Separate model syntax from run/inference configuration.**
2. **Define a typed semantic IR with IDs and dimension coordinates, not mangled
   strings.**
3. **Make time windows and schedules exact.**
4. **Make parameter domains/transforms mathematically bijective.**
5. **Delete or reject every "parsed but ignored" feature.**
6. **Make all worked examples compile-clean under the spec.**

The most important rule: every example in the spec should be a golden test. If
an example is future syntax, it should live in a clearly marked "future
proposal" document, not in the normative language spec.

# Refuted findings — what didn't survive verification

After upstream feedback on the earlier assessment (2026-05-27), this section is
short: **no finding is fully refuted against the reviewed spec text.** Earlier
labels said otherwise; that was wrong.

- **F2 — previously marked "refuted"; now confirmed-code-protected-but-spec-stale.**
  The compiler rejects bare stratified stoichiometry with E272 (good), but the
  §10 coupling-sugar example at lines 3331-3333 of the current spec still tells
  users "Progression and recovery are automatically replicated within each
  stratum (default behavior when no coupling is declared)" — which contradicts
  §5.1 and the actual compiler behavior. Implementation does not refute a spec
  inconsistency; the spec is the contract. Moved back to its §2 slot at the top
  of the file with the full finding text and the revised verification.

- **F11 sub-claim — previously marked "refuted"; now confirmed-open.** The
  reviewer cited the IR example
  `Cond(Pop("I"), <rate_expr>, Const(0.0))` and recommended changing it to
  `Cond(Gt(Pop("I"), Const(0.0)), <rate_expr>, Const(0.0))`. Earlier I claimed
  the phrase wasn't in the spec — that was wrong (my grep pattern was too
  literal). The text is at line 1647. Both the top-level F11 finding and the
  sub-claim are confirmed; the F11 verification block above now records both.

- **F12 sub-claim — previously marked "refuted"; now reframed as text-fix-pending.**
  The reviewer's "both formulas cannot be true" complaint is fair: §9.8 prose
  reads as `Var[G] = σ²` while the §9.7 wrapper-table form implies `Var[G] =
  σ²/dt`. Earlier I tried to reconcile by treating σ² as a parameter name,
  which is mathematically possible but isn't what the prose says. The
  imprecision is real; the F12 verification block above now records the
  correct framing (text fix: pin the parameterization in one clean sentence).

The previous "refuted" labels were the methodological mistake the upstream
reviewer named in their feedback: **don't let implementation behavior refute a
normative spec inconsistency.** For camdl, the spec is the contract — if the
compiler rejects a construct but the spec teaches users to write it, the
defect is still real.

# Status summary (revised 2026-05-27)

Using the multi-axis labels from the status legend at the top of this file:

**resolved-in-commit `1a7ba112` (cheap-batch spec edits), verified against
current spec file:**

- F7 — stale `--flow` / `--obs-model` CLI text removed from §21.5
- F16 — forcing syntax unified to colon-block form throughout
- F17 — unit-literal list includes `'count` / `'ratio`; singular `'day` fixed
- F27 — `events_block` + `balance_block` added to §24.1 grammar
- F29 — §14.2 reserved-identifier list expanded to 22 names across 6 namespaces
- F30 — section numbering pass (75 mechanical shifts + 3 manual catches;
  §2.3 duplicate resolved)
- F15 sub-claim — sha256("") special-case-display clarification

**resolved-in-commit `ab69bdbe` (spec §1 rewrite):**

- F1 — §1's "Model ≠ parameterization" prose rewritten to "Model + layered
  configuration" so the principle matches the actual layered-precedence
  design (the reviewer's "split into three files / ban values" remedy is
  explicitly rejected as breaking self-contained reproducible models)

**resolved-in-this-commit (cleanup riding alongside the assessment revision):**

- F26 — stale `--table NAME=FILE supply a runtime external() table` line
  removed from §21.2 CLI flag table

**confirmed-code-protected-but-spec-stale (the methodology-correction class):**

- F2 — compiler emits E272 on bare stratified stoichiometry, but the §10
  coupling-sugar example still teaches the contradictory "automatically
  replicated" pattern. Fix the §10 sentence.

**needs-proposal (structural changes deserving a `docs/dev/proposals/` doc):**

- F4 — split `count` (integer) from `population` (real with dim P)
- F8 + F9 + F19 — typed semantic IR with axis names + coordinate metadata
  (overlaps `docs/dev/proposals/2026-05-26-typed-indexed-reference-resolver.md`
  on the OCaml side; needs IR + Rust-runtime addendum for the
  coordinate-metadata change in IR)
- F11 — typed Boolean (and fix the `Cond(Pop("I"))` IR example at the same
  time)
- F14 — backend capability table; reject unsupported combinations before
  simulation
- F15 main body — hash-input revision: name a canonicalization step
  (`structural_model_hash`) and move simulate window / output schedules /
  scenario simulate overrides into `run_hash`
- F6 + F25 — interval-crossing semantics for observation windows and
  schedule firing
- F20 — explicit-column-mapping table load (depends on F8 axis-name proposal)
- F22 — scenario semantics: move set/scale patches out of the model file
  or include them in `run_hash`; reserve `baseline` as identity
- F23 — two-phase scenario `scale` validation (compile-time
  parameter-only; runtime full-domain check)
- F24 — explicit init `default = error | 0`; `camdl check` reports
  zero-init count

**needs code-side resolution — existing GH issues already cover:**

- F13 ↔ #99 (event-action validation gaps)
- F14 ↔ #95 (Gillespie nonhomogeneous Poisson), #120 (chain-binomial real state)

**text-fix-pending (spec-prose-only fixes that don't need a proposal):**

- F2 §10 coupling-sugar sentence (see above)
- F3 — real-compartment dimensions (currently `_planned v0.2_`; need to
  state the dim rule before that section becomes implementable)
- F5 — split `branch { }` (weights sum to 1) from `rates { }` (independent
  rate contributions) constructs
- F10 — math-function domain policy: remove silent clamps for log/sqrt/mod
- F12 — pin down `overdispersed` variance parameterization in one clean
  sentence
- F18 — dimension-correct worked examples (the forcing sub-bullet was
  resolved alongside F16; the `C_age 'per_day`, `import_rate : rate`,
  `pop : patch = read(...)` no-`'count`, `default = 0.0`-in-denominator
  bullets all stand)
- F21 — string-typed dimension levels (quoted indexing + alias columns)
- F28 — reject vs parse-and-discard policy for unimplemented features
- F30 version-status sub-issue (the feature-status table — separate from
  the numbering pass)

## Methodology meta-finding (revised 2026-05-27)

Two methodology corrections after upstream feedback on the earlier
assessment:

1. **Compiler protection is not refutation.** F2 is the canonical example:
   the OCaml compiler correctly rejects bare stratified stoichiometry, but
   the spec still teaches users to write it (§10 coupling-sugar example).
   Previously marking F2 "refuted" because of compiler behavior was wrong —
   the spec is the user-facing contract and a stale example is still a
   real bug. The reviewer named this trap explicitly: "do not let
   implementation behavior refute a normative spec inconsistency."

2. **"Resolved" needs evidence, not just a commit hash.** Earlier I labeled
   F16/F17/F27/F29/F30 "resolved by commit 1a7ba112" without showing the
   post-commit spec text. The reviewer was reading a snapshot that didn't
   include the commit; from their perspective the labels lied. Revised
   verification blocks now show line-cited evidence against the current
   spec file (`grep -n …` receipts inline) — not just the commit hash.

The remaining substantive observation about the original review still
stands: the reviewer audited spec-only with no compiler access and got
substantially all of it right. Of 30 findings, none is fully refuted
against the reviewed spec text; the only category with reduced standing
is "implementation-protected" (F2, partially F7, partially F26), and in
each of those the spec still needs to be cleaned to match.
