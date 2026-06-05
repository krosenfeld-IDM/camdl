# CLI UX: unify chain-start under `--init`; unify value-setting under `--fixed`; single resolver

Date: 2026-05-25 Author: vsb Status: draft for review — revision 2
(post-discussion) Related: gh#83 (init from_prior / from_posterior), gh#85
(--params split semantics)

> **Casing note (post-implementation, 2026-05-26).** Earlier drafts of this
> proposal spelled the init mode names in kebab-case (`--init from-mle`, etc.).
> The shipped surface (steps 6 + 7) uses **snake_case** to match the in-tree
> `InitMethod` serde deserializer (`from_mle`, `from_prior`, `from_posterior`,
> `from_params`, `survey_top_k`). The deserializer is the truth; the proposal's
> prose has been updated to match in the load-bearing sections (§"`--init`
> family" and §"Migration" actionable-error text — these are the two places the
> shipped surface is documented verbatim). Other historical mentions of
> kebab-case mode names lower in this document are preserved as a record of the
> design discussion. See commit `cb47ee1` (step 12a) for the deserializer
> rename.
>
> **Display-side inconsistency to fix separately.** The shipped
> `impl Display for InitMethod` in `rust/crates/cli/src/fit/init.rs` still
> renders the warm-start variants in kebab-case (`from-prior`, `from-posterior`,
> `from-mle`, `from-params`) while emitting `single` / `uniform` / `lhs` /
> `survey_top_k` in the matching CLI form. The §"Provenance" JSON examples below
> (which use `to_string()` for `init_provenance.method`) therefore describe what
> the code actually emits today. Aligning Display with the CLI/deserializer
> (snake_case throughout) is a separate small fix tracked outside this proposal.

## Class

**doc-vs-code + code-vs-code**:

- Help text on `--params` claims one thing while the code does another (gh#85).
- Three separate value-resolution code paths exist (`util::resolve_run_model`,
  `FixedParams::resolve_with_model`,
  `priors_precedence::resolve_priors_with_precedence`); each is correct for its
  slice but drift-prone and used inconsistently.
- Cross-subcommand naming of the same concept is inconsistent (`--init` vs
  `--init-method`, `--starts` vs `--chains`, `--starts-from` vs nothing).

## TL;DR

Rev 2 collapses parameter-related flags into two verbs that mean the same thing
on every subcommand:

- **`--fixed`** — _these parameters are set to these values_. On non-inference
  subcommands all values are effectively fixed (no inference is happening); on
  inference subcommands the named parameters are pinned out of `[estimate]`.
- **`--init`** — _where do chains start from_ (inference only). The mode family
  expands to cover prior / posterior / single-file warm starts (gh#83).

`--params` and `--param` are removed everywhere. `--init-method` and
`--starts-from` (fit run) are renamed for parity.

A single `ParameterResolver` abstraction owns the precedence chain and replaces
the three half-resolvers in the codebase. Every subcommand routes through it.
Provenance (where each value came from) is recorded into the resolver's output
so `run.json` can faithfully serialize "this value came from `--fixed`, that one
from `fit.toml [fixed]`, that one from a scenario preset."

## Audit: current state

Verified by reading `rust/crates/cli/src/args/mod.rs`,
`rust/crates/cli/src/util.rs:803-1004`,
`rust/crates/cli/src/fit/config_v2.rs:574-697`,
`rust/crates/cli/src/fit/priors_precedence.rs`, and the per-subcommand
`<sub>.rs` files.

### Flag table

| Subcommand | `--params` | `--param` | `--fixed`  | `--fit` | `--init`        | warm-start      |
| ---------- | ---------- | --------- | ---------- | ------- | --------------- | --------------- |
| `simulate` | fix        | fix       | —          | —       | —               | —               |
| `pfilter`  | fix        | fix       | —          | —       | —               | —               |
| `eval`     | fix        | fix       | —          | —       | —               | —               |
| `if2`      | mixed      | mixed     | names      | —       | — (`--rw-sd`)   | —               |
| `profile`  | mixed      | mixed     | names      | yes     | `--init <mode>` | (gh#74-A WIP)   |
| `survey`   | —          | —         | NAME=VALUE | yes     | —               | —               |
| `fit run`  | —          | —         | (toml)     | (toml)  | `--init-method` | `--starts-from` |

Three layers of inconsistency:

1. `--params` carries `(fix)` semantic on non-inference but
   `(mixed: start-vs-fix-per-param)` on inference. (gh#85)
2. `--fixed` itself takes name-only form on `profile`/`if2` but `NAME=VALUE`
   form on `survey`. (audit-discovered, not yet filed)
3. Init / warm-start vocabulary differs across commands (`--init` vs
   `--init-method`; `--starts` vs `--chains`; `--starts-from` vs nothing).

### Resolver fragmentation

| Resolver                          | Lives in                                                   | Used by                                      |
| --------------------------------- | ---------------------------------------------------------- | -------------------------------------------- |
| `resolve_run_model`               | `cli/util.rs:803-1004`                                     | `simulate`, `lineage`                        |
| `FixedParams::resolve_with_model` | `cli/fit/config_v2.rs:574-697`                             | `survey`, `profile` (for fit-toml `[fixed]`) |
| `resolve_priors_with_precedence`  | `cli/fit/priors_precedence.rs`                             | `profile`, `fit run` (for priors)            |
| inline per-subcommand resolution  | `profile.rs:437-453`, `if2.rs:109-168`, `pfilter.rs:47-55` | profile, if2, pfilter                        |

Each is correct on its own. Together they let small details drift silently — the
spec-documented precedence in `docs/camdl-run-spec.md §1.3` is enforced only in
`resolve_run_model`; profile and if2 implement a _similar_ order inline but the
next edit might or might not preserve it.

## Problems, in priority order

### P1. `--params` on inference subcommands is a footgun (gh#85)

Verified at `cli/util.rs:493-501`. `apply_params_file` sets `p.value = Some(v)`
indiscriminately; the role (fix vs start) is decided downstream by `[estimate]`
membership. User reading `--params truth.toml` reasonably expects "fix these";
in profile context, parameters that happen to be in `[estimate]` walk off during
PMMH instead.

### P2. `--init` is bounds-only (gh#83)

`InitMethod` (`cli/fit/init.rs:41`) has four variants — `single`, `uniform`,
`lhs`, `survey_top_k`. All sample from bounds; none sample from prior shape or
posterior draws. Important warm-start patterns (prior visualisation, posterior →
posterior handoff between fits, profile cells starting from a wa-fit's
posterior) are not expressible.

### P3. `--fixed` itself is inconsistent across subcommands

`--fixed` on `profile`/`if2` takes a comma-list of _names_ (pin at model default
value); on `survey` takes `NAME=VALUE` pairs (pin at explicit value). The same
flag with the same name behaves differently. Easy to miss.

### P4. Resolver fragmentation lets precedence drift

Three half-resolvers plus inline per-subcommand reimplementations of the same
precedence rules. Provenance ("where did this value come from?") is partial —
only the priors resolver currently records it.

## Proposal

### Principle

Two verbs, two concepts, one resolver:

- **`--fixed`** — _these parameters are set to these values_
- **`--init`** — _where do chains start from_ (inference only)
- One `ParameterResolver` abstraction implements precedence; every subcommand
  routes through it.

### `--fixed` semantics, defined once

Universal form, all subcommands:

```
--fixed NAME=VALUE          # repeatable; explicit value form
--fixed-file <toml>         # repeatable; layered, later overrides earlier
```

Both accept the same value vocabulary. Name-only `--fixed NAME` form is
**removed** — the "pin at model default" case is just "don't list the param at
all and let the model default flow through the precedence chain." Removing the
form costs nothing and removes a per-subcommand inconsistency.

On non-inference subcommands (`simulate`, `pfilter`, `eval`), `--fixed` is the
sole flag for setting parameter values. Every listed value is used as the
parameter's runtime value. (No inference is happening; all values are trivially
"fixed" in the "set and not varying" sense.)

On inference subcommands (`profile`, `if2`, `fit run`), `--fixed` both sets a
value _and_ removes the parameter from the `[estimate]` set if present:

```
camdl profile model.camdl --fit fit.toml --fixed gamma=0.1 --sweep tau=lin(-35,-1,30)
```

Resolves as: estimated set = `(fit.toml [estimate]) − {gamma, tau}`; `gamma` is
pinned at 0.1; `tau` is swept along the grid. The likelihood landscape is a
slice through `(tau, gamma=0.1)`-space.

This is the natural ergonomic for profile-likelihood slicing — "hold gamma at a
specific value while sweeping tau" is exactly what `--fixed gamma=0.1` should
mean. The alternative (error on collision with `[estimate]`) is more defensive
but obstructs the canonical workflow.

### `--init` family

Same name, same modes, every inference subcommand:

```
--init single                       # all chains at the seeded base params
--init uniform                      # per-chain U(lo, hi) within bounds
--init lhs                          # Latin-hypercube stratified, scale-aware
--init from_prior                   # per-chain draw from each parameter's `~ <dist>`
--init from_posterior --posterior <path>
                                    # per-chain draw from a posterior draws TSV
                                    #   (accepts <draws.tsv> OR a fit-results <dir>)
--init from_params --params <toml>  # all chains at the point given by a flat params TOML
                                    # (e.g. a hand-written truth.toml or a warm-start file
                                    # the user maintains). Top-level keys are parameter
                                    # names → values. Replaces the inference-context
                                    # `--params` use case explicitly.
--init from_mle --mle <path>        # all chains at the MLE point from a prior fit;
                                    # accepts a fit-results <dir> (auto-resolving the
                                    # canonical `mle.toml` or `final_params.toml`) OR a
                                    # specific MLE-shape TOML file. Knows about the
                                    # fit-output schema: skips `[provenance]`,
                                    # `[focal]`, `final_loglik`, etc., and reads the
                                    # parameter values from the section that holds them.
                                    # Formalises today's `--starts-from <fit-dir>` for
                                    # fit run.
--init survey_top_k --survey-path <dir>
                                    # existing behaviour (snake_case to match the
                                    # in-tree InitMethod deserializer)
```

`from_params` and `from_mle` are kept as **distinct verbs**, not collapsed into
a generic `from_point`. They look operationally similar ("load one point from a
file"), but the _file contracts_ differ:

- `from_params` expects a **flat** TOML where top-level keys are parameter
  names. This is the schema a model author writes by hand for a truth value, a
  hand-tuned warm start, or a posterior median exported from another tool.
- `from_mle` expects a **structured** TOML produced by a camdl fit. The
  `[focal]`, `[mle]`, `[provenance]`, `final_loglik` fields are not bureaucracy
  — they encode which parameter was swept, what the loglik at convergence was,
  and which camdl version produced the file. A loader that flattens these into
  one bag either silently corrupts the data (turning `[mle] R0 = 25` into
  `mle_R0`) or forces fit output to drop its structure to fit a general loader.
  Both are bad.

Verb-per-source means each loader knows its specific shape and dispatches on the
user's stated intent. This is also the right posture for adding new sources
later (e.g. `from_stan_fit
<csv>`, `from_arviz <netcdf>`) without retrofitting a
generic "point loader" to accommodate every schema we'd ever support.

Init applies _only_ to parameters in the `[estimate]` set after `--fixed`
resolution. Parameters in `--fixed` or absent from `[estimate]` take their
resolved value regardless of init mode.

When a `from_posterior`, `from_mle`, or `from_params` source file is missing
parameters that the current fit's `[estimate]` set includes, fall back to the
subcommand's default init mode for those columns. Emit a startup warning naming
the missing parameters. When the source file contains parameters not in
`[estimate]`, ignore those columns silently — they are either fixed or absent
from the model.

### Cross-subcommand renames

- `fit run`'s `--init-method` → `--init`. Parity with `profile`.
- `fit run`'s `--starts-from <dir>` → `--init from_mle --mle <dir>` (the
  existing behaviour is exactly MLE warm-start from a fit output dir; the new
  verb names this correctly).
- `profile`'s `--starts` (per-cell count) stays as is.
- `if2`'s `--chains` stays as is — established MCMC vocab.

### fit.toml schema (paired toml renames)

The CLI changes have toml-side counterparts. The principle is that toml keys and
CLI flags share names — divergence here was the source of the audit's `--init`
vs `--init-method` confusion.

| Today (toml)                           | Rev 2 (toml)                                              | Note                      |
| -------------------------------------- | --------------------------------------------------------- | ------------------------- |
| `[stages.<n>] init_method = "lhs"`     | `[stages.<n>] init = "lhs"`                               | matches CLI `--init`      |
| `[stages.<n>] starts_from = "<stage>"` | `[stages.<n>] init = "from_mle"` + `init_mle = "<stage>"` | one key per concept       |
| `[fixed] foo = 1.0`                    | unchanged                                                 | already correct semantics |
| `[estimate] foo = { ... }`             | unchanged                                                 | already correct semantics |

Every fit.toml in the repo gets the two keys renamed atomically; this is ~70
files (vignettes/, golden fixtures, tests). Old keys produce a clap-style error
at config-load with the replacement spelled out.

The toml-key rename is what the audit's P3 ("naming drift") is really about —
CLI and toml drifted because they were edited separately. Going forward, any new
CLI flag in this family requires a matching toml key with the same kebab-cased
name.

### The single resolver — `ParameterResolver`

Replaces `resolve_run_model`, `FixedParams::resolve_with_model`, the inline
resolvers in `profile.rs` / `if2.rs` / `pfilter.rs`, and (optionally — separate
concern) sits next to `resolve_priors_with_precedence`.

#### Shape

```rust
/// Inputs gathered from the CLI + IR. Every subcommand assembles
/// one of these before dispatch; the resolver returns a
/// `ResolvedParameters` carrying the per-parameter outcome plus
/// provenance.
pub struct ParameterInputs<'a> {
    pub model:           &'a ir::Model,
    pub scenario:        Option<&'a str>,        // model preset name
    pub fixed_cli:       &'a [(String, f64)],    // --fixed NAME=VALUE
    pub fixed_files:     &'a [PathBuf],          // --fixed-file <toml>...
    pub fit_toml_fixed:  &'a IndexMap<String, f64>, // [fixed] block of --fit
    pub fit_toml_estimate: &'a IndexSet<String>, // names in [estimate]
    pub table_files:     &'a HashMap<String, PathBuf>, // --table NAME=FILE
}

#[derive(Debug, Clone)]
pub enum ValueSource {
    ModelDefault,
    Scenario(String),       // preset name
    FitTomlFixed,
    FixedFile { path: PathBuf },        // carries the file that won (under layering)
    FixedCli,
}

/// Resolver-decided role for a parameter. ADT-shaped rather than
/// a `bool fixed` field so the *reason* a parameter ended up
/// fixed is first-class — the run.json provenance distinguishes
/// "never in [estimate]" from "was in [estimate], --fixed kicked it
/// out", which matters for auditing whether a profile-likelihood
/// slice did what the user intended.
pub enum ParameterRole {
    Fixed { reason: FixReason },
    Estimated,
}

pub enum FixReason {
    NotInEstimate,                                  // never was in [estimate]
    KickedFromEstimate { by: ValueSource },         // was in [estimate]; --fixed kicked it
}

pub struct ResolvedParameter {
    pub name:   String,
    pub value:  f64,
    pub source: ValueSource,
    pub role:   ParameterRole,
}

pub struct ResolvedParameters {
    pub params:       Vec<ResolvedParameter>,
    pub estimate_set: IndexSet<String>,  // names with role=Estimated, in declaration order
    pub model:        ir::Model,         // mutated to carry the resolved .value fields
    pub warnings:     Vec<ResolverWarning>,
}

pub enum ResolverWarning {
    KickedFromEstimate    { name: String, by: ValueSource },
    UnknownParam          { name: String, source: ValueSource, did_you_mean: Vec<String> },
    BoundsViolation       { name: String, value: f64, lo: f64, hi: f64 },
    /// Scenario set `name` to `scenario_value`, but `by`
    /// (a higher-precedence source) overrode it to `new_value`.
    /// Surfaced on stderr at resolve time and into run.json's
    /// `overrode_scenario` field. Not an error — CLI override
    /// of a scenario value is a legitimate quick-test workflow;
    /// the warning is so the override is never silent.
    ScenarioOverridden    { name: String, scenario: String,
                            scenario_value: f64, by: ValueSource, new_value: f64 },
    /// fit-toml `[fixed]` and `[estimate]` both name the same
    /// parameter. The resolver treats `[fixed]` as winning (a
    /// parameter that is both fixed and estimated is a config
    /// bug, but the conservative interpretation is "the user
    /// meant fixed"). Surfaced so the user fixes their toml.
    FixedEstimateOverlap  { name: String },
}

pub enum ResolveError {
    UnknownParameter     { name: String, source: ValueSource, candidates: Vec<String> },
    NonFiniteValue       { name: String, value: f64, source: ValueSource },
    UnsetRequired        { name: String },           // no model default, no override
    SchemaMismatch       { path: PathBuf, msg: String },
    ScenarioNotFound     { name: String, available: Vec<String> },
    ExternalTableMissing { table: String },
}

pub fn resolve_parameters<'a>(
    inputs: ParameterInputs<'a>,
) -> Result<ResolvedParameters, ResolveError>;
```

`ParameterRole` is the load-bearing ADT: a parameter is either
`Fixed { reason: ... }` or `Estimated`, never both, never neither. Downstream
consumers can pattern-match exhaustively rather than encoding the same logic as
`if param.fixed && param.was_in_estimate
{ ... }` branches scattered across the
codebase. The compiler enforces that every new code path handles both cases.

#### Precedence (last wins)

The order is **the documented spec** in
[`docs/camdl-run-spec.md §1.3`](../../camdl-run-spec.md):

1. Model parameter default (`p.value` from DSL)
2. `fit.toml [fixed]` block (when present)
3. `--fixed-file <toml>` (each file layered in order; later overrides earlier)
4. Scenario preset (`preset.params` for the active scenario)
5. `--fixed NAME=VALUE` (highest)

The key non-obvious point — and the one that an earlier draft of this proposal
got wrong — is **scenario beats `--fixed-file`** (and beats fit-toml `[fixed]`).
The rationale is that scenarios travel with the model: they are named, audited
configurations declared inside the `.camdl` file, and choosing a scenario is a
deliberate "use this whole bundle." A user-supplied params file is a _base of
values to start from_; the scenario then refines them per the model author's
intent. CLI `--fixed` remains the highest precedence so a "quick test" override
is always expressible (`--scenario worst_case --fixed beta=0.5` works exactly as
expected — worst_case applies, then `beta=0.5` wins).

This order is enforced by the test
`scenario_runtime_application::scenario_set_replaces_mu_value` and is the order
the existing `resolve_run_model` already implements (lines 932-950 of
`cli/src/util.rs`). The new resolver preserves it byte-for-byte.

`[estimate]` membership rule:

- Start: `estimate_set = inputs.fit_toml_estimate`
- Remove every name that appears in (3) or (5) — these are user-explicit "pin
  this" assertions and take precedence over the toml's `[estimate]` block.
- Emit a warning (not an error) for each such removal, naming the parameter and
  the source: `"--fixed gamma=0.1 removes gamma from [estimate]"`.
- On non-inference subcommands, `inputs.fit_toml_estimate` is empty; the
  kick-out logic is a no-op.

#### Scenario-override visibility

CLI `--fixed` overriding a scenario's value is a legitimate quick-test workflow,
but it should never be _silent_. Six months later when re-reading a run, the
user needs to know whether the scenario's value was actually applied or quietly
overridden.

The resolver emits a `ScenarioOverridden` warning at resolve time whenever the
final winning source is `--fixed-file` or `--fixed-cli` AND the active scenario
also set the same parameter to a different value:

```
[info] --fixed beta=0.5 overrides scenario 'worst_case'
       which would have set beta=0.3
```

Run-time provenance records both values:

```json
"beta": {
  "value": 0.5,
  "source": "fixed_cli",
  "role": "fixed",
  "overrode_scenario": {
    "scenario": "worst_case",
    "scenario_value": 0.3
  }
}
```

Cost in the resolver: comparison against the scenario's intended value (already
iterated by the resolver) before writing the final winner. ~20 lines.

A `--strict-scenario` mode that escalates this warning to a hard error is
deferred — file a follow-up gh# if the slicing workflow surfaces a confusing
case. The warning + provenance combination is the right default; turning it into
an error is a one-line policy on top.

#### Validation, post-resolution

- Every parameter must end with a finite value. Unset parameters with no model
  default → error.
- Bounds checks (`validate_parameter_values`) run on the resolved values, as
  today.
- External tables are resolved here too (per `resolve_run_model:977-993`).
- Names appearing in `--fixed` / `--fixed-file` / `fit.toml` that don't exist in
  the model are an error with a "did you mean" hint built from the model's
  parameter list.

#### Provenance into `run.json`

Each `ResolvedParameter` carries its `ValueSource`. The subcommand-specific
`run.json` writer renders this as a `parameters_provenance` block:

```json
"parameters_provenance": {
  "beta":  { "value": 0.42, "source": "model_default" },
  "gamma": { "value": 0.10, "source": "fixed_cli",     "kicked_from_estimate": true },
  "rho":   { "value": 0.07, "source": "fit_toml_fixed" }
}
```

This is the gh#73 / gh#75 provenance work, generalised to all values (not just
priors).

### Why one resolver, not three

The three existing resolvers each cover a slice:

- `resolve_run_model` — values, but no `[estimate]` interaction (it's the
  simulate path).
- `FixedParams::resolve_with_model` — fit-toml `[fixed]`, but doesn't see CLI
  `--fixed`.
- `resolve_priors_with_precedence` — priors only.

The seam is at "values that come from the model + scenario + toml + CLI." That's
one operation. Splitting it across three codepaths means three places to keep
the same precedence in sync, and inline reimplementations whenever a new
subcommand appears. One resolver, one set of tests, one provenance shape.

Priors stay in `priors_precedence.rs` — they have a different content type
(`Prior`, not `f64`) and a different precedence shape (fit_toml > model_ir >
flat_fallback, not the 5-tier value chain). Two resolvers (values + priors),
both rigorous, beats five sloppy ones.

## Migration

camdl is alpha. CLAUDE.md alpha posture: "Backwards compatibility is a
non-goal." Recommend **M-1 (hard break)** — confirmed by the conversation that
produced this revision:

- Remove `--params` and `--param` from all subcommand argument structs
  (`SimulateArgs`, `PfilterArgs`, `If2Args`, `ProfileArgs`, `EvalArgs`).
- Remove the name-only form of `--fixed`. Require `NAME=VALUE`.
- Rename `fit run`'s `--init-method` → `--init`.
- Remove `--starts-from`; users must write `--init from_mle --mle <dir>` (or
  `from_params --params <toml>` for a hand-written warm-start file).
- Old invocations error with an actionable message:
  ```
  error: --params is no longer accepted. Replacement:
    --fixed NAME=VALUE             (set & freeze specific values)
    --fixed-file <toml>            (load fixed values from a TOML file)
    --init from_params --params <toml>   (warm-start from a hand-written params TOML)
    --init from_mle --mle <fit-dir>      (warm-start from a prior fit's MLE)
  ```
- Update camdl-book chapters, blog draft, examples in `--help` (`after_help`
  strings in `args/mod.rs`), `docs/user-features.md`, `docs/dsl-cheatsheet.md`.

## Blast radius

Estimate from a downstream doc-agent audit pass over both `camdl` and
`camdl-book` repositories, in addition to fit-toml fixtures and vignettes:

| metric                                                          | total                               |
| --------------------------------------------------------------- | ----------------------------------- |
| Mechanical renames (just sed)                                   | ~200 sites across both repos        |
| Semantic re-reads (profile/if2 with `--params`)                 | ~10 sites                           |
| Multi-paragraph prose rewrites (load-bearing — see below)       | 4 sections                          |
| TOML field renames (`init_method`, `starts_from`)               | ~70 files                           |
| Name-only `--fixed name1,name2` → explicit (needs value lookup) | ~15 sites                           |
| Effort estimate                                                 | > 2h; real prose work, not pure sed |

The 4 load-bearing prose sections cannot be renamed in place — their pedagogy
depends on the old flag shape and has to be rewritten under the new model:

1. **`camdl-book/CLAUDE.md:642-661` — the synthetic-recovery rule.** Current
   text reads, in essence: "use `pfilter --params` to evaluate the likelihood at
   truth without leakage; do _not_ use `profile --params` because the value
   walks." Under rev 2 this becomes "`pfilter --fixed-file` for evaluation;
   `profile --init from_params --params` (hand-written) or
   `profile --init from_mle --mle` (prior fit) for inference warm-start" — same
   teaching point, completely different flag pair.
2. **`camdl/docs/camdl-language-spec.md:2960-3001` (and the book mirror at
   `language/spec.qmd:2849-2890`).** A multi-paragraph block explaining that
   name-only `--fixed "N0,mu,k"` is the documented surface. Removing the
   name-only form is a _feature removal_ dressed as a naming cleanup; the
   replacement pattern (`--fixed-file` for many params, explicit `NAME=VALUE`
   for few) needs an explicit "the equivalent is now …" callout.
3. **`camdl/docs/inference.md:654-665` — the four-way precedence list.**
   Currently enumerates `--params`, `--fit`, `--fixed`, fit-toml `[fixed]`.
   Under rev 2, `--params` disappears entirely; the list collapses to three
   sources (CLI `--fixed{,-file}`, `--fit` toml `[fixed]`, scenario) plus the
   model default. Full rewrite, not a search-and-replace.
4. **`vignettes/he2010*/Makefile FIXED_PARAMS`.** Current value:
   `FIXED_PARAMS=mu,iota,sigma_se,cohort,rho,psi,e0,i0,N0` — nine names, no
   values. This is precisely the name-only-`--fixed` pattern. Migration requires
   either looking up each value (and committing them to the Makefile) or
   extracting them into a `vignettes/he2010/fixed.toml` and passing
   `--fixed-file`. The latter is cleaner; standardise on it as the recommended
   pattern for vignettes that fix many parameters.

These four sections together are ~1–2 hours of careful writing on top of the
mechanical work. The implementation outline below calls them out as
deliverables, not as line items in a generic "update docs" bullet.

### camdl-book coordination

The seed-timing/draft.qmd chapter has 3 `--params` hits (lines 304, 935, 938).
Line 935 is the `profile`-tau command in the WA-weak section, which is actively
being rendered. Under M-1, the chapter command examples must be updated in
lockstep with the camdl release — render after rewrite, not before.

If the chapter render schedule cannot tolerate a coordinated update, M-2
(one-release deprecation alias) buys time at the cost of carrying a `--params`
shim through the next minor release. Otherwise M-1 stays as the recommended
path.

## Addendum (post-step-5 implementation discovery): partial resolution

The proposal's §"Special handling for main.rs partial-resolution helpers"
claimed `prepare_cas_ctx` (the CAS cache-key builder) could route through the
unified resolver "with the right slots set to empty — the resolver handles this
correctly." That turned out to be wrong, and surfaced a real architectural shape
worth documenting.

`prepare_cas_ctx` deliberately applies `--params` + `--param` **but withholds
the scenario** because scenario is the other half of the CAS cache key (the hash
is computed _over_ base params, then scenario is applied separately to produce
the final simulation context). The unified resolver, by design, validates
`UnsetRequired` immediately — a parameter with no resolved value at the end of
the precedence chain is an error. This is the correct default for normal
subcommand flow but is incompatible with partial-resolution callers like
`prepare_cas_ctx`, because those callers know that _some_ parameters will be
filled in later (by the scenario half) and don't want the resolver to reject the
model before that happens.

The reproduction was caught by
`cas_integration::cas_first_run_writes_cache_and_metadata`: the test model
declares parameters whose values come from the scenario only (no DSL default, no
`--params`); migrating `prepare_cas_ctx` to call `resolve_parameters` made the
test fail with `parameter 'beta' has no value: ...`.

**Resolution (shipped in commit `2b419bd`):** `prepare_cas_ctx` keeps
`util::apply_params_file` as its value loader and is documented as the _only_
legitimate non-resolver writer of `model.parameters[i].value`. The audit
checklist's item 1 (sole writer) is amended: the resolver is the sole writer _on
the normal subcommand flow_; `prepare_cas_ctx` is an explicit exception with an
inline comment naming the test that pins it.

**Future direction (not in this proposal's scope):** A clean long-term fix is to
give the resolver a `ResolveValidation` mode on `ParameterInputs`:

```rust
pub enum ResolveValidation {
    Strict,              // current default — UnsetRequired errors
    PartialAllowed,      // params without a value are allowed; reported but not errors
}
```

`prepare_cas_ctx` would pass `PartialAllowed` and get the provenance benefits of
the resolver without the strict-validation incompatibility. This is a follow-up
RFC, not a blocker for the rest of the rev 2 migration.

A second case discovered alongside this one: `generate_prior_draws_from_ir`
accepts a `&[&str]` list of scenarios applied left-to-right, distinct from the
resolver's `Option<&str>` single-scenario API. Multi-scenario composition is a
separate semantic from compose-block scenarios and would require either a new
resolver entry point (`resolve_parameters_multi_scenario`) or a CLI restriction
to single-scenario. Deferred for maintainer triage.

## What this proposal does NOT touch

- The fit-toml schema (`[estimate]`, `[fixed]`, `[data]`, `[model]`) is
  unchanged.
- Priors resolution (`priors_precedence.rs`) is unchanged.
- `--fit`, `--data`, `--scenario`, `--enable`/`--disable`, `--table` are
  unchanged.
- The forward-sim precedence order documented in `docs/camdl-run-spec.md §1.3`
  is preserved exactly — the resolver is a refactor, not a semantic change to
  the order.

## Implementation outline

1. **Resolver first.** Write `ParameterResolver` in `cli/src/params_resolver.rs`
   (or `cli/src/resolve.rs`). Port `resolve_run_model`'s logic into it; add the
   `[estimate]` kick-out and provenance. Cover with unit tests for each
   precedence layer (model default, scenario, fit-toml-fixed, --fixed-file,
   --fixed-cli) and collision/kick-out cases.
2. **Migrate `simulate` / `lineage`** to use it. These two are the simplest — no
   `[estimate]`. Confirm `resolve_run_model` becomes a thin shim or is deleted.
3. **Migrate `pfilter` / `eval`.** Same shape as simulate.
4. **Migrate `survey`.** The `FixedParams::resolve_with_model` path gets folded
   into the unified resolver via `inputs.fit_toml_fixed`.
5. **Migrate `if2` / `profile` / `fit run`.** These pick up the `[estimate]`
   kick-out semantics. Replace inline resolvers with `resolve_parameters` calls.
6. **Init family.** Implement the four new `InitMethod` variants — `FromPrior`,
   `FromPosterior`, `FromMle`, and `FromParams` — each with its own
   loader/sampler that knows the file shape it expects. `SurveyTopK` already
   exists. `FromPosterior` ships in this series (gh#83 explicitly couples
   prior + posterior — splitting would leave a worse state than today, where
   neither warm-start mode is cleanly supported across subcommands).
7. **CLI surface.** Add `--fixed-file`, `--posterior`, `--mle` flags. Remove
   `--params`, `--param`, name-only `--fixed`, `--starts-from`, `--init-method`.
8. **Help text.** Single normative `--init` block shared via clap `long_about`.
   Single normative `--fixed` block. Update all `after_help` examples in
   `args/mod.rs`.
9. **Provenance into `run.json`.** Add `parameters_provenance` block; extend
   `run_meta.rs` / `RunMeta` schema.
10. **Doc churn — mechanical.** `user-features.md`, `dsl-cheatsheet.md`,
    `camdl-run-spec.md §1.3` (precedence chain stays, flag names change), the
    alpha blog draft, and ~200 sed-equivalent rename sites across both
    repositories per the Blast radius table.
11. **Doc churn — load-bearing prose rewrites.** Four named sections from the
    Blast radius audit must be hand-rewritten, not renamed:
    - `camdl-book/CLAUDE.md:642-661` (synthetic-recovery rule)
    - `camdl/docs/camdl-language-spec.md:2960-3001` (name-only `--fixed` removal
      — feature-removal callout)
    - `camdl/docs/inference.md:654-665` (4-way precedence list → 3-source
      restructure)
    - `vignettes/he2010*/Makefile` `FIXED_PARAMS` migration (recommend
      extracting to `fixed.toml` + `--fixed-file` as the canonical pattern for
      many-fixed-param vignettes)
12. **fit.toml fixture migration.** ~70 files with `init_method = "..."` /
    `starts_from = "..."` keys; rename atomically to `init = "..."` /
    (`init = "from_mle"` + `init_mle = "<stage>"` for the `starts_from` case).
    Old keys produce an actionable error at config-load.

Rough sizing: 1000–1500 lines including tests. The resolver itself is ~300
lines; the per-subcommand wiring + flag plumbing is ~400; tests and doc updates
account for the rest.

## Init phase types

The init phase runs only on inference subcommands and produces chain starting
points for parameters in the resolver's `estimate_set`. Fixed parameters take
their resolved value regardless of init mode — they are not in the domain of the
init draw.

```rust
pub enum InitMethod {
    Single,
    Uniform,
    Lhs,
    FromPrior,
    FromPosterior { source: PosteriorSource },
    FromMle       { source: MleSource },
    FromParams    { path: PathBuf },
    SurveyTopK    { source: SurveySource, k: usize },
}

pub enum PosteriorSource {
    DrawsTsv(PathBuf),
    FitDir(PathBuf),     // auto-resolves to <dir>/draws.tsv
}

pub enum MleSource {
    File(PathBuf),       // an mle.toml or final_params.toml directly
    FitDir(PathBuf),     // auto-resolves: <dir>/mle.toml → <dir>/final_params.toml
}

pub enum SurveySource {
    Dir(PathBuf),
}

/// One chain's starting point. Only contains params in
/// `estimate_set`; fixed params are not in this map.
pub struct ChainStart {
    pub chain_id: usize,
    pub values:   HashMap<String, f64>,
    pub source:   InitSource,
}

pub enum InitSource {
    SeededBase,
    UniformDraw  { seed: u64 },
    LhsCell      { row: usize },
    PriorDraw    { seed: u64 },
    PosteriorRow { row: usize, path: PathBuf },
    MlePoint     { path: PathBuf },
    ParamsPoint  { path: PathBuf },
    SurveyRank   { rank: usize, path: PathBuf },
}

pub struct ChainStarts {
    pub starts: Vec<ChainStart>,    // length = n_chains
    pub method: InitMethod,         // echoed for run.json
}

pub enum InitError {
    MissingParam   { name: String, source: PathBuf, suggested_fallback: InitMethod },
    UnknownSource  { path: PathBuf },
    SchemaMismatch { path: PathBuf, expected: &'static str, msg: String },
    NoPrior        { params: Vec<String> },  // from-prior asked, no `~` declared
}

pub fn draw_chain_starts(
    resolved: &ResolvedParameters,
    method:   &InitMethod,
    n_chains: usize,
    seed:     u64,
) -> Result<ChainStarts, InitError>;
```

The Phase 2 → Phase 3 seam is the key invariant: `draw_chain_starts` only writes
values for names in `resolved.estimate_set`. The compiler doesn't enforce this
directly (it's a HashMap, not a typed key set), but every code path that builds
a `ChainStart` constructs the HashMap by iterating `resolved.estimate_set` —
there is no public way to ask "what's the starting value for `gamma`?" when
`gamma` is in `Fixed`. This is what guarantees `--fixed` always wins over
`--init`.

## Provenance into `run.json`

The resolver's outputs flow into `run.json` via two new blocks that mirror the
type design:

```rust
// run_meta.rs / RunMeta
pub struct ParameterProvenance {
    pub value:                f64,
    pub source:               String,                  // ValueSource tag
    pub role:                 String,                  // "fixed" | "estimated"
    pub kicked_from_estimate: Option<KickReason>,      // present iff Fixed{KickedFromEstimate}
    /// Present iff the active scenario set this parameter to a
    /// different value than the final winner. Lets a future
    /// reader see "the scenario said X but `--fixed` overrode
    /// it to Y" without having to cross-reference the scenario
    /// preset by hand. Pairs with the `ScenarioOverridden`
    /// resolver warning.
    pub overrode_scenario:    Option<ScenarioOverride>,
}

pub struct ScenarioOverride {
    pub scenario:       String,    // preset name
    pub scenario_value: f64,
}

pub struct InitProvenance {
    pub method: String,                                // InitMethod tag
    pub chains: Vec<HashMap<String, ChainStartProvenance>>,
}

pub struct ChainStartProvenance {
    pub value:  f64,
    pub source: String,                                // InitSource tag
}
```

Rendered:

```json
"parameters_provenance": {
  "beta":  { "value": 0.42, "source": "model_default", "role": "estimated" },
  "gamma": { "value": 0.10, "source": "fixed_cli",     "role": "fixed",
             "kicked_from_estimate": { "by": "fixed_cli" } },
  "mu":    { "value": 0.50, "source": "fixed_cli",     "role": "fixed",
             "overrode_scenario": { "scenario": "worst_case",
                                    "scenario_value": 0.30 } },
  "rho":   { "value": 0.07, "source": "fit_toml_fixed", "role": "fixed" }
},
"init_provenance": {
  "method": "from-posterior",
  "chains": [
    { "beta": { "value": 0.38, "source": "PosteriorRow{row=42}" } },
    ...
  ]
}
```

A user re-reading a fit six months later can see exactly which flag set each
value, which scenario value (if any) was overridden, and which method drew each
chain start — no archaeology required.

## Post-implementation audit

After the implementing agents finish, a pass-through verifies that no
parameter-value resolution escaped the single resolver. This is itself a
deliverable; the migration isn't complete until the audit passes.

### Audit checklist

1. **Sole writer of `model.parameters[i].value`.** Outside `params_resolver.rs`,
   no code mutates the field. Grep:
   ```
   rg 'parameters\[.*\]\.value\s*=' --type rust
   rg '\.value\s*=\s*Some' --type rust
   ```
   Any hit outside `params_resolver.rs` (or its tests) is a leak; fix before
   merge.

2. **Old resolvers fully removed.** `apply_params_file`,
   `FixedParams::resolve_with_model`, and the inline resolution blocks in
   `profile.rs` / `if2.rs` / `pfilter.rs` are gone:
   ```
   rg 'apply_params_file' --type rust
   rg 'fn resolve_with_model' --type rust
   ```
   Zero non-test hits.

3. **Sole entry point per subcommand.** Every command function takes
   `ResolvedParameters` (and `ChainStarts` for inference) from `params_resolver`
   and `init` calls, never builds its own value map.

4. **Provenance round-trip.** `run.json` for every subcommand carries a
   non-empty `parameters_provenance` block; every entry's `source` matches a
   `ValueSource` variant. Test by parsing a `run.json` from each subcommand and
   asserting structure.

5. **Init-source coverage.** Every `InitMethod` variant has at least one
   integration test producing a `run.json` whose `init_provenance.method` equals
   that variant's tag. No silent fall-throughs.

6. **No alias shims.** Per M-1 (hard break), no
   `--params`/`--param`/name-only-`--fixed`/`--starts-from`/ `--init-method`
   definitions survive in `args/mod.rs`. Grep confirms.

### What the audit catches

The failure mode this checklist prevents is _partial migration_ — code that
compiles and tests green but still has one path that side-steps the resolver and
silently produces a wrong provenance record (or worse, a wrong runtime value).
Items (1) and (2) are the structural checks; (3)–(5) are the runtime behaviour
checks; (6) is the surface-area check.

## Resolved decisions (from PI discussion)

These were flagged as open in earlier revisions; recorded here as the resolution
that the implementing agents should follow.

- **D — Precedence tier list (highest priority).** The proposal's earlier draft
  listed scenario at tier 2 (below `--fixed-file` / fit-toml `[fixed]`), which
  contradicts the documented spec at `docs/camdl-run-spec.md §1.3` and the
  locked-in integration test
  `scenario_runtime_application::scenario_set_replaces_mu_value`. **Resolution:
  scenario beats `--fixed-file` and `[fixed]`, per spec.** The §"Precedence
  (last wins)" section above is the fixed tier list. Scenarios travel with the
  model; choosing a scenario is choosing a documented bundle, and the bundle's
  param sets should win over a user-supplied params file. CLI `--fixed` remains
  highest so quick-test overrides work.
- **A — `from_prior` for params with no `~` declared.** Fallback to
  bounds-uniform with a startup warning naming the parameters, same shape as the
  fit-prior fall-through warning in gh#73. Matches the existing "warn, don't
  punish well-specified-but- incomplete models" posture.
- **B — Warn on fit-toml `[fixed] ∩ [estimate]` overlap.** Yes — `[fixed]` wins,
  `ResolverWarning::FixedEstimateOverlap` emitted. Same provenance shape as
  `ScenarioOverridden`. Costs almost nothing and catches a config-file bug class
  that would otherwise be silent.
- **C — Resolver accepts `scenario` + adhoc `enable`/`disable` independently;
  CLI keeps the mutex.** The CLI mutex (`conflicts_with` in clap) is a UX
  guardrail, not a resolver invariant. The resolver doesn't need to enforce it;
  the argument parser already does.

## Open questions

- **`--fixed` collision warning vs error.** Default: warn (kick-out is the
  useful default). Should `--strict-fixed` exist to escalate to error? Probably
  not worth a flag — file an issue if the slicing workflow surfaces a confusing
  case.
- **Posterior subsampling.** Default: with-replacement (gh#83 pseudocode). Add
  `--posterior-replacement
  {true,false}` only if a real use case shows up.
- **Where does `from_mle` look first?** When given a directory: try
  `<dir>/mle.toml`, then `<dir>/final_params.toml`. Error if neither exists.
  (These are the two canonical filenames in current fit output.)

## Acceptance

This proposal is approved when:

- The audit tables above are acknowledged as the current state.
- The two-flag (`--fixed`, `--init`) unification is accepted as the target API.
- The single-resolver design (`ParameterResolver` / `resolve_parameters`) is
  accepted as the implementation vehicle, with provenance into `run.json` as a
  load-bearing feature.
- M-1 (hard break) confirmed as the migration path.
