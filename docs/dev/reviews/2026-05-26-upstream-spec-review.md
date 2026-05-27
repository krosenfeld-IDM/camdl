---
status: open
date: 2026-05-26
kind: upstream review
scope: language spec (docs/camdl-language-spec.md) — normative design document audit
reviewer: external / upstream
methodology: spec-only audit; reviewer did not run the compiler
assessment: 2026-05-26-upstream-spec-review-assessment.md
---

# Upstream language-spec review — 2026-05-26

Reviewed the language spec as a **normative design document**, not as code. The main issue is not that the spec is too ambitious; it is that several sections promise mutually incompatible semantics. For a public-health modeling DSL, those contradictions are dangerous because modelers will copy examples, assume the documented behavior is enforced, and get a different model than intended.

# Critical findings

## 1. The spec violates its own "model ≠ parameterization" principle

**Location** — §1, §4, §4.2, §17–18, §20–21, §24

**Category** — bad abstraction; reproducibility; user footgun

**Defect** — The opening contract says `.camdl` files define model structure and that parameter values, inference configuration, and scenario selection are external. Later sections allow optional parameter defaults in the model, priors in the model, concrete scenario `set` values in the model, `baseline` scenarios with parameter values, and a compilation pipeline that takes `params.toml` before expansion.

**Why it matters** — A reviewer cannot tell whether a `.camdl` file is a structural model, a calibrated analysis, a scenario definition, or an inference prior bundle. That breaks reviewability and hashing. Changing a prior or scenario value can change posterior or counterfactual conclusions without looking like a model change.

**Fix** — Split the specification into three contracts:

```text
camdl-model-spec.md       structural DSL only
camdl-run-spec.md         params, scenarios, seeds, output, caching
camdl-inference-spec.md   priors, transforms, fit stages, diagnostics
```

Then enforce the boundary:

* `.camdl` may declare parameter names, kinds, dimensions, and optional structural bounds.
* Concrete parameter values live in params/scenario/experiment files.
* Priors either move to fit config, or the opening principle must be rewritten to say priors are part of the model contract.
* `camdlc compile` must not require parameter values.

**Severity** — Critical

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

**Severity** — Critical

## 3. Real-valued compartments have no dimensional semantics

**Location** — §2.2.1, §3, §11, §23.5

**Category** — type design; numerical correctness

**Defect** — The dimensional checker says compartment references have dimension `P`, and ODE derivatives must have dimension `P·T⁻¹`. But real compartments are introduced for things like environmental reservoirs, where `W` is not a population count. The cholera example uses `W` as a concentration, yet the language has no way to declare the dimension of `W` or `K`.

**Why it matters** — This makes the environmental-reservoir example either dimensionally invalid or silently treated as population. A modeler can write a waterborne force of infection that passes only because the checker lacks the vocabulary to represent concentration, dose, viral load, or wastewater signal units.

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

Also change E306 from "ODE derivative must have dimension `P·T⁻¹`" to "ODE derivative must have the compartment's declared dimension divided by time."

**Severity** — Critical

## 4. Parameter domains and transforms are underspecified at the boundaries

**Location** — §4.1, §4.4, §8.4, §21.2

**Category** — statistical correctness; user footgun

**Defect** — `rate` is documented as `≥ 0` with log transform; `probability` is `[0,1]` with logit transform; `count` is integer ≥ 0 but examples use `let iota : count = 1e-6` and `let obs_floor : count = 0.01`. Finite upper bounds on positive/rate/count parameters do not override the default log transform.

**Why it matters** — A log transform cannot represent exactly zero. A logit transform cannot represent exactly 0 or 1. A count cannot be both an integer-valued parameter and a tiny population-dimensional continuity correction. These ambiguities directly affect inference, priors, initialization, and likelihoods.

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

## 5. Probabilistic branching is mathematically ambiguous

**Location** — §9.1.2

**Category** — statistical correctness; user footgun

**Defect** — The construct is called "probabilistic branching," and branch weights are said to have probability dimension `[0,1]`, but the compiler "does not enforce that weights sum to 1" and allows "rate-weighted branches."

**Why it matters** — For:

```camdl
infection : S --> { A : p, B : q } @ r
```

if `p + q = 0.8`, the total depletion rate from `S` becomes `0.8r`, not `r`. That is no longer "one event, multiple outcomes"; it is silently changing the total event rate.

**Fix** — Separate the two concepts:

```camdl
# true branching: weights must sum to 1
infection : S --> branch { A : p, B : 1 - p } @ r

# rate splitting: each destination has its own rate contribution
infection : S --> rates { A : rA, B : rB }
```

For `branch`, enforce `sum(weights) = 1` when statically provable. When weights depend on parameters, runtime validation must reject invalid values or assign `-inf` likelihood during inference.

**Severity** — Critical

## 6. Observation time-window semantics are not defined

**Location** — §12–13, §16–17, §22.5

**Category** — statistical correctness; user footgun

**Defect** — `incidence(transition)` is described as "cumulative flow since last observation," but the spec does not define the interval convention. It does not say whether an observation at time `t` covers `[t-every, t]`, `(t-every, t]`, `[t, t+every)`, or the interval since the previous observed row. It also does not define first-observation behavior, missing observations, irregular observation times, or how data timestamps align with output schedules.

**Why it matters** — Weekly AFP cases, campaign windows, and incidence curves are interval data. A one-bin shift changes inferred reporting rates, intervention effects, and epidemic timing.

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

For irregular data, the data file should drive the interval boundaries, not `every`.

**Severity** — Critical

## 7. The data-observation contract is missing

**Location** — §12–13, §21–22

**Category** — statistical correctness; bad UX

**Defect** — The model-level `observations {}` block defines streams, projections, and likelihoods, but the CLI still documents `--flow` and `--obs-model`, which bypass that model-level observation contract. The spec also does not define the observation data file schema for multiple streams.

**Why it matters** — A modeler can define `weekly_cases` in the DSL and then fit with `--flow recovery --obs-model discretized_normal`, accidentally using a different observation model. That makes the `.camdl` review meaningless for inference.

**Fix** — Make the model observation block authoritative. Inference commands should accept stream selection, not model replacement:

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

Legacy `--flow` / `--obs-model` should be marked as low-level deprecated commands or removed.

**Severity** — Critical

## 8. Tables with repeated dimensions need axis names

**Location** — §6, §9.7, §23, §25–26

**Category** — type design; user footgun

**Defect** — A table can be declared as `patch × patch`, but both axes have the same dimension name. Named indexing cannot distinguish source patch from destination patch, so examples rely on positional indexing:

```camdl
distance[i,j]
mig[dst,src]
```

File loading also uses positional mapping and treats column names as human-readable only.

**Why it matters** — Spatial kernels and migration matrices are exactly where source/destination swaps are common and hard to detect. A `src,dst,value` file read as `dst,src,value` can reverse migration or infection importation with no type error.

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

## 9. Expanded names are not a safe semantic representation

**Location** — §4.3, §5, §18.1, §25–26

**Category** — FFI; type design; user footgun

**Defect** — The spec repeatedly relies on mangled names such as `N_urban`, `infection_child`, and `S_child_p1`. It does not define escaping or collision rules, yet examples use level names with underscores such as `kano_dala` and `borno_maiduguri`.

**Why it matters** — String mangling is not reversible. These collide:

```text
S[a_b, c]  -> S_a_b_c
S[a, b_c]  -> S_a_b_c
```

The runtime, inference output, observation projections, and scenario patches cannot safely recover dimension coordinates from names. This is already the kind of design that leads to prefix/suffix bugs.

**Fix** — IR must carry IDs and coordinate metadata:

```json
{
  "compartment_id": 42,
  "base": "S",
  "coords": { "age": "child", "patch": "kano_dala" },
  "display_name": "S[age=child, patch=kano_dala]"
}
```

Flat names may exist only as display strings. Projection, scenario, and inference code must use IDs, not parsed names.

**Severity** — Critical

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

**Why it matters** — `sqrt(negative)` and `mod(a,0)` are model errors or invalid parameter proposals. Turning them into 0 can shut off a rate, create artificial thresholds, or make invalid particles look plausible. This is exactly how numerical plumbing bugs become scientific conclusions.

**Fix** — Define a strict domain policy:

```text
Compile-time invalid constant expression -> hard compiler error.
Runtime invalid expression in simulation -> SimError with name/time/context.
Runtime invalid expression in particle inference -> particle gets -inf weight if proposal-dependent; setup error if structural.
```

Do not clamp `sqrt`, `log`, or `mod`.

**Severity** — Critical

## 11. Boolean expressions are not typed

**Location** — §9.3, §9.7, §17.3

**Category** — type design; user footgun

**Defect** — Comparisons exist, `if` conditions exist, guards use `and/or`, and examples multiply comparisons:

```camdl
let is_pulse = (day_of_year > 250.0) * (day_of_year < 252.0)
```

But the spec never defines a Boolean type, whether comparisons return Booleans or 0/1 numbers, or whether Booleans can be multiplied.

**Why it matters** — Boolean-as-number is convenient but dangerous. A modeler can accidentally use a predicate as a rate multiplier without realizing the discontinuity. It also affects typechecking of `if`, `time_when`, guards, and summary predicates.

**Fix** — Add a real Boolean type. Comparisons return `Bool`. `if` requires `Bool`. Arithmetic on `Bool` is illegal. If indicator functions are desired, make them explicit:

```camdl
indicator(day_of_year > 250 and day_of_year < 252)
```

Also fix the IR example that says `if I > 0 ...` becomes `Cond(Pop("I"), ...)`; it should become `Cond(Gt(Pop("I"), Const(0)), ...)`.

**Severity** — Critical

## 12. Overdispersion is parameterized inconsistently

**Location** — §9.7, §9.8

**Category** — statistical correctness

**Defect** — The spec says `σ²_SE` is "the variance of the Gamma noise multiplier," but the wrapper table says:

```text
Var = mean + mean² · σ² / dt
```

Both cannot be true. If the multiplier variance is `σ²`, the count variance is `mean + mean²σ²`. If the multiplier variance is `σ²/dt`, the table formula is correct.

**Why it matters** — This changes the meaning of the overdispersion parameter and therefore the posterior for extra-demographic stochasticity. A model calibrated under one interpretation cannot be compared to a model simulated under the other.

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

## 13. Events and interventions allow silent invalid state changes

**Location** — §13–14.8

**Category** — numerical correctness; user footgun

**Defect** — `add` accepts negative values and only warns if the result makes a compartment negative. `transfer(fraction=...)` does not explicitly require the fraction to be finite and in `[0,1]`. `transfer(count=...)` does not define count domain or rounding. `balance` mentions "clamps" even though silent non-negativity clamps are not otherwise specified.

**Why it matters** — State modification is how vaccination, importation, cohort entry, and campaign scenarios are represented. A negative campaign count, fraction greater than 1, or silent clamp can turn an intervention counterfactual into a different policy.

**Fix** — Make action domains strict:

```text
transfer fraction: finite, 0 ≤ f ≤ 1
transfer count: finite, integer-compatible, nonnegative
add count: finite, integer-compatible; negative only with explicit allow_negative = true
set integer compartment: finite, integer-compatible, nonnegative
```

If an event would make an integer compartment negative, that is a hard simulation error or a dead particle in inference. No silent clamp.

**Severity** — Critical

## 14. Backend compatibility is underspecified for time-dependent rates

**Location** — §7, §9.8, §11, §18, §21–22

**Category** — statistical correctness; numerical correctness

**Defect** — The language encourages time-dependent forcing, interventions, events, and real-compartment ODEs, while the CLI defaults to `gillespie`. The spec only says Gillespie rejects `overdispersed`; it does not say whether Gillespie is exact, approximate, or illegal for `t`, forcing functions, interventions, or real-valued ODE coupling.

**Why it matters** — Standard direct-method SSA is exact only when propensities are constant between events. Seasonal forcing and ODE-coupled hazards violate that. A default backend that is exact for one model class and approximate for another needs explicit acceptance rules.

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

## 15. Content-addressable output omits simulation-defining inputs

**Location** — §19–20

**Category** — reproducibility; not wired through

**Defect** — `model_hash` excludes `simulate`, and `sim_hash` includes model hash, base params, backend, dt, and tool version. It does not include `simulate.from`, `simulate.to`, output schedules, observation/synthetic-output schedules, enabled event schedules, or scenario-level `simulate.to`. The spec also says `sha256("")` has prefix `00000000`, which is false unless special-cased.

**Why it matters** — A 2-year run and a 5-year run can reuse the same cached directory. That is not a cache miss; it is a wrong result served from cache.

**Fix** — Define separate hashes:

```text
model_hash = structural model only
run_hash   = model_hash + params + backend + dt + simulate window + output schedules + tool version
scen_hash  = resolved scenario delta, including enable/disable/set/scale and scenario simulation overrides
fit_hash   = run_hash + priors + transforms + data hash + inference config
```

If baseline should display as `00000000`, define it as a literal special case, not `sha256("")`.

**Severity** — Critical

# High findings

## 16. Forcing syntax contradicts itself

**Location** — §7, §23 full example, §27 primitive summary

**Category** — spec consistency; user footgun

**Defect** — The spec says every forcing declaration must carry a tier-3 unit literal:

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

**Why it matters** — Users copy examples. In this spec, copied examples are syntax errors or, worse, accepted by an old parser with wrong dimensional inference.

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

**Severity** — High

## 17. Unit literals are inconsistently defined

**Location** — §2.1, §2.3, §2.4, §7, §16, §23

**Category** — type design; user footgun

**Defect** — §2.1 says supported units are only time and rate units. Later sections use `'ratio`, `'count`, and singular `'day`. The text also mentions invalid parameter syntax such as `beta : rate 'per_month`, even though the spec says unit literals cannot be applied to unknown parameter values.

**Why it matters** — Unit syntax is the primary defense against per-day/per-week and count/fraction mistakes. If the unit grammar is inconsistent, users will either avoid units or rely on examples that do not parse.

**Fix** — Define one unit grammar:

```text
time_unit_decl_unit = 'days | 'weeks | 'months | 'years
duration_unit       = 'days | 'weeks | 'months | 'years
rate_unit           = 'per_day | 'per_week | 'per_month | 'per_year
value_unit          = duration_unit | rate_unit | 'count | 'ratio
```

Then update all examples. Either accept singular aliases like `'day`, or remove them.

**Severity** — High

## 18. Several worked examples are dimensionally wrong

**Location** — §6, §7, §9.1, §23

**Category** — numerical correctness; user footgun

**Defect** — Examples contain dimension errors or dangerous defaults:

* `C_age : age × age 'per_day` combined with `beta : rate` gives `P·T⁻²` in infection rates.
* `import_rate : rate` used as an inflow allocation gives `T⁻¹`, not `P·T⁻¹`.
* `pop : patch = read(...)` omits `'count`, making population tables dimensionless.
* `distance : patch × patch = read(..., default = 0.0)` is then used in a denominator.
* The full example uses old forcing syntax without the required unit literal.

**Why it matters** — Worked examples are de facto tests and tutorials. If they do not typecheck under the spec, users learn the wrong modeling patterns.

**Fix** — Make all examples compile-clean under the stated dimensional checker. For example:

```camdl
C_age : contact_age × participant_age = [[...]]      # dimensionless
beta  : rate

import_rate : positive [P/T]

pop : patch 'count = read("data/lga_pop.tsv")

distance : src:patch × dst:patch 'km = read("data/lga_dist.tsv")
```

Do not use `default = 0.0` for denominator tables.

**Severity** — High

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

**Why it matters** — Age × patch parameters are common: reporting rates, coverage, importation weights, initial conditions, and covariates. The spec gives users no clear answer about whether these are legal.

**Fix** — Support arbitrary dimension vectors:

```camdl
parameters {
  rho[age, patch] : probability
}
```

Then define named indexing, runtime `--param-vec` file format, scenario `set`/`scale`, and IR coordinate metadata for multi-dimensional parameters. If v0.3 only supports one dimension, delete the multi-dimensional claims.

**Severity** — High

## 20. CSV/TSV table loading by position is too error-prone

**Location** — §6.2–6.4

**Category** — user footgun; scientific correctness

**Defect** — Table files use positional mapping from the table type signature, and column names are "for human readability." Multi-value columns also force all output tables to share one unit annotation.

**Why it matters** — Public-health spatial data often has columns like `src`, `dst`, `distance`, `population`, `coverage`. Positional mapping makes a source/destination swap invisible. Multi-value files commonly mix units, such as `pop` as count and `init_sus` as ratio.

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

## 21. Dimension levels from files need a string/identifier policy

**Location** — §5, §6.3, §9.7, §25–26

**Category** — UX; FFI; type design

**Defect** — Data-derived levels are actual file values, but expression syntax only shows identifier-like levels. Real geographic labels often contain spaces, hyphens, leading digits, slashes, or underscores.

**Why it matters** — Nigeria LGA names and administrative codes are not guaranteed to be valid identifiers. If the compiler canonicalizes them, users need to know the mapping. If it does not, many real datasets cannot be indexed directly.

**Fix** — Treat dimension levels as strings internally and support quoted indexing:

```camdl
S[patch = "Kano Dala"]
S[patch = "001"]
```

For display names, preserve original labels. For identifiers, allow optional aliases:

```camdl
dimensions {
  patch = read("lga.tsv", column = "lga_name", alias_column = "lga_id")
}
```

**Severity** — High

## 22. Scenario semantics are too powerful for the model file

**Location** — §17–18

**Category** — bad abstraction; reproducibility; user footgun

**Defect** — Scenario blocks can set and scale parameters, inherit from other scenarios, alter `simulate.to`, enable interventions, and compose patches. Some examples put baseline parameter values inside scenario definitions.

**Why it matters** — A model file can contain a hidden calibrated baseline and counterfactual values. Reviewers looking only for structural model errors may miss that the scenario block changes transmission, coverage, or horizon.

**Fix** — Keep intervention declarations in `.camdl`, but move scenario patches to experiment/run files. If embedded scenarios remain, mark them as analysis configuration and include them in the relevant run hash. Also reserve `baseline` as the identity scenario; do not allow a user-defined `baseline` block to mutate parameters.

**Severity** — High

## 23. Scenario `scale` cannot be validated at compile time as claimed

**Location** — §18.1–18.3

**Category** — not wired through; user footgun

**Defect** — The spec says `scale` on a probability parameter that would exceed `[0,1]` is a compile error. But parameter values are external, and scenario RHS expressions can reference current parameter values, so the compiler often cannot know the result.

**Why it matters** — `scale = { rho = 1.5 }` may be valid if `rho = 0.4` and invalid if `rho = 0.8`. This must be checked against the actual runtime parameter environment.

**Fix** — Replace "compile error" with a two-phase rule:

```text
Compile time: validate target parameter exists and scale expression is parameter-only.
Runtime: after applying params and scenario patch, validate all parameter domains and bounds.
```

If all values are literal and statically known, the compiler may reject early as an optimization.

**Severity** — High

## 24. Initial conditions defaulting to zero is a footgun

**Location** — §15

**Category** — user footgun; scientific correctness

**Defect** — Unlisted compartments and unlisted stratum combinations default to 0.

**Why it matters** — In a 774-patch model, an omitted init entry can erase an entire region's population. The model still runs and may produce zero incidence in omitted areas, which looks like epidemiology but is just missing initialization.

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

## 25. Schedule boundary semantics are not robust enough

**Location** — §13–14.7, §16–17

**Category** — numerical correctness; user footgun

**Defect** — `at_day` fires when `|t - target| < 0.5 * dt`. Output, observation, intervention, and event schedules are described separately rather than as a single boundary system.

**Why it matters** — Timed vaccination, importation pulses, reporting windows, and output snapshots should not depend on floating-point tolerance or whether `dt` lands near a target. A campaign can fire one step early, one step late, or not at all.

**Fix** — Define all schedules by interval crossing:

```text
An event at time τ fires exactly once when the integrator advances from t_old < τ ≤ t_new.
Backends must split steps at event, observation, output, and simulation-end boundaries,
or reject schedules not aligned with dt.
```

Do not use tolerance-based firing as normative semantics.

**Severity** — High

## 26. External table loading contradicts compile-time inlining

**Location** — §6.5, §22.2

**Category** — reproducibility; FFI

**Defect** — §6.5 says external tables are loaded at compile time and inlined into the IR. The CLI later documents:

```bash
--table NAME=FILE  supply a runtime external() table
```

but the language has no `external()` table syntax and the hash rules do not cover runtime table contents.

**Why it matters** — Table data can be transmission kernels, populations, schedules, and contact matrices. If table values can change at runtime without changing model hash or IR, reproducibility is broken.

**Fix** — Pick one design:

1. **Compile-time only:** remove `--table`; all table data is in IR and model hash.
2. **Runtime external tables:** add explicit syntax:

   ```camdl
   kernel : src:patch × dst:patch external 'ratio
   ```

   Then include table file content hash in `run_hash`.

**Severity** — High

# Medium findings

## 27. The grammar omits advertised blocks

**Location** — §14.5–14.8, §25.1, §27 primitive summary

**Category** — spec consistency; tests

**Defect** — `events {}` and `balance {}` are documented, but the file-level grammar does not list `events_block` or `balance_block`. The primitive summary also omits them.

**Why it matters** — Implementers will disagree on whether these are real language constructs or prose-only future features.

**Fix** — Add them to the grammar if supported. If not supported, move them to a future section and require the compiler to reject them.

**Severity** — Medium

## 28. "Parsed but discarded" features should not exist

**Location** — §11, §13.4, §14 Timepoints

**Category** — user footgun; spec consistency

**Defect** — The spec says the `ode {}` block and `timepoints {}` block are parsed but discarded or unavailable. It also contains stale status text for observations.

**Why it matters** — Accepted-but-ignored syntax is worse than rejected syntax. A modeler can think they defined an ODE or timepoint while the model runs without it.

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

## 29. Reserved identifiers are incomplete

**Location** — §2.3, §9.7, §12–14, §15.2, §27 primitive summary

**Category** — type design; user footgun

**Defect** — The reserved list omits names that are treated specially elsewhere: `t`, `origin`, `projected`, `date`, `add_calendar_months`, `add_calendar_years`, `date_range`, `unchecked_dim`, `overdispersed`, `deterministic`, likelihood family names, `baseline`, and `scenario`.

**Why it matters** — A user-defined parameter or table named `projected`, `date`, or `baseline` can make expressions ambiguous or context-dependent.

**Fix** — Define reserved names by namespace and context:

```text
Global reserved: t, origin, t_start, t_end, sum, consecutive, compartments, date, ...
Observation-likelihood reserved: projected
Experiment-summary reserved: baseline, scenario
Function reserved: exp, log, sqrt, ...
```

Then reject user declarations that collide.

**Severity** — Medium

## 30. Section numbering and version status are unstable

**Location** — throughout

**Category** — documentation correctness; maintainability

**Defect** — Sections are misnumbered: §12 observations has §13 subsections, §13 interventions has §14 subsections, §14 timepoints follows §14.8, §15 has §16 subsections, and so on. Version labels also mix v0.1, v0.2, and v0.3-draft.

**Why it matters** — This is not cosmetic. Diagnostic codes and implementation tickets will point to section numbers. Broken numbering makes the spec harder to implement, review, and cite in errors.

**Fix** — Regenerate section numbers and remove stale version notes. Keep feature status in one machine-checkable table:

```text
Feature              Status        Compiler behavior
observations          stable        compile + runtime score/sample
ode                   experimental  reject unless --experimental-ode
timepoints            future        parse error
coupling sugar        future        parse error
```

**Severity** — Medium

# Highest-priority spec cleanup

The spec needs one hard pass before more implementation work:

1. **Separate model syntax from run/inference configuration.**
2. **Define a typed semantic IR with IDs and dimension coordinates, not mangled strings.**
3. **Make time windows and schedules exact.**
4. **Make parameter domains/transforms mathematically bijective.**
5. **Delete or reject every "parsed but ignored" feature.**
6. **Make all worked examples compile-clean under the spec.**

The most important rule: every example in the spec should be a golden test. If an example is future syntax, it should live in a clearly marked "future proposal" document, not in the normative language spec.
