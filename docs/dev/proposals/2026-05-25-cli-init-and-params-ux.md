# CLI UX: unify chain-start semantics under `--init`; disambiguate `--params`

Date: 2026-05-25
Author: vsb
Status: draft for review
Related: gh#83 (init from_prior / from_posterior), gh#85 (--params split semantics)

## Class

**doc-vs-code + code-vs-code**. Help text on `--params` claims one
thing while the code does another (gh#85 §"The trap"); and the
naming of the chain-start family is inconsistent across subcommands
in ways that aren't documented anywhere (audit below).

## TL;DR

camdl has a single underlying concept — *how to set parameter
values, with the role (fix vs start) determined by inference
context* — surfaced through three flags (`--params`, `--init`,
`--starts-from`) whose names, scopes, and meanings disagree across
subcommands. A camdl-book chapter author hit the ambiguity in a
live session (gh#85). The recommended fix is to make `--init` the
single source of truth for chain starting points across all
inference subcommands, expand its mode set to cover prior /
posterior / params-file warm starts (gh#83), and demote `--params`
to non-inference subcommands only. `--params` on inference
subcommands becomes an alias for `--init from-params
--params-file <toml>` with a deprecation notice.

## Audit: current state of parameter-related flags

Verified by reading `rust/crates/cli/src/args/mod.rs` and
`rust/crates/cli/src/profile.rs:440-453` /
`rust/crates/cli/src/util.rs:493-501`.

| Subcommand     | `--params` | `--param` | `--fixed` | `--fit` | `--init`         | warm-start          |
|----------------|------------|-----------|-----------|---------|------------------|---------------------|
| `simulate`     | fix        | fix       | —         | —       | —                | —                   |
| `pfilter`      | fix        | fix       | —         | —       | —                | —                   |
| `eval`         | fix        | fix       | —         | —       | —                | —                   |
| `if2`          | mixed      | mixed     | yes       | —       | — (`--rw-sd`)    | —                   |
| `profile`      | mixed      | mixed     | yes       | yes     | `--init <mode>`  | (none; gh#74-A WIP) |
| `survey`       | —          | —         | yes       | yes     | —                | —                   |
| `fit run`      | —          | —         | (toml)    | (toml)  | `--init-method`  | `--starts-from`     |

Where:

- **fix** — value is used as the parameter's runtime value, frozen.
- **mixed** — value is the runtime value, but for parameters in the
  inference `[estimate]` set the value functions as a *starting
  point* and is free to move; for parameters not in `[estimate]`
  it functions as fixed. The user has to look up which mode each
  parameter is in to predict the flag's effect.
- The `--init` column entries refer to the four currently-implemented
  modes: `single`, `lhs`, `uniform`, `survey_top_k`
  (`rust/crates/cli/src/fit/init.rs:41`).
- Warm-start: `fit run --starts-from <dir>` uses the prior stage's
  MLE (a single point). No equivalent exists on `profile`.

### Concrete inconsistencies, by example

The same concept appears under different names across subcommands:

- "init mode" → `--init` (profile), `--init-method` (fit run);
  `if2` has no direct equivalent (chains start uniformly from
  bounds when `--rw-sd auto`, no other modes available).
- "number of independent starting points" → `--starts` (profile,
  per-cell), `--chains` (if2, top-level), `chains = N` in
  fit-toml `[stages.<name>]`. Same concept, three names.
- "warm-start from a prior fit" → `--starts-from <dir>` (fit run
  only, MLE point). gh#83 asks for `--init from_posterior` to
  fill the gap for `profile` and to extend `fit run` to draws
  rather than MLE only.

These were all built incrementally as features grew. They are
small individually; collectively they oblige the user to learn
each subcommand's vocabulary separately.

## Problems, in priority order

### P1. `--params` on `profile` / `if2` is a footgun (gh#85)

Verified at `rust/crates/cli/src/util.rs:493-501`:

```
pub fn apply_params_file(model: &mut ir::Model, path: &str) -> Result<(), String> {
    let vals = load_params_toml(path)?;
    for p in &mut model.parameters {
        if let Some(&v) = vals.get(&p.name) {
            p.value = Some(v);
        }
    }
    ...
}
```

`apply_params_file` is a pure value-setter. The resulting role
(fix vs start) is decided downstream by whether the parameter
appears in the inference `[estimate]` set (resolved per-subcommand
from `--fit` toml, `--fixed`, etc.).

Worst case: a user writes `profile model.camdl --params truth.toml`
expecting to fix `gamma` at its true value while profiling
`tau`. If `gamma` happens to be in the `[estimate]` block of the
attached `--fit` toml, it becomes a *starting value* and walks
off during PMMH. The likelihood landscape looks fine; the user's
profile is wrong in a way no diagnostic will catch.

The current help text on `profile`'s `--params`
(`rust/crates/cli/src/args/mod.rs:1086-1097`) is paragraph-form,
buries the dual semantic mid-sentence, and is the kind of doc the
user has to read very carefully to extract the actual rule from.

### P2. `--init` is bounds-only (gh#83)

Verified at `rust/crates/cli/src/fit/init.rs:41` (the
`InitMethod` enum). All four modes — `single`, `lhs`, `uniform`,
`survey_top_k` — produce starting points by sampling the
parameter *bounds*, ignoring (a) the prior's shape and (b) any
posterior information from a previous fit. This leaves two
important warm-start patterns unsupported:

- *From the model's prior.* `LogNormal(log 5, 1.0)` and
  `Uniform(0.01, 50)` over the same bounds produce wildly
  different starting-point distributions; current `--init`
  cannot distinguish them.
- *From another fit's posterior.* `--starts-from <dir>` covers
  the MLE-point case for `fit run` but not the posterior-sample
  case, and isn't implemented for `profile` at all.

### P3. Cross-subcommand naming inconsistency (filed implicitly by the audit)

Same concept, three names (init mode); same concept, three names
(start count); same concept, two implementations of warm-start
(MLE-only via `--starts-from`, nothing via `--init from_posterior`).

## Proposal

### Principle: one flag per concept; one name per concept

For inference subcommands (`profile`, `if2`, `fit run`):

- **`--init <mode>`** is the single source of truth for chain
  starting points. Same name, same semantics, everywhere.
- **`--params`** is *not accepted* on inference subcommands going
  forward. The closest replacement is `--init from-params
  --params-file <toml>` (one explicit mode in the `--init`
  family).
- **`--fixed`** keeps its current semantic: pin specific
  parameters out of the inference set. Orthogonal to `--init`.

For non-inference subcommands (`simulate`, `pfilter`, `eval`):

- `--params` remains as today — unambiguous "set values for this
  run", no inference context to muddy the meaning.

### The expanded `--init` family

```text
--init single                   # all chains at the seeded base params
--init uniform                  # per-chain U(lo, hi) within bounds
--init lhs                      # Latin-hypercube stratified, scale-aware
--init from-params              #  + --params-file <toml>
                                # load a single point from TOML; equivalent to
                                # the old `--params` semantic on inference
--init from-prior               # sample once per chain from each parameter's
                                # `~ <dist>` declaration; falls back to
                                # bounds-uniform for parameters without `~`
--init from-posterior           #  + --posterior <draws.tsv | fit-dir>
                                # sample chain starts as uniform rows from a
                                # posterior draws TSV
--init from-mle                 #  + --mle-path <mle.toml | fit-dir>
                                # all chains at a single MLE point (formalises
                                # the current `--starts-from <dir>` behaviour
                                # for `fit run`)
--init survey-top-k             #  + --survey-path <dir>
                                # existing behaviour, kebab-cased for parity
```

`from-posterior` accepts either a raw `draws.tsv` path or a
fit-results directory; the loader auto-resolves
`<dir>/draws.tsv` when given a directory. `from-mle` does the
same with `<dir>/mle.toml`. This is the natural extension of
`fit run`'s `--starts-from`, generalised across subcommands.

When the source file is missing parameters that the current
fit's `[estimate]` set includes, fall back to the current
`--init` default for those columns. Emit a warning naming the
missing parameters. When the source file contains parameters
*not* in `[estimate]`, ignore those columns (they are either
fixed or not part of the model — neither outcome benefits from
overriding from the file).

### Cross-subcommand consistency renames

- `fit run`'s `--init-method` → `--init`. Parity with `profile`.
- `fit run`'s `--starts-from <dir>` → `--init from-mle
  --mle-path <dir>` (current behaviour preserved exactly; same
  resolved-file behaviour).
- `profile`'s `--starts` (per-cell count) stays as is — the name
  collision with gh#85's Option A (which we are not adopting) is
  no longer a concern.
- `if2`'s `--chains` stays as is — established MCMC vocab; renaming
  to `--starts` would obscure rather than clarify. Both terms now
  refer to "number of independent chain starting points", and the
  per-subcommand name reflects the algorithm's idiom.

### Help-text rewrite

`--init` gets a single normative help block, shared via a clap
`#[command(long_about = …)]` between subcommands. The text reads:

```
INIT MODES (where do chain starting points come from?)

  single             every chain starts at the seeded base params
  uniform            per-chain U(lo, hi) over [estimate] parameter bounds
  lhs                Latin-hypercube stratified within bounds (scale-aware
                     via Transform; best basin coverage at low chain counts)
  from-params        load a single point from a TOML file; pass
                     --params-file <path>. (Use this where you'd previously
                     have written `--params <path>` on profile or if2.)
  from-prior         sample once per chain from each parameter's `~ <dist>`
                     declaration in the .camdl source
  from-posterior     sample chain starts uniformly from a posterior draws TSV
                     (or a fit-results directory containing draws.tsv); pass
                     --posterior <path>
  from-mle           all chains at the MLE point from a prior fit; pass
                     --mle-path <path>
  survey-top-k       initialise from the top-K best landscape points of a
                     prior survey; pass --survey-path <dir>

Init applies only to parameters in the inference `[estimate]` set; parameters
in `[fixed]` (or absent from `[estimate]`) take their model value or `--fixed`
override regardless of init mode.
```

The line *"Init applies only to parameters in the inference `[estimate]`
set"* is the clarification gh#85 was asking for, with a clear pointer
to the right mechanism (`[fixed]` / `--fixed`) for the fixing case.

## Migration

camdl is alpha. The default at alpha is clean break with updated
docs; backwards-compatibility shims accrete and rot. That said,
the camdl-book chapters and the recently-released blog post both
use `--params` on inference subcommands. Two reasonable paths:

### Option M-1: hard break, update docs (recommended)

- Remove `--params` and `--param` from `ProfileArgs` and
  `If2Args`.
- Old invocations error with:
  ```
  error: --params is no longer accepted on `camdl profile`.
    Replacement: --init from-params --params-file <toml>
    See `camdl profile --help` (INIT MODES section).
  ```
- Update camdl-book chapters and blog draft.
- Single release cycle of acute pain, no long-term carry cost.

### Option M-2: deprecation alias for one release

- Keep `--params` / `--param` on `ProfileArgs` / `If2Args` as
  aliases for `--init from-params --params-file <toml>`.
- Emit a one-line stderr deprecation notice on each use:
  ```
  [deprecation] --params on inference subcommands is deprecated;
                use --init from-params --params-file <path> instead.
                Will be removed in vNEXT.
  ```
- Remove after one release.

Option M-1 fits CLAUDE.md's stated alpha posture
("Backwards compatibility is a non-goal"; "When a field is
renamed, rename it everywhere atomically") and is what I'd
recommend. Option M-2 is the courtesy version if the camdl-book
chapter rendering schedule makes a hard break inconvenient.

## What this proposal does NOT touch

To keep scope bounded:

- The fit-toml schema is unchanged. `[estimate]`, `[fixed]`,
  `[data]`, and `[model]` continue to do what they do.
- `--fixed` and `--fit` are unchanged.
- The `InitMethod` enum gains four variants (`FromParams`,
  `FromPrior`, `FromPosterior`, `FromMle`); existing variants
  are unchanged.
- Non-inference subcommands (`simulate`, `pfilter`, `eval`) are
  unchanged.
- Display of which params became "starting values" vs "fixed" in
  the run log is a separate UX win (the kind the camdl-book agent
  asked for) and belongs to a follow-up issue, not this proposal.

## Implementation outline

If approved, the work is:

1. Extend `InitMethod` (`crates/cli/src/fit/init.rs`) with the
   four new variants and their sampler implementations.
2. Add the supporting flags (`--params-file`, `--posterior`,
   `--mle-path`) to `InferenceCore` or a new
   `InitSourceArgs` flatten struct, depending on which gives
   the cleaner help text under clap.
3. Rename `fit run`'s `--init-method` → `--init`; resolve
   `--starts-from` → `--init from-mle --mle-path <dir>` at
   parsing time (or remove `--starts-from` outright per M-1).
4. Remove `--params` / `--param` from `ProfileArgs` and
   `If2Args` (M-1) or attach a deprecation parser shim (M-2).
5. Update `docs/user-features.md`, `docs/dsl-cheatsheet.md`, the
   profile / fit-run / if2 examples in `--help` (after-help
   strings in `args/mod.rs`), and any camdl-book chapter that
   references the old flag form.
6. Tests: unit tests on the new sampler variants
   (sampling-respects-prior, posterior-row-sampling-is-uniform,
   missing-params-warn-and-fallback); integration tests on a
   small golden fit toml that exercises each `--init` mode end
   to end.

Rough sizing: ~600–900 lines including tests, mostly in
`fit/init.rs` and `args/mod.rs`. The doc churn is comparable in
volume to the code churn.

## Open questions

- **Posterior subsampling policy** — gh#83 proposes uniform-with-
  replacement over draws. For chains > 1, sampling without
  replacement gives uncorrelated starts; with replacement is fine
  when n_draws ≫ n_chains. Default: with-replacement, simpler and
  matches the gh#83 pseudocode. Add `--posterior-replacement
  {true,false}` if users push back.
- **`from-prior` fallback for params lacking `~`** — bounds-uniform
  is the safe default; warn at startup naming the parameters that
  fell through, same shape as the fit-prior fall-through warning
  in gh#73.
- **Deprecation horizon** — if Option M-2, one release. If we
  decide alpha lets us skip the alias, M-1 ships immediately.

## Acceptance

This proposal is approved when:

- The audit table above is acknowledged as the current state.
- The `--init` family expansion (gh#83) and the `--params`
  demotion on inference subcommands (gh#85) are accepted as
  *coupled* — they share `from-params` as the resolver path, so
  shipping one without the other leaves a worse state than
  today.
- Migration path (M-1 vs M-2) is chosen so the implementer knows
  whether to write the alias shim.
