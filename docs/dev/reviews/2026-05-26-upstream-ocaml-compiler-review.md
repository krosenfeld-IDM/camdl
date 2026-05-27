---
status: open
date: 2026-05-26
kind: upstream review
scope: OCaml compiler (parser, expander, dimcheck, validate, golden tests) — static audit against `docs/camdl-language-spec.md`
reviewer: external / upstream
methodology: static code audit; dune not available in reviewer container; Rust inference backend explicitly out of scope
counts: 8 Critical / 6 High / 2 Medium + 1 structural cross-cutting fix
comparison: 2026-05-26-week-audit-comparison.md
---

# Upstream OCaml compiler review — 2026-05-26

Reviewed the OCaml compiler portion of the attached zip against the language spec as the contract. I did not audit the Rust inference backend in this pass. I also could not run the OCaml test suite because `dune` is not installed in this container, so this is a static code audit.

## Critical findings

### 1. Indexed references are lowered by string concatenation instead of a dimension-aware resolver

**Location** — `ocaml/lib/compiler/expander.ml:926-931`, `1499-1578`, `3716-3755`

**Category** — user footgun; not wired through; numerical correctness

**Defect** — Named index labels are discarded by `index_item_to_str`, and `EIndex` lowering builds concrete names with `String.concat "_"`. The compiler does not implement the spec's order-independent named indexing or omitted-dimension summation for compartment references, transition projections, table accesses, indexed lets, forcing functions, or indexed parameters.

**Why it matters** — `S[patch = p]`, `S[sex = female, age = child]`, and `incidence(infection[patch = p])` are core public-health modeling idioms. The compiler can reject valid models, bind the wrong object when labels overlap, or produce unknown partial names such as `infection_kano` instead of summing `infection_kano_child + infection_kano_adult + ...`. Observation streams by patch are especially exposed: the likelihood can attach to the wrong flow or fail to compile only after expansion.

**Fix** — Replace all `index_item_to_str` + string-concatenation paths with one resolver:

```ocaml
resolve_indexed_ref :
  context ->
  env ->
  namespace:[ `Compartment | `TransitionFlow | `Table | `Parameter | `Let | `Forcing ] ->
  base:string ->
  index_item list ->
  resolved_ref
```

The resolver must know each object's declared dimension vector, map named indices by dimension name, map positional indices by declaration order, validate dimension membership, and expand omitted dimensions to sums in expression/projection contexts. Use it in `resolve_expr`, `resolve_stoich_ref`, `expand_observations`, `expand_init`, interventions, and scenario indexed parameter handling.

**Severity** — Critical

### 2. Table lookup arity and shape are not validated; under-indexing silently selects the wrong cell

**Location** — `ocaml/lib/compiler/expander.ml:1507-1523`, `2788-2837`; no table shape validation in `ocaml/lib/ir/validate.ml:63-129`

**Category** — numerical correctness; user footgun

**Defect** — For `TableLookup`, the compiler iterates over the provided index items and uses `List.nth tdims i`, but it never checks that the number of supplied indices equals the table rank. `C_age[child]` against `C_age : age × age` compiles to a lookup at a lower-dimensional linear index instead of an arity error. Inline table values are also flattened and emitted without checking that the number of cells equals the product of declared dimension sizes.

**Why it matters** — Contact matrices, spatial kernels, age durations, fertility tables, and mortality tables drive force of infection and demography. A missing index can use cell 0 or another row-major prefix cell instead of the intended matrix entry. A too-short inline table can fail late or corrupt runtime behavior; a too-long table can silently carry unused data. This directly changes transmission intensity, inferred `beta`, and intervention impact estimates.

**Fix** — Add `validate_table_decl` before IR emission:

```ocaml
expected_cells = product (List.map dim_size tdims)
actual_cells   = count_flattened_inline_cells td.tvalue
```

Emit a hard diagnostic when they differ. In lookup lowering, require exact arity, resolve named indices by declared dimension, and reject under/over-indexed lookups before producing `Ir.TableLookup`.

**Severity** — Critical

### 3. Block-form transitions without `rate =` compile as zero-rate transitions

**Location** — `ocaml/lib/compiler/parser.mly:344-350`, `402-417`

**Category** — numerical correctness; user footgun

**Defect** — `transition_body` initializes `rate` to `EConst 0.0`. A block transition with only `tag = ...` or with a missing `rate = ...` compiles successfully as a transition that never fires.

**Why it matters** — Omitting `rate` from `infection : S --> I { tag = "transmission" }` removes transmission from the model without a compiler error. Inference then compensates through importation, reporting, or initial conditions, producing a posterior for the wrong dynamical system.

**Fix** — Make block transition rate mandatory in the grammar or AST:

```ocaml
type transition_body = {
  rate : expr option;
  guard : guard option;
  tag : string option;
}
```

After parsing, emit a hard `E26x` if `rate =` is missing. Do not use `0.0` as a continuation value for successful compilation.

**Severity** — Critical

### 4. Stratified initial conditions are not checked against expanded compartments

**Location** — `ocaml/lib/compiler/parser.mly:687-693`, `ocaml/lib/compiler/expander.ml:2883-2928`; no initial-condition checks in `ocaml/lib/ir/validate.ml:63-129` or `rust/crates/ir/src/validate.rs:55-165`

**Category** — user footgun; not wired through

**Defect** — `init { S = N0 }` is accepted even when `S` is stratified. `expand_init` emits an initial-condition entry named `S` instead of rejecting the bare stratified reference or expanding it. Neither the OCaml nor Rust IR validator checks that initial-condition names are real expanded compartments.

**Why it matters** — Initial conditions are one of the easiest ways to get a plausible but wrong epidemic. A stratified model can carry an initial value for nonexistent `S` while real cells such as `S_child_kano` default to zero. Depending on runtime handling, this either fails late or starts the epidemic in an empty population.

**Fix** — Reuse the indexed-reference resolver for init LHS. In init context:

* bare unstratified compartment: allowed
* bare stratified compartment: hard error
* fully indexed stratified compartment: allowed
* indexed-binding init such as `S[p in patch] = ...`: expand to all concrete cells
* every emitted init key must be validated against `expanded_comp_tbl`

Add initial-condition reference checks to both OCaml and Rust IR validators.

**Severity** — Critical

### 5. Scenario overrides and intervention enables are not validated

**Location** — `ocaml/lib/compiler/parser.mly:4-6`, `862-878`, `880-883`; `ocaml/lib/compiler/expander.ml:4432-4494`

**Category** — not wired through; user footgun

**Defect** — `extract_ident_list` returns `[]` for non-list values, so `enable = sia` silently disables nothing. `expand_scenarios` serializes `preset_enable`, `preset_disable`, `preset_params`, and `preset_scale` without checking that intervention names, family names, scenario names, or parameter names exist. Scenario `set` and `scale` keys are also not checked against declared parameters.

**Why it matters** — Interventions are inactive by default. A typo in `enable`, `disable`, `set`, or `scale` can make a vaccination-campaign scenario run as baseline or leave `beta`, `rho`, or coverage unchanged. That is a direct wrong counterfactual.

**Fix** — Make scenario fields closed and typed:

```ocaml
enable  : ident_list
disable : ident_list
compose : ident_list
set     : param_patch_map
scale   : param_patch_map
```

Reject non-list `enable`/`disable`/`compose`. Validate enable/disable entries against exact intervention names and `base_name` family names. Validate `set`/`scale` keys against scalar and expanded indexed parameters. For `scale`, enforce probability-domain checks at compile time when possible, as the spec requires.

**Severity** — Critical

### 6. Observation likelihood arguments are dimensionally unchecked

**Location** — `ocaml/lib/ir/dimcheck.ml:713-736`, `801-813`

**Category** — statistical correctness; user footgun

**Defect** — The dimchecker sets `st.permissive_dim <- true` around the entire observation likelihood pass. For `binomial` and `bernoulli`, it only infers `p`; it does not require `p` to be dimensionless/probability. For `poisson`, `neg_binomial`, and `normal`, it does not enforce meaningful dimensions for mean/rate/sd beyond negative-binomial dispersion.

**Why it matters** — `binomial(n = N_tested, p = projected)` compiles when `projected` is a count. The common missing `/N` error turns prevalence likelihood into a count-valued probability. If the backend clamps, saturates, or returns NaNs inconsistently, inference targets a measurement artifact rather than the surveillance model.

**Fix** — Remove blanket permissive mode. Add `check_likelihood_dims`:

* `binomial.p`, `bernoulli.p`: dimensionless and statically bounded to `[0,1]` when possible
* `binomial.n`, `beta_binomial.n`: count/integer-compatible
* `poisson.rate`, `neg_binomial.mean`: count/event-count dimension
* `normal.sd`: same dimension as `normal.mean`, positive
* `neg_binomial.r`, `beta_binomial.alpha`, `beta_binomial.beta`: positive dimensionless

If specific epidemiological variance formulas need relaxed dimensional algebra, isolate permissiveness to that expression, not the whole likelihood.

**Severity** — Critical

### 7. Invalid calendar dates compile as shifted real dates

**Location** — `ocaml/lib/compiler/expander.ml:99-114`, `111-128`, `4803-4806`

**Category** — user footgun; numerical correctness; FFI

**Defect** — `parse_iso_date` only splits the string and parses integers. It does not validate month range, day range, leap-day legality, or year bounds before `days_of_date` computes a day number. `date("2020-02-31")` is accepted and converted to a day offset.

**Why it matters** — Campaign schedules, origins, observation cutoffs, and reporting calendars can shift by days without an error. For SIA timing or AFP surveillance windows, that can move interventions across incidence peaks and change estimated campaign impact.

**Fix** — Validate dates before computing day numbers:

```ocaml
1 <= month <= 12
1 <= day <= days_in_month year month
```

Use the same validation for `origin`, `date(...)`, `add_calendar_*`, `date_range`, and data-boundary date parsing. Invalid dates must be a hard named diagnostic, not normalization.

**Severity** — Critical

### 8. Duplicate and cross-namespace names are silently overwritten or resolved in the wrong order

**Location** — `ocaml/lib/compiler/expander.ml:715-745`, `1857-1879`, `3955-3973`

**Category** — type design; user footgun

**Defect** — Lookup tables are built with `Hashtbl.replace`, so duplicate lets, duplicate forcing names, and duplicate table names can overwrite earlier declarations before validation. `resolve_ident_name` checks let bindings before compartments and parameters, while the spec requires compartments → parameters → lets → forcing → tables and an error on ambiguous names. `check_shadowing` only warns for let names matching stratum values.

**Why it matters** — A user can declare both `parameter N : count` and `let N = S + I + R`; expressions resolve to the let, not the parameter. A duplicate `let beta = ...` can override another let with no diagnostic. In a model where names carry epidemiological meaning, this changes equations while leaving the source visually plausible.

**Fix** — Add a declaration-name validation pass before `build_lookup_tables`:

* reject duplicates within every namespace
* reject ambiguous names across compartments, parameters, lets, forcing, and tables
* reserve `t`, `dt`, `origin`, `projected`, `sum`, `consecutive`, and `compartments` consistently
* stop using `Hashtbl.replace` until after uniqueness is proven

Then make `resolve_ident_name` match the spec's resolution order, with ambiguity impossible by construction.

**Severity** — Critical

## High findings

### 9. `c in compartments` does not fill omitted dimensions for partially stratified compartments

**Location** — `ocaml/lib/compiler/expander.ml:1985-2014`, `1937-1958`, `2124-2233`

**Category** — not wired through; user footgun

**Defect** — `IComp` iteration only substitutes base compartment names. `resolve_stoich_ref` then concatenates whatever indices were supplied. It does not inspect the selected compartment's actual dimension vector and does not cartesian-product omitted dimensions.

**Why it matters** — The spec's canonical partial-stratification pattern is:

```camdl
death[c in compartments, a in age] : c[a] --> @ mu * c[a]
```

If `R` has `[age, immunity]`, this must emit separate death transitions for `R[a, natural]` and `R[a, vaccine]`. The compiler instead forms partial names such as `R_child`, which are invalid or semantically wrong. Demography, migration, and aging can omit entire hidden dimensions.

**Fix** — During expansion of any stoichiometry reference produced by `c in compartments`, resolve the selected compartment's full dimension vector. Treat supplied indices as a partial binding and expand all omitted dimensions before emitting stoichiometry and rate expressions.

**Severity** — High

### 10. Bare stratified `transfer(from = S, to = V)` is not expanded over strata

**Location** — `ocaml/lib/compiler/expander.ml:2990-3017`, `3526-3586`

**Category** — not wired through; user footgun

**Defect** — `resolve_comp_name` requires `from =` and `to =` to resolve to `Ir.Pop`. For a stratified compartment, bare `S` resolves to `PopSum`, so the compiler errors instead of expanding the transfer over all cells. The spec says bare stratified transfers expand over all dimensions.

**Why it matters** — A national campaign written as `transfer(fraction = vacc_frac, from = S, to = V)` should vaccinate every susceptible stratum. The compiler forces hand enumeration or rejects the model. Hand enumeration of hundreds of cells is exactly the error-prone pattern the DSL is meant to eliminate.

**Fix** — Handle transfers with the same indexed-reference resolver:

* bare unstratified `S`: one transfer
* bare stratified `S` and compatible bare stratified `V`: one transfer per matching stratum tuple
* partially indexed `S[patch = p]`: one transfer per omitted stratum
* mismatched source/destination dimension sets: hard error with explicit routing requirement

**Severity** — High

### 11. Compile-time `if` over index variables is not implemented

**Location** — `ocaml/lib/compiler/expander.ml:1586-1589`

**Category** — not wired through; user footgun

**Defect** — Every `ECond` lowers to runtime `Ir.Cond`. There is no compile-time evaluation for conditions involving only index variables and constants, despite the spec's `let mig[i in patch, j in patch] = if i == j then 0.0 else ...` semantics.

**Why it matters** — Gravity kernels, self-loop exclusions, and block-diagonal mixing often use index-variable conditionals. With the current lowering, `i` and `j` are substituted to literal level names, then resolved as ordinary identifiers. That produces undeclared-name errors or, worse, binds to a parameter/compartment if a level name overlaps a declaration.

**Fix** — Add `try_eval_index_cond env expr` before runtime lowering. If the predicate contains only bound index variables, dimension levels, constants, and comparisons, fold the branch at expansion time. Emit runtime `Ir.Cond` only when the predicate actually depends on state, time, parameters, or forcing.

**Severity** — High

### 12. Inline table shape validation is missing

**Location** — `ocaml/lib/compiler/expander.ml:2762-2786`, `2788-2837`

**Category** — tests; user footgun; numerical correctness

**Defect** — Inline tables are flattened depth-first and emitted. The compiler does not validate nested rank, row lengths, or total cell count against the declared dimensions.

**Why it matters** — `C_age : age × age = [[12.0, 4.0, 8.0, 3.0]]` has the right flattened count but the wrong 2D shape; `C_age : age × age = [12.0, 4.0]` has the wrong count. Contact matrices and spatial kernels can be malformed without a compiler error at the table declaration site.

**Fix** — Validate inline nested structure recursively against the declared dimension sizes before flattening. Emit row/axis-specific diagnostics:

```text
table 'C_age': axis 1 expects 2 entries for dimension 'age', got 1
```

Keep flattening only after shape validation succeeds.

**Severity** — High

### 13. Output blocks are parsed but most of their content is discarded

**Location** — `ocaml/lib/compiler/parser.mly:625-665`; `ocaml/lib/compiler/expander.ml:2943-2976`

**Category** — not wired through; user footgun

**Defect** — The AST stores `out_trajectories`, `out_flows`, and `out_summary`, but `expand_output` only uses the trajectory `every` and `format`. Flow quantities, summary quantities, synthetic output, and output expressions are discarded. The parser also does not match the spec's nested `quantities { ... }` form.

**Why it matters** — A user can request `weekly_infections = incidence(infection)` or `total_cases = cumulative(infection)` and get no corresponding IR contract. Diagnostics and reported public-health quantities can disappear even though the source file appears to define them.

**Fix** — Either remove unsupported output syntax from the parser or extend the IR to carry:

```ocaml
trajectory_quantities : (string * expr) list
flow_quantities       : (string * projection_expr) list
summary_quantities    : (string * summary_expr) list
synthetic_config      : ...
```

Then validate every output expression with the same projection/index resolver used by observations.

**Severity** — High

### 14. `simulate` silently defaults to `from = 0`, `to = 100`

**Location** — `ocaml/lib/compiler/parser.mly:668-676`; `ocaml/lib/compiler/expander.ml:2932-2941`

**Category** — user footgun

**Defect** — The parser fills missing simulation fields with `0.0` and `100.0`, and `expand_simulate` also emits the same defaults when the whole block is absent.

**Why it matters** — A model can accidentally run for 100 model-time units instead of the intended campaign, surveillance, or forecast horizon. Cumulative cases, extinction probability, and intervention timing all depend on the horizon.

**Fix** — For runnable compilation, require an explicit `simulate` block with explicit `from` and `to`. For `camdl check`, allow omission only in validation mode and mark `t_start`/`t_end` unavailable. Do not serialize a default horizon into IR unless a CLI layer explicitly requested it.

**Severity** — High

## Medium findings

### 15. `time_unit` accepts non-time units and can crash unit conversion

**Location** — `ocaml/lib/compiler/parser.mly:63-65`, `117-128`; `ocaml/lib/compiler/expander.ml:631-640`, `656-663`

**Category** — user footgun; FFI

**Defect** — Top-level `time_unit =` uses the general `unit_lit` grammar, which accepts `'count` and `'ratio`. Later, `unit_to_model_time` calls `days_per ctx.time_unit`, and `days_per Count` / `days_per Ratio` raises `invalid_arg`.

**Why it matters** — A malformed model should produce a domain diagnostic, not an uncaught compiler exception. It also violates the IR contract that `time_unit` is a time scale.

**Fix** — Split the grammar:

```ocaml
time_unit_lit := 'days | 'weeks | 'months | 'years
unit_lit      := time_unit_lit | rate_unit | 'count | 'ratio
```

Also validate `ctx.time_unit` once after declaration collection and before any conversion.

**Severity** — Medium

### 16. Tests do not cover the spec's dangerous front-end surfaces

**Location** — `ocaml/golden/*`; absence of tests for named indexing, partial indexing, inline table shape failures, scenario typo failures, stratified bare init failure, and observation likelihood dimension failures

**Category** — tests

**Defect** — Existing goldens exercise many happy paths, but the failure modes above are not locked down. Several are exactly the mutation tests the suite should catch: delete a `rate`, under-index a table, typo a scenario parameter, use `binomial(p = I)`, or write `S[patch = p]` in a multi-dimensional model.

**Why it matters** — These are not style regressions. They are pathways to wrong likelihoods, wrong intervention scenarios, and wrong initial states. A future patch can reintroduce them unless they become golden error fixtures and property tests.

**Fix** — Add negative golden tests for:

* block transition missing `rate`
* `C_age[child]` for a 2D table
* inline table wrong count and wrong row length
* `init { S = ... }` with stratified `S`
* `enable = sia` and `enable = [sia_typo]`
* `set = { betta = 0.3 }`
* `binomial(p = I)`
* `incidence(infection[patch = p])` in a patch × age model
* `S[sex = female, age = child]`

Add one positive golden for named indexing and omitted-dimension summation.

**Severity** — Medium

## Structural fix that removes several defects at once

The compiler needs a typed, central lowering layer between AST and IR:

```ocaml
type namespace =
  | compartment
  | transition_flow
  | table
  | parameter
  | let_binding
  | forcing

type resolved_ref =
  | OnePop of string
  | PopSum of string list
  | OneFlow of string
  | FlowSum of string list
  | TableCell of string * int
  | Param of string
  | LetExpansion of Ir.expr
  | TimeFunc of string
```

All current string-mangling paths should disappear behind this resolver. That one change directly fixes the named-indexing bug, partial-indexing bug, observation projection bug, table lookup arity bug, indexed parameter dimension bug, stratified transfer bug, and most partial-stratification expansion bugs. The current compiler is too permissive where it must be strict, and too stringly-typed where the spec requires dimension-aware semantics.
