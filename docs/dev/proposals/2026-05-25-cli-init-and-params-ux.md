# CLI UX: unify chain-start under `--init`; unify value-setting under `--fixed`; single resolver

Date: 2026-05-25
Author: vsb
Status: draft for review — revision 2 (post-discussion)
Related: gh#83 (init from_prior / from_posterior), gh#85 (--params split semantics)

## Class

**doc-vs-code + code-vs-code**:

- Help text on `--params` claims one thing while the code does another (gh#85).
- Three separate value-resolution code paths exist
  (`util::resolve_run_model`, `FixedParams::resolve_with_model`,
  `priors_precedence::resolve_priors_with_precedence`); each is correct
  for its slice but drift-prone and used inconsistently.
- Cross-subcommand naming of the same concept is inconsistent
  (`--init` vs `--init-method`, `--starts` vs `--chains`,
  `--starts-from` vs nothing).

## TL;DR

Rev 2 collapses parameter-related flags into two verbs that mean
the same thing on every subcommand:

- **`--fixed`** — *these parameters are set to these values*. On
  non-inference subcommands all values are effectively fixed
  (no inference is happening); on inference subcommands the named
  parameters are pinned out of `[estimate]`.
- **`--init`** — *where do chains start from* (inference only).
  The mode family expands to cover prior / posterior / single-file
  warm starts (gh#83).

`--params` and `--param` are removed everywhere. `--init-method`
and `--starts-from` (fit run) are renamed for parity.

A single `ParameterResolver` abstraction owns the precedence chain
and replaces the three half-resolvers in the codebase. Every
subcommand routes through it. Provenance (where each value came
from) is recorded into the resolver's output so `run.json` can
faithfully serialize "this value came from `--fixed`, that one
from `fit.toml [fixed]`, that one from a scenario preset."

## Audit: current state

Verified by reading `rust/crates/cli/src/args/mod.rs`,
`rust/crates/cli/src/util.rs:803-1004`,
`rust/crates/cli/src/fit/config_v2.rs:574-697`,
`rust/crates/cli/src/fit/priors_precedence.rs`, and the per-subcommand
`<sub>.rs` files.

### Flag table

| Subcommand     | `--params` | `--param` | `--fixed` | `--fit` | `--init`         | warm-start          |
|----------------|------------|-----------|-----------|---------|------------------|---------------------|
| `simulate`     | fix        | fix       | —         | —       | —                | —                   |
| `pfilter`      | fix        | fix       | —         | —       | —                | —                   |
| `eval`         | fix        | fix       | —         | —       | —                | —                   |
| `if2`          | mixed      | mixed     | names     | —       | — (`--rw-sd`)    | —                   |
| `profile`      | mixed      | mixed     | names     | yes     | `--init <mode>`  | (gh#74-A WIP)       |
| `survey`       | —          | —         | NAME=VALUE| yes     | —                | —                   |
| `fit run`      | —          | —         | (toml)    | (toml)  | `--init-method`  | `--starts-from`     |

Three layers of inconsistency:

1. `--params` carries `(fix)` semantic on non-inference but
   `(mixed: start-vs-fix-per-param)` on inference. (gh#85)
2. `--fixed` itself takes name-only form on `profile`/`if2`
   but `NAME=VALUE` form on `survey`. (audit-discovered, not yet
   filed)
3. Init / warm-start vocabulary differs across commands (`--init`
   vs `--init-method`; `--starts` vs `--chains`; `--starts-from`
   vs nothing).

### Resolver fragmentation

| Resolver                                    | Lives in                                   | Used by                                            |
|---------------------------------------------|--------------------------------------------|----------------------------------------------------|
| `resolve_run_model`                         | `cli/util.rs:803-1004`                     | `simulate`, `lineage`                              |
| `FixedParams::resolve_with_model`           | `cli/fit/config_v2.rs:574-697`             | `survey`, `profile` (for fit-toml `[fixed]`)       |
| `resolve_priors_with_precedence`            | `cli/fit/priors_precedence.rs`             | `profile`, `fit run` (for priors)                  |
| inline per-subcommand resolution            | `profile.rs:437-453`, `if2.rs:109-168`, `pfilter.rs:47-55` | profile, if2, pfilter                  |

Each is correct on its own. Together they let small details drift
silently — the spec-documented precedence in
`docs/camdl-run-spec.md §1.3` is enforced only in `resolve_run_model`;
profile and if2 implement a *similar* order inline but the next
edit might or might not preserve it.

## Problems, in priority order

### P1. `--params` on inference subcommands is a footgun (gh#85)

Verified at `cli/util.rs:493-501`. `apply_params_file` sets
`p.value = Some(v)` indiscriminately; the role (fix vs start) is
decided downstream by `[estimate]` membership. User reading
`--params truth.toml` reasonably expects "fix these"; in profile
context, parameters that happen to be in `[estimate]` walk off
during PMMH instead.

### P2. `--init` is bounds-only (gh#83)

`InitMethod` (`cli/fit/init.rs:41`) has four variants — `single`,
`uniform`, `lhs`, `survey_top_k`. All sample from bounds; none
sample from prior shape or posterior draws. Important
warm-start patterns (prior visualisation, posterior → posterior
handoff between fits, profile cells starting from a wa-fit's
posterior) are not expressible.

### P3. `--fixed` itself is inconsistent across subcommands

`--fixed` on `profile`/`if2` takes a comma-list of *names*
(pin at model default value); on `survey` takes `NAME=VALUE`
pairs (pin at explicit value). The same flag with the same name
behaves differently. Easy to miss.

### P4. Resolver fragmentation lets precedence drift

Three half-resolvers plus inline per-subcommand reimplementations
of the same precedence rules. Provenance ("where did this value
come from?") is partial — only the priors resolver currently
records it.

## Proposal

### Principle

Two verbs, two concepts, one resolver:

- **`--fixed`** — *these parameters are set to these values*
- **`--init`** — *where do chains start from* (inference only)
- One `ParameterResolver` abstraction implements precedence;
  every subcommand routes through it.

### `--fixed` semantics, defined once

Universal form, all subcommands:

```
--fixed NAME=VALUE          # repeatable; explicit value form
--fixed-file <toml>         # repeatable; layered, later overrides earlier
```

Both accept the same value vocabulary. Name-only `--fixed NAME`
form is **removed** — the "pin at model default" case is just
"don't list the param at all and let the model default flow
through the precedence chain." Removing the form costs nothing
and removes a per-subcommand inconsistency.

On non-inference subcommands (`simulate`, `pfilter`, `eval`),
`--fixed` is the sole flag for setting parameter values. Every
listed value is used as the parameter's runtime value. (No
inference is happening; all values are trivially "fixed" in the
"set and not varying" sense.)

On inference subcommands (`profile`, `if2`, `fit run`), `--fixed`
both sets a value *and* removes the parameter from the
`[estimate]` set if present:

```
camdl profile model.camdl --fit fit.toml --fixed gamma=0.1 --sweep tau=lin(-35,-1,30)
```

Resolves as: estimated set = `(fit.toml [estimate]) − {gamma, tau}`;
`gamma` is pinned at 0.1; `tau` is swept along the grid. The
likelihood landscape is a slice through `(tau, gamma=0.1)`-space.

This is the natural ergonomic for profile-likelihood slicing —
"hold gamma at a specific value while sweeping tau" is exactly
what `--fixed gamma=0.1` should mean. The alternative (error on
collision with `[estimate]`) is more defensive but obstructs the
canonical workflow.

### `--init` family

Same name, same modes, every inference subcommand:

```
--init single                       # all chains at the seeded base params
--init uniform                      # per-chain U(lo, hi) within bounds
--init lhs                          # Latin-hypercube stratified, scale-aware
--init from-prior                   # per-chain draw from each parameter's `~ <dist>`
--init from-posterior --posterior <path>
                                    # per-chain draw from a posterior draws TSV
                                    #   (accepts <draws.tsv> OR a fit-results <dir>)
--init from-mle --mle <path>        # all chains at the MLE point from a file
                                    #   (accepts <mle.toml> OR a fit-results <dir>;
                                    #    formalises `--starts-from` for fit run AND
                                    #    absorbs what was the `--params`-as-start case)
--init survey-top-k --survey-path <dir>
                                    # existing behaviour, kebab-cased
```

`from-params` and `from-mle` collapse into a single mode
(`from-mle`). They were operationally identical — load a single
TOML, all chains start there. "MLE" is a slight misnomer for the
user-written-TOML case but is the right *operation* and the
natural language match for the fit-output case it primarily
serves.

Init applies *only* to parameters in the `[estimate]` set after
`--fixed` resolution. Parameters in `--fixed` or absent from
`[estimate]` take their resolved value regardless of init mode.

When a `from-posterior` or `from-mle` source file is missing
parameters that the current fit's `[estimate]` set includes,
fall back to the subcommand's default init mode for those
columns. Emit a startup warning naming the missing parameters.
When the source file contains parameters not in `[estimate]`,
ignore those columns silently — they are either fixed or absent
from the model.

### Cross-subcommand renames

- `fit run`'s `--init-method` → `--init`. Parity with `profile`.
- `fit run`'s `--starts-from <dir>` → `--init from-mle --mle <dir>`.
- `profile`'s `--starts` (per-cell count) stays as is.
- `if2`'s `--chains` stays as is — established MCMC vocab.

### The single resolver — `ParameterResolver`

Replaces `resolve_run_model`, `FixedParams::resolve_with_model`,
the inline resolvers in `profile.rs` / `if2.rs` / `pfilter.rs`,
and (optionally — separate concern) sits next to
`resolve_priors_with_precedence`.

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
    FixedFile(PathBuf),
    FixedCli,
}

#[derive(Debug, Clone)]
pub struct ResolvedParameter {
    pub name:     String,
    pub value:    f64,
    pub source:   ValueSource,
    pub fixed:    bool,    // true if this param is held fixed (out of [estimate])
}

pub struct ResolvedParameters {
    pub params:        Vec<ResolvedParameter>,
    pub estimate_set:  IndexSet<String>,  // names that survived [estimate] after --fixed kick-out
    pub model:         ir::Model,         // model with .value fields populated
    pub warnings:      Vec<String>,       // collision / kick-out / unknown-name diagnostics
}

pub fn resolve_parameters(inputs: ParameterInputs<'_>) -> Result<ResolvedParameters, String> { ... }
```

#### Precedence (last wins)

1. Model parameter default (`p.value` from DSL)
2. Scenario preset (`preset.params` for the active scenario)
3. `fit.toml [fixed]` block (when present)
4. `--fixed-file <toml>` (each file layered in order; later overrides earlier)
5. `--fixed NAME=VALUE` (highest)

`[estimate]` membership rule:

- Start: `estimate_set = inputs.fit_toml_estimate`
- Remove every name that appears in (4) or (5) — these are
  user-explicit "pin this" assertions and take precedence over
  the toml's `[estimate]` block.
- Emit a warning (not an error) for each such removal, naming the
  parameter and the source: `"--fixed gamma=0.1 removes gamma from [estimate]"`.
- On non-inference subcommands, `inputs.fit_toml_estimate` is
  empty; the kick-out logic is a no-op.

#### Validation, post-resolution

- Every parameter must end with a finite value. Unset parameters
  with no model default → error.
- Bounds checks (`validate_parameter_values`) run on the resolved
  values, as today.
- External tables are resolved here too (per
  `resolve_run_model:977-993`).
- Names appearing in `--fixed` / `--fixed-file` / `fit.toml`
  that don't exist in the model are an error with a "did you
  mean" hint built from the model's parameter list.

#### Provenance into `run.json`

Each `ResolvedParameter` carries its `ValueSource`. The
subcommand-specific `run.json` writer renders this as a
`parameters_provenance` block:

```json
"parameters_provenance": {
  "beta":  { "value": 0.42, "source": "model_default" },
  "gamma": { "value": 0.10, "source": "fixed_cli",     "kicked_from_estimate": true },
  "rho":   { "value": 0.07, "source": "fit_toml_fixed" }
}
```

This is the gh#73 / gh#75 provenance work, generalised to all
values (not just priors).

### Why one resolver, not three

The three existing resolvers each cover a slice:

- `resolve_run_model` — values, but no `[estimate]` interaction
  (it's the simulate path).
- `FixedParams::resolve_with_model` — fit-toml `[fixed]`, but
  doesn't see CLI `--fixed`.
- `resolve_priors_with_precedence` — priors only.

The seam is at "values that come from the model + scenario +
toml + CLI." That's one operation. Splitting it across three
codepaths means three places to keep the same precedence in
sync, and inline reimplementations whenever a new subcommand
appears. One resolver, one set of tests, one provenance shape.

Priors stay in `priors_precedence.rs` — they have a different
content type (`Prior`, not `f64`) and a different precedence
shape (fit_toml > model_ir > flat_fallback, not the 5-tier value
chain). Two resolvers (values + priors), both rigorous, beats
five sloppy ones.

## Migration

camdl is alpha. CLAUDE.md alpha posture: "Backwards compatibility
is a non-goal." Recommend **M-1 (hard break)** — confirmed by
the conversation that produced this revision:

- Remove `--params` and `--param` from all subcommand argument
  structs (`SimulateArgs`, `PfilterArgs`, `If2Args`, `ProfileArgs`,
  `EvalArgs`).
- Remove the name-only form of `--fixed`. Require `NAME=VALUE`.
- Rename `fit run`'s `--init-method` → `--init`.
- Remove `--starts-from`; users must write
  `--init from-mle --mle <dir>`.
- Old invocations error with an actionable message:
  ```
  error: --params is no longer accepted. Replacement:
    --fixed NAME=VALUE             (set & freeze specific values)
    --fixed-file <toml>            (load fixed values from a TOML file)
    --init from-mle --mle <path>   (chain warm-start from a single point)
  ```
- Update camdl-book chapters, blog draft, examples in
  `--help` (`after_help` strings in `args/mod.rs`),
  `docs/user-features.md`, `docs/dsl-cheatsheet.md`.

## What this proposal does NOT touch

- The fit-toml schema (`[estimate]`, `[fixed]`, `[data]`,
  `[model]`) is unchanged.
- Priors resolution (`priors_precedence.rs`) is unchanged.
- `--fit`, `--data`, `--scenario`, `--enable`/`--disable`,
  `--table` are unchanged.
- The forward-sim precedence order documented in
  `docs/camdl-run-spec.md §1.3` is preserved exactly — the
  resolver is a refactor, not a semantic change to the order.

## Implementation outline

1. **Resolver first.** Write `ParameterResolver` in
   `cli/src/params_resolver.rs` (or `cli/src/resolve.rs`). Port
   `resolve_run_model`'s logic into it; add the
   `[estimate]` kick-out and provenance. Cover with unit tests
   for each precedence layer (model default, scenario,
   fit-toml-fixed, --fixed-file, --fixed-cli) and
   collision/kick-out cases.
2. **Migrate `simulate` / `lineage`** to use it. These two are
   the simplest — no `[estimate]`. Confirm `resolve_run_model`
   becomes a thin shim or is deleted.
3. **Migrate `pfilter` / `eval`.** Same shape as simulate.
4. **Migrate `survey`.** The `FixedParams::resolve_with_model`
   path gets folded into the unified resolver via
   `inputs.fit_toml_fixed`.
5. **Migrate `if2` / `profile` / `fit run`.** These pick up the
   `[estimate]` kick-out semantics. Replace inline resolvers
   with `resolve_parameters` calls.
6. **Init family.** Implement the four new `InitMethod`
   variants (`FromPrior`, `FromPosterior`, `FromMle`,
   `SurveyTopK` already exists) with sampler bodies.
7. **CLI surface.** Add `--fixed-file`, `--posterior`, `--mle`
   flags. Remove `--params`, `--param`, name-only `--fixed`,
   `--starts-from`, `--init-method`.
8. **Help text.** Single normative `--init` block shared via
   clap `long_about`. Single normative `--fixed` block.
   Update all `after_help` examples in `args/mod.rs`.
9. **Provenance into `run.json`.** Add
   `parameters_provenance` block; extend
   `run_meta.rs` / `RunMeta` schema.
10. **Doc churn.** `user-features.md`, `dsl-cheatsheet.md`,
    `camdl-run-spec.md §1.3` (the precedence chain stays, the
    flag names change), camdl-book chapters that reference the
    old flag form, the alpha blog draft.

Rough sizing: 1000–1500 lines including tests. The resolver
itself is ~300 lines; the per-subcommand wiring + flag
plumbing is ~400; tests and doc updates account for the rest.

## Open questions

- **`--fixed` collision warning vs error.** Default: warn
  (kick-out is the useful default). Should `--strict-fixed` exist
  to escalate to error? Probably not worth a flag — file an
  issue if the slicing workflow surfaces a confusing case.
- **Posterior subsampling.** Default: with-replacement
  (gh#83 pseudocode). Add `--posterior-replacement
  {true,false}` only if a real use case shows up.
- **`from-prior` for params with no `~`.** Bounds-uniform
  fallback with a startup warning naming the parameters, same
  shape as the fit-prior fall-through warning in gh#73.
- **Where does `from-mle` look first?** When given a directory:
  try `<dir>/mle.toml`, then `<dir>/final_params.toml`. Error
  if neither exists. (These are the two canonical filenames in
  current fit output.)

## Acceptance

This proposal is approved when:

- The audit tables above are acknowledged as the current state.
- The two-flag (`--fixed`, `--init`) unification is accepted as
  the target API.
- The single-resolver design (`ParameterResolver` /
  `resolve_parameters`) is accepted as the implementation
  vehicle, with provenance into `run.json` as a load-bearing
  feature.
- M-1 (hard break) confirmed as the migration path.
