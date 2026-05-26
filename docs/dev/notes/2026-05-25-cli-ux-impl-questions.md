# CLI UX rev 2 implementation: status and remaining work

Date: 2026-05-25
Author: claude-agent (session for V. Buffalo)
Proposal: `docs/dev/proposals/2026-05-25-cli-init-and-params-ux.md`
Branch: `worktree-agent-a0d854c5fd1d64f12`

This note records the state of the CLI UX rev 2 implementation as of
this session — what shipped, what's deferred, and the specific
decisions / ambiguities I flagged for review before continuing. It is
filed under `docs/dev/notes/` rather than `docs/dev/proposals/` per
CLAUDE.md ("If you can't produce a reproduction, the artifact is a
*question* filed under `docs/dev/notes/`") — the question here is
"how do you want the migration sequenced given the schedule
coordination call-outs in the proposal?"

## What shipped this session

Three commits on `worktree-agent-a0d854c5fd1d64f12`:

  - `gh#83/gh#85: params_resolver — unified value-resolution chain (rev 2)`
    - `rust/crates/cli/src/params_resolver.rs` (new, ~660 lines with
      tests). Implements the proposal's type design exactly:
      `ParameterInputs<'a>`, `ResolvedParameters`, `ValueSource` ADT,
      `ParameterRole { Fixed { reason: FixReason } | Estimated }`,
      `ResolveError` ADT, `ResolverWarning`, `resolve_parameters` +
      `print_warnings`.
    - 16 unit tests covering each precedence tier (model default,
      scenario, fit-toml [fixed], --fixed-file, --fixed CLI),
      [estimate] kick-out + warning shape, bounds/finite validation,
      unknown-param/scenario errors, and full provenance round-trip
      across all five sources.
    - `eval.rs` migrated as the proof-of-design consumer (smallest
      subcommand; no inference / no [estimate] semantics; exercises
      every resolver code path including bounds validation).
    - `main.rs` registers the new module.

  - `docs(notes): gh#83/gh#85 — CLI UX rev 2 implementation status`
    (this file).

  - `gh#83/gh#85: migrate pfilter to params_resolver`
    - `pfilter.rs:42-92` inline block (load_model + apply_params_file
      loop + scenario lookup + filter + CLI overrides + validate)
      collapses to a single `resolve_parameters()` call.
    - Behaviour-identical surface; `pfilter_trajectories` integration
      tests pass.

  - Test posture:
    - `cargo test --release -p cli --bin camdl params_resolver` →
      16/16 pass.
    - `CAMDL_SKIP_VERSION_CHECK=1 cargo test --release --workspace
      --tests --exclude camdl-tests` → all green except the 4 expected
      pre-existing `time::tests::*_panics_in_debug` failures
      (debug_assert! compiled out in release; tracked separately).

## What is deferred (and why)

The proposal estimates 1000-1500 LOC + ~200 sed sites + ~70 fixture
migrations + 4 hand-rewrites for a full landing. That is real
multi-session work for code that touches inference math
(`pgas.rs`, `if2.rs`, `pmmh.rs`, `particle_filter.rs`) and the
camdl-book chapter render pipeline (per the proposal's "camdl-book
coordination" section). Per CLAUDE.md "Conservatively scoped: if a
change touches inference math, treat it as high-risk regardless of
how mechanical it looks. Read the full function before editing any
part of it" — I am declining to ship a half-migration of the
inference subcommands in a single session, because:

  1. The proposal explicitly says the camdl-book seed-timing chapter
     (`draft.qmd:935`) "is actively being rendered" and the chapter
     command examples must update in lockstep with the camdl release
     under M-1.
  2. A partial migration (some subcommands use the resolver, others
     still write `p.value = Some(_)` inline) is exactly the
     "two-paths" state the proposal exists to eliminate. Worse than
     either fully-old or fully-new.
  3. The four hand-rewrites named in the proposal (camdl-book
     CLAUDE.md, language spec §2960, inference.md §654, he2010
     vignette Makefile) require careful prose work that benefits
     from sitting next to the maintainer, not being drafted by an
     agent in isolation.

The resolver foundation that *did* ship is reusable as-is — every
subsequent subcommand migration becomes a small, reviewable diff
that pattern-matches against `eval.rs`'s shape. The risky pieces
(`if2.rs`, `profile.rs`, `fit run` runner, `survey.rs`) can be
sequenced one-per-commit with a green test run between each.

## Detailed remaining-work breakdown

Tagged by proposal section, sized for follow-up sessions.

### Step 2 — migrate `simulate` / `lineage` to the resolver

These have no `[estimate]` set. Replace the inline blocks in
`util::resolve_run_model:803-1004` with a call to
`resolve_parameters`. Confirm `resolve_run_model` either becomes a
thin shim or is deleted outright. Expected diff: ~100 lines of
deletions in `util.rs`, ~30 lines of insertions at the
`simulate.rs` / `lineage.rs` call sites.

Risk: low. simulate-path forward simulation is well-covered by the
golden integration tests (`make test-golden`). Any divergence in
behaviour surfaces as a TSV diff under `ir/expected/`.

### Step 3 — migrate `pfilter` / `eval`

**Done this session.** Both shipped — `eval.rs` in the resolver
foundation commit, `pfilter.rs` in commit 3.

### Step 4 — migrate `survey`

`FixedParams::resolve_with_model` (config_v2.rs:574-697) folds into
the unified resolver via `inputs.fit_toml_fixed`. The complication is
that `survey` *also* uses fit-toml's `[fixed].from_scenario` and
`from_file` semantics, which currently live inside `FixedParams`. The
resolver doesn't need to know about those — fold them at the
`survey.rs` call site by expanding `from_scenario` / `from_file` into
the `IndexMap<String, f64>` *before* calling `resolve_parameters`.
See `FixedParams::expand_from_scenario` and `resolve()` for the
existing logic to lift.

Risk: medium. Survey is the entrypoint for camdl-book's identifiability
chapter; check `rust/crates/cli/tests/survey_*.rs` golden behaviour
after migration.

### Step 5 — migrate `if2` / `profile` / `fit run`

These pick up the `[estimate]` kick-out semantics. Each replaces an
inline resolver:

  - `if2.rs:109-168`: model load + scenario + overrides + apply_params_file
  - `profile.rs:437-453`: similar shape; per-cell PMMH/IF2 with `--fixed`
    name-list (today) → `--fixed NAME=VALUE` (post-migration)
  - `fit/runner.rs:147-204`: scenario expansion + fit-toml [fixed] +
    apply_params_file. Highest-risk site because it's the production
    Bayesian fit path.

Risk: **high** — these are inference subcommands. CLAUDE.md mandates
"read the full function before editing any part of it" for
`pgas.rs`, `if2.rs`, etc. The migration is mechanical (the resolver
already encodes the precedence) but the *provenance recording* into
`run.json` is new behaviour that needs a careful test pass.

Migration sequence I'd recommend (per-commit, green-tested between):

  1. `if2.rs`: simplest of the three. No fit-toml interaction.
  2. `profile.rs`: per-cell loop; needs the focal-parameter
     kick-out to layer cleanly on top of the resolver's existing
     kick-out (focal swept value goes in `fixed_cli` per-cell).
  3. `fit/runner.rs`: drop `FixedParams::resolve_with_model`,
     route all fit-toml [fixed] through `fit_toml_fixed`.

### Step 6 — Init family

Extend `InitMethod` with four new variants:

  - `FromPrior` — draws from each parameter's `~ <dist>` (or
    bounds-uniform fallback per the open question; see Decision A
    below).
  - `FromPosterior { source: PosteriorSource }` where
    `PosteriorSource::{ DrawsTsv(PathBuf) | FitDir(PathBuf) }`.
  - `FromMle { source: MleSource }` where `MleSource::{ File | FitDir }`.
    The FitDir loader tries `<dir>/mle.toml` then
    `<dir>/final_params.toml` (proposal "Open questions" §"Where
    does `from-mle` look first?").
  - `FromParams { path: PathBuf }` — flat TOML, top-level keys are
    parameter names.

Each loader knows its file shape; no generic auto-detection (proposal
"verb-per-source" rule).

The `ChainStart` / `ChainStarts` / `InitSource` ADTs in the
proposal's "Init phase types" section need new entries in
`run_meta.rs` for serialisation.

Risk: medium. `init.rs` is well-encapsulated; the new variants slot
into the existing match dispatch in `build_chain_starts` /
`resolve_per_chain_starts_from_method`.

### Step 7 — CLI surface changes (M-1 hard break)

Per the proposal, this is the breaking-change commit:

  - Add `--fixed-file <toml>` (repeatable, layered) everywhere
    `--fixed` is accepted.
  - Add `--posterior <path>`, `--mle <path>`, `--params <toml>` as
    *init-mode arguments* (distinct from the removed `--params`
    *flag*).
  - Remove `--params` and `--param` from `SimulateArgs`,
    `PfilterArgs`, `If2Args`, `ProfileArgs`, `EvalArgs`.
  - Remove the name-only form of `--fixed`; require `NAME=VALUE`.
  - Rename `fit run`'s `--init-method` → `--init`.
  - Remove `--starts-from`; users must write
    `--init from-mle --mle <fit-dir>`.
  - Each removed flag errors with the actionable replacement message
    from the proposal §"Migration".

**Coordination call-out — needs maintainer sign-off before landing:**
The proposal §"camdl-book coordination" says: "the chapter command
examples must be updated in lockstep with the camdl release — render
after rewrite, not before." If the seed-timing chapter is *currently*
being rendered (i.e. its `make` target is on the maintainer's daily
loop), Step 7 should NOT land until the rewrite is queued. Otherwise
the chapter renders fail and the maintainer is blocked.

### Step 8 — Help text

Single normative `long_about` blocks for `--init` and `--fixed`,
shared across subcommands via clap attribute. Update every
`after_help` example in `args/mod.rs` that uses `--params` / `--param`
/ `--starts-from` / `--init-method` / `--fixed name1,name2`.

This is purely mechanical once Step 7 is in.

### Step 9 — Provenance into `run.json`

Extend `run_meta.rs`'s `RunMeta` (and per-kind `Meta` structs) with:

  - `parameters_provenance: HashMap<String, ParameterProvenance>` —
    one entry per parameter, mirroring the resolver's
    `ResolvedParameter` shape (value + source tag + role +
    `kicked_from_estimate` Option). Proposal §"Provenance into
    run.json" has the exact JSON shape.
  - `init_provenance: InitProvenance` (inference subcommands only) —
    `method` tag + per-chain `ChainStartProvenance { value, source }`.

Wire each subcommand's `cmd_*` function to populate these blocks from
`ResolvedParameters` and `ChainStarts` before writing `run.json`.

Risk: low. Schema additions, not changes. Backward-compatible reads of
old run.json files break (per alpha posture: M-1 hard break is OK).

### Step 10 — Doc churn, mechanical

~200 sed sites across `camdl` and `camdl-book` (proposal's blast-radius
estimate). The proposal lists the high-volume targets:
`docs/user-features.md`, `docs/dsl-cheatsheet.md`,
`docs/camdl-run-spec.md §1.3`, the alpha blog draft.

### Step 11 — Doc churn, load-bearing prose rewrites

The four named hand-rewrites (proposal §"Blast radius"):

  1. `camdl-book/CLAUDE.md:642-661` — synthetic-recovery rule.
     The current text says "use `pfilter --params` to evaluate
     likelihood at truth; do NOT use `profile --params` (walks)."
     New text: "`pfilter --fixed-file` for evaluation;
     `profile --init from-params --params <toml>` (hand-written) or
     `profile --init from-mle --mle <fit-dir>` (prior fit) for
     warm-start." Same teaching point, completely different flag pair.
  2. `camdl/docs/camdl-language-spec.md:2960-3001` (+ book mirror at
     `language/spec.qmd:2849-2890`) — the name-only `--fixed "N0,mu,k"`
     surface. This is a *feature removal* dressed as a cleanup. Need
     an explicit "the equivalent is now ..." callout.
  3. `camdl/docs/inference.md:654-665` — four-way precedence list.
     `--params` disappears; the list collapses to three CLI sources
     (`--fixed{,-file}`, `--fit [fixed]`, scenario) + model default.
  4. `vignettes/he2010*/Makefile` `FIXED_PARAMS=mu,iota,sigma_se,...`
     — nine names, no values. Standard fix: extract to
     `vignettes/he2010/fixed.toml` and pass `--fixed-file` (the
     proposal recommends this as the canonical pattern for
     many-fixed-param vignettes).

Each of these needs careful prose work by someone who understands
both the old and new model — best done with the maintainer
present, not by an agent.

### Step 12 — fit.toml fixture migration

~70 files, mechanical:

  - `[stages.<n>] init_method = "lhs"` → `init = "lhs"`
  - `[stages.<n>] starts_from = "scout"` → `init = "from-mle"` +
    `init_mle = "scout"`

Old keys should produce an actionable clap-style error at
config-load (cite the rename + give the replacement). Doable as a
single sed-equivalent pass once Step 7's CLI surface is in.

## Decisions deferred to maintainer review

These are points where I had a provisional answer but the proposal
didn't fully pin them down — flagging for review rather than
guessing.

### Decision A — `from-prior` for params with no `~`

Proposal §"Open questions": "Bounds-uniform fallback with a startup
warning naming the parameters, same shape as the fit-prior
fall-through warning in gh#73."

My provisional implementation choice for Step 6: emit the warning
using the same `format_flat_fallback_warning`-style structure as the
prior fallback. Distinct warning code (`init_prior_uniform_fallback`)
so users can suppress independently.

Alternative: hard error. Rationale would be that a user who asked
for `from-prior` and got `from-bounds-uniform` is being silently
demoted, which the proposal otherwise treats as a critical bug
(silent-wrong-answer in fit-prior context, §"Profile vs fit run
semantics for tier 3"). Worth pinning before landing Step 6.

### Decision B — fit-toml [fixed] kick-out warning

Proposal §"Precedence" says only tiers 4 (--fixed-file) and 5
(--fixed CLI) should kick a parameter out of `[estimate]` with a
warning. My resolver implements this exactly. But the proposal text
is ambiguous about whether `fit-toml [fixed]` overlapping with
`fit-toml [estimate]` should warn (it can't happen in practice if
the toml validator enforces mutual exclusion at config-load) or
error.

Current resolver behaviour: no warning, no error — the override
applies and `[estimate]` membership is unchanged. This matches the
proposal's tier-3 description ("the toml's [fixed] block already
excludes those names from [estimate] at config-load time"). The
fit-toml validator already enforces this; the resolver assumes the
input is well-formed.

If the maintainer prefers defence-in-depth (validate at resolver
level too), file as a follow-up — the resolver has the data to do
it; the test would be straightforward.

### Decision C — `--scenario` + `--enable`/`--disable` mutual exclusion

Today's CLI surface declares these mutually exclusive at the clap
level (`conflicts_with = "scenario"`). My resolver accepts either
form — `scenario: Some(name)` is handled by the scenario branch,
and `adhoc_enable / adhoc_disable` non-empty is handled by the
ad-hoc branch. The two cases are exclusive in the type sense (an
input with `scenario = Some(_)` ignores ad-hoc; an input with
`scenario = None` uses ad-hoc).

This is intentional — the clap-level conflict is the right UX, but
the resolver itself shouldn't enforce CLI ergonomics. If a future
code path wants to compose scenarios with ad-hoc overrides (e.g.
"baseline scenario + ad-hoc enable of one experimental
intervention"), the resolver doesn't need to change.

## Audit checklist results (proposal §"Post-implementation audit")

Run on the post-commit tree after eval + pfilter migrations:

```
$ rg 'parameters\[.*\]\.value\s*=' --type rust rust/crates/cli/src/
# (no hits)

$ rg '\.value\s*=\s*Some' --type rust rust/crates/cli/src/
# params_resolver.rs writes (the sole resolver — these are CORRECT):
rust/crates/cli/src/params_resolver.rs   (5 hits — scenario params, scale, fit-toml fixed, --fixed-file, --fixed CLI)

# Pre-migration legacy paths (Steps 2/4/5):
rust/crates/cli/src/util.rs              (6 hits — resolve_run_model + apply_params_file + --param-vec)
rust/crates/cli/src/main.rs              (2 hits — simulate scenario apply + --param apply)
rust/crates/cli/src/profile.rs           (2 hits — scenario + overrides)
rust/crates/cli/src/survey.rs            (5 hits — fit-toml [fixed] + scenario + CLI fixed)
rust/crates/cli/src/if2.rs               (2 hits — scenario + overrides)
rust/crates/cli/src/fit/runner.rs        (3 hits — scenario + apply_params_file + bound starts fill)
```

The eval.rs and pfilter.rs migrations eliminated their entries. The
remaining hits are exactly the migration targets enumerated in
Step 2 / Step 4 / Step 5 above.
Audit item (1) passes for `params_resolver.rs` (it is the sole
resolver writer; the other hits are the unmigrated legacy paths
that will be removed as their subcommands migrate).

Audit items (2)-(6) require the full migration to land:

  - (2) `apply_params_file` / `FixedParams::resolve_with_model` /
    inline blocks: 0 hits required. Currently: ~20 hits (the
    legacy paths above).
  - (3) Every subcommand command function takes `&ResolvedParameters`:
    currently 1/N (eval).
  - (4) `run.json` carries `parameters_provenance`: not yet — Step 9.
  - (5) Every `InitMethod` variant has an integration test: not yet —
    Step 6.
  - (6) No alias shims in `args/mod.rs` for removed flags: not yet —
    Step 7.

## Suggested commit sequence for the rest of the work

A maintainer-reviewable sequence, one logical unit per commit:

1. `simulate` + `lineage` migration (deletes most of
   `util::resolve_run_model`)
2. `pfilter` migration
3. `survey` migration (folds `FixedParams` into resolver inputs)
4. `if2` migration
5. `profile` migration
6. `fit/runner.rs` migration (drops `FixedParams::resolve_with_model`)
7. Audit grep zero-hit confirmation (no code outside `params_resolver`
   writes `p.value = Some(_)`)
8. Add `FromPrior` to `InitMethod`
9. Add `FromPosterior` to `InitMethod` (+ `PosteriorSource` ADT)
10. Add `FromMle` to `InitMethod` (+ `MleSource` ADT, mle.toml/final_params.toml lookup)
11. Add `FromParams` to `InitMethod` (flat-TOML loader)
12. CLI surface — add `--fixed-file`, `--posterior`, `--mle`,
    `--params` (init-mode); remove `--params`/`--param`/name-only
    `--fixed`/`--starts-from`/`--init-method`. M-1 hard break.
    **Needs camdl-book coordination per proposal.**
13. Help text — shared `long_about` blocks for `--init` / `--fixed`
14. Provenance into `run.json` — extend `run_meta.rs`
15. Mechanical doc churn (~200 sed sites)
16. fit.toml fixture migration (~70 files)
17. Four hand-rewrites (camdl-book CLAUDE.md, language spec,
    inference.md, he2010 Makefile) — best done with maintainer
    co-present.

Each commit independently green-testable; each one a reviewable diff
of < ~300 lines.
