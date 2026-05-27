---
status: open
date: 2026-05-26
kind: language-spec review
scope: docs/camdl-language-spec.md — normative design document audit
reviewer: external / upstream
verification: per-finding code+spec checks added 2026-05-26 (paste-the-receipt per CLAUDE.md). Each confirmed finding ends with a `Verification (2026-05-26)` block that records what we checked locally and any severity adjustment.
verdict: 28 confirmed (some with severity adjustments after verification) / 1 definitively refuted (F2) / 2 sub-claims refuted (within F11 and F12 framing) — refuted items moved to the bottom of this file.
followup-commits:
  - 1a7ba112 (cheap-batch spec edits: F7, F15-subclaim, F16, F17, F27, F29, F30)
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

## 2. _(moved to refuted section — see bottom)_

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

**Severity** — Critical → **High** (after verification)

> **Verification (2026-05-26):** §21.5 (renumbered from §22.5) still documented
> `--flow recovery --obs-model discretized_normal` in the worked examples. Grep
> of current `rust/crates/cli/src/args/mod.rs` for these flags returns zero hits
> — they were removed in the 2026-05-25 CLI UX revision. The bypass the reviewer
> warned about no longer exists at the CLI surface; the spec text was stale and
> was fixed in commit `1a7ba112` (cheap-batch). Severity downgraded from
> Critical to High because the live failure mode is gone; the canonical
> multi-stream data-file schema described by the reviewer is still missing from
> the spec and remains a real open issue.

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

> **Verification (2026-05-26):** §4.3 explicitly says "the compiler always
> mangles to `N0_urban` in the IR". The example level names `kano_dala`,
> `borno_maiduguri` contain underscores. Collision is a real concern. Overlaps
> with `docs/dev/proposals/2026-05-26-typed-indexed-reference-resolver.md`
> (compiler-side); the IR + Rust side still needs an addendum proposal for the
> coordinate-metadata change the reviewer recommends. Confirmed.

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

> **Verification (2026-05-26):** The `is_pulse` example exists at line 1575:
> `let is_pulse = (day_of_year > 250.0) * (day_of_year < 252.0)` — using
> comparison results as numeric 0/1 multipliers. No Boolean type is defined.
> Confirmed. The sub-claim about an IR example `Cond(Pop("I"), ...)` was not
> located in current spec text (the exact phrase does not appear; possibly an
> older draft) — that sub-claim is listed in the "Refuted sub-claims" section at
> the bottom; the top-level finding stands.

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

> **Verification (2026-05-26):** §9.8 prose (line 1635): "σ²_SE, the variance of
> the Gamma noise multiplier (which has mean 1)". §9.7 wrapper table (line
> 1585): `Var = mean + mean² · σ² / dt`. The two are inconsistent on the literal
> reading: if `Var[G] = σ²` per prose, then under Gamma-Poisson
> `Var[events] = mean + mean²·σ²` with no `/dt`; the table form requires
> `Var[G] = σ²/dt`. The spec needs to pick one and say which. The reviewer's
> "both cannot be true" framing is too binary (a careful read can reconcile them
> by treating σ² as a _name_ and `σ²/dt` as the actual Var[G]) — that framing
> sub-claim is in the "Refuted sub-claims" section below; the top-level
> contradiction stands. Confirmed.

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

> **Verification (2026-05-26):** Confirmed — §7 had both forms; the OCaml parser
> at `parser.mly:319` accepts only `name : KIND 'unit { … }`. Rewrote every
> forcing example in §7 and §23 to the colon-block form in commit `1a7ba112`.
> **Resolved.**

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

> **Verification (2026-05-26):** §2.1 list was missing `'count` and `'ratio`;
> one occurrence of singular `'day` at line 2369. Added `'count` and `'ratio` to
> the supported-units list with their dimensions, and changed `every = 1 'day`
> to `every = 1 'days` (the lexer only accepts plural). Commit `1a7ba112`.
> **Resolved.**

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

**Severity** — High (partial confirmation)

> **Verification (2026-05-26):** The forcing-syntax sub-bullet was confirmed and
> resolved in commit `1a7ba112` alongside F16. The individual dimension-error
> sub-bullets (`C_age 'per_day`, `import_rate : rate`, `pop : patch = read(...)`
> no `'count`, `default = 0.0` denominator) are each present in the spec but
> each one requires either compiler test-drive or a proposal for the right
> structural fix (axis-named tables — see F8 — would change the call site too).
> Left as a partial-confirm; per-bullet remediation can land alongside the F8 /
> F4 proposals.

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

**Severity** — High → **Medium** (after verification)

> **Verification (2026-05-26):** Grep for `external()` syntax in spec returns
> nothing; grep for `--table` flag in `rust/crates/cli/src/args/mod.rs` also
> returns nothing. The contradiction the reviewer warned about is gone in
> current code; the spec hash rules are still incomplete in the way F15
> describes, but the specific runtime-external-tables-bypass concern is moot.
> Downgraded to Medium because no implementation gap exists today.

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

> **Verification (2026-05-26):** Confirmed — §24.1 grammar listing was missing
> `events_block` and `balance_block` even though the OCaml parser supports both
> (verified at `parser.mly:90-100`). Added both productions to the declaration
> grammar in commit `1a7ba112`. **Resolved.**

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

> **Verification (2026-05-26):** Confirmed — §14.2 list was 5 names; expanded to
> cover `t`, `dt`, `origin`, calendar builtins, `projected` (observation
> namespace), rate wrappers, likelihood family names, `baseline`, `scenario`.
> Organized by namespace so the contextual reservations (observation-only
> `projected`, etc.) are clear. Commit `1a7ba112`. **Resolved.**

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

**Severity** — Medium → **resolved (numbering pass)**

> **Verification (2026-05-26):** Confirmed:
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
> "Table Unit Annotations" → §2.5). One stale cross-ref `§9.9 → §9.8` fixed. The
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

# Refuted findings — we looked, did not find

## 2. Bare stratified transition semantics contradict "no auto-localization"

**Reviewer's claim:** the spec promises bare stratified compartments in
stoichiometry auto-localize per stratum — specifically that the coupling-sugar
example's `progression : E --> I @ sigma * E` documents auto-localization as
default behavior. The reviewer's recommended fix was to make bare stratified
compartments illegal in stoichiometry.

**Verification (2026-05-26): refuted.** The spec §5.1 is the _opposite_ of what
the reviewer described — bare stratified is already illegal:

> "In stoichiometry (left of `@`, source/destination of `-->`): **all dimensions
> of the compartment must be specified.** You cannot write into a marginal — the
> compiler must know exactly which cell gains or loses an individual."

The spec follows that paragraph with an explicit ERROR example showing
partial-stratification rejected. And the OCaml compiler enforces this — verified
by writing a test model with bare-stratified transitions and running
`camdl compile`:

```
$ camdl compile /tmp/test_bare_strat.camdl
error[E272]: compartment 'E' is stratified but used without indices in stoichiometry
  = hint: pick an expansion or index the transition: E_child, E_adult
error[E272]: compartment 'I' is stratified but used without indices in stoichiometry
error[E272]: compartment 'R' is stratified but used without indices in stoichiometry
```

The reviewer appears to have confused §10 coupling-sugar (where the compiler
expands a base model into per-stratum transitions via _explicit_ `coupling[dim]`
declarations) with auto-localization at the transition level. They are different
mechanisms; only the former exists. No spec change required.

# Refuted sub-claims

These sub-claims were inside otherwise-confirmed top-level findings; the parent
findings stand, but the specific sub-claims do not.

- **F11 sub-claim about a particular IR example.** The reviewer cited an IR
  example saying `if I > 0 ...` becomes `Cond(Pop("I"), ...)` and recommended
  changing it to `Cond(Gt(Pop("I"), Const(0)), ...)`. The exact phrase
  `Cond(Pop("I"))` does not appear in the current spec (grep returned no
  matches). Possibly an older draft or a misquote. The broader F11 finding —
  Boolean expressions are not typed, with the
  `is_pulse = (day_of_year > 250.0) * (day_of_year < 252.0)` example shown — is
  confirmed and stands.

- **F12 sub-claim that "both formulas cannot be true".** The reviewer's binary
  framing skips the possibility that `σ²` is a _parameter name_ and the actual
  Gamma-multiplier variance is `σ²/dt` (per the wrapper table). On that reading
  the two formulas are reconcilable; the spec is still imprecise (the prose
  calling `σ²_SE` "the variance of the Gamma noise multiplier" reads as
  `Var[G] = σ²`, not `σ²/dt`), but the contradiction is in the _prose
  imprecision_, not the math itself. The top-level F12 finding — that the
  parameterization is ambiguously specified and needs pinning down — stands; the
  rhetorical claim of strict mathematical contradiction does not.

# What this means for the spec-cleanup proposal

The reviewer's six-step cleanup list above stands. Even removing the one
definitively refuted finding (F2), 28 of 30 findings hold as written or with the
noted severity adjustments. Status by category:

- **Resolved by the 2026-05-26 cheap-batch commit `1a7ba112`:** F7 (CLI flag
  deletion), F16 (forcing syntax unification), F17 (unit-literal completeness),
  F27 (grammar listing), F29 (reserved identifier list), F30 (section
  renumbering), F15 sub-claim (sha256 special-case display).

- **Need proposals before landing — structural changes that ripple through
  IR/tests:**
  - F1 + F22: split model from parameterization
  - F4: split `count` (integer) from `population` (real with dim P)
  - F8 + F9 + F19: typed semantic IR with axis names (overlaps
    `docs/dev/proposals/2026-05-26-typed-indexed-reference-resolver.md` on OCaml
    side; needs IR + Rust addendum for the coordinate-metadata change in IR)
  - F11: typed Boolean
  - F14: backend capability table
  - F15 main body: hash-input revision (move `simulate.from/to`, output
    schedules, scenario simulate overrides into the run hash)
  - F6 + F25: interval-crossing semantics for observation windows and schedule
    firing
  - F20: explicit-column-mapping table load (depends on F8 axis-name proposal)
  - F23: two-phase scenario `scale` validation
  - F24: explicit init `default = error | 0`

- **Need code-side resolution (existing GH issues already cover):**
  - F13 ↔ #99 (event-action validation gaps)
  - F14 ↔ #95 (Gillespie nonhomogeneous Poisson), #120 (chain-binomial real
    state)

- **Spec-only edits but design-flavored, not pure docs:**
  - F3 (real-compartment dimensions — currently _planned v0.2_)
  - F5 (split `branch { }` from `rates { }` constructs)
  - F10 (math-function domain policy)
  - F12 (pin down overdispersion variance parameterization)
  - F18 (dimension-correct worked examples — partly resolved by F16; per-bullet
    fixes alongside F8/F4 proposals)
  - F21 (string-typed dimension levels)
  - F26 main spec wording (already removed in code, just needs prose cleanup)
  - F28 (reject vs parse-and-discard policy for unimplemented features)
  - F30 version-status sub-issue (status table)

## Methodology meta-finding

The reviewer audited spec-only with no compiler access and got 28 of 30 right.
That's strong evidence the spec is genuinely contradictory in the ways described
— not just imprecise. F2 is the one case where the implementation has been
pulled toward correct semantics ahead of the spec text catching up to clarify.
