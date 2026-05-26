# CLI UX rev 2 implementation: status and remaining work

Date: 2026-05-25
Author: claude-agent (session for V. Buffalo)
Proposal: `docs/dev/proposals/2026-05-25-cli-init-and-params-ux.md`
Branch: `worktree-agent-a0d854c5fd1d64f12` (original) → continued on
`main` after maintainer merged keeper commits.

This note records the state of the CLI UX rev 2 implementation as of
this session — what shipped, what's deferred, and the specific
decisions / ambiguities I flagged for review before continuing. It is
filed under `docs/dev/notes/` rather than `docs/dev/proposals/` per
CLAUDE.md ("If you can't produce a reproduction, the artifact is a
*question* filed under `docs/dev/notes/`") — the question here is
"how do you want the migration sequenced given the schedule
coordination call-outs in the proposal?"

## Update 2026-05-25 (later session): ScenarioOverridden + FixedEstimateOverlap landed

Following the proposal's updated `§"Scenario-override visibility"` and
the resolved decisions (D = spec wins; A = bounds-uniform fallback for
from-prior; B = warn on fit-toml `[fixed] ∩ [estimate]` overlap; C =
resolver does not enforce scenario-vs-enable/disable mutex), this
session implemented the additive resolver changes:

- `ResolverWarning::ScenarioOverridden { name, scenario, scenario_value,
  by, new_value }` — emitted when the scenario set a value but a
  higher-precedence source (currently only `--fixed-cli` under the
  spec §1.3 ordering, though the FixedFile branch is reachable if
  precedence ever shifts) overrode it.
- `ResolverWarning::FixedEstimateOverlap { name }` — emitted when a
  name appears in both `[fixed]` and `[estimate]` of the same fit-toml.
  Resolution: `[fixed]` wins; the name is dropped from `estimate_set`
  with role `Fixed { reason: NotInEstimate }`.
- `ResolvedParameter.overrode_scenario: Option<ScenarioOverride>` and
  the new `ScenarioOverride { scenario, scenario_value }` struct.
  Populated alongside the `ScenarioOverridden` warning whenever
  applicable; future Step 9 (run.json provenance) will read this
  directly into the `parameters_provenance.overrode_scenario` JSON
  block.
- `print_warnings` now also runs a debug-only structural cross-check
  that warnings-of-override and `overrode_scenario` provenance name
  the same parameters. Catches a class of resolver bugs where one
  side was added without the other.

Tests (7 new, all green):

- `fixed_cli_override_of_scenario_emits_warning_and_provenance` —
  scenario `worst_case` sets `beta=0.3`; `--fixed beta=0.5` wins;
  warning emitted; `overrode_scenario = Some(ScenarioOverride
  { scenario: "worst_case", scenario_value: 0.3 })`.
- `scenario_applied_cleanly_does_not_emit_override_warning` —
  scenario wins cleanly; no warning, no provenance entry.
- `fixed_cli_matching_scenario_value_does_not_warn` — equal values
  → no warning even though source ends up `FixedCli`.
- `scenario_override_warning_formats_actionably` — stderr format
  names parameter, CLI flag+value, scenario name, and intended value.
- `fit_toml_fixed_estimate_overlap_warns_and_fixed_wins` — overlap
  warning emitted; param removed from `estimate_set`; role is
  `Fixed { reason: NotInEstimate }`.
- `fixed_estimate_overlap_warning_formats_actionably` — message
  names the param and both block names.
- `no_overlap_means_no_overlap_warning` — disjoint blocks → no
  warning.

Replaced the previous `fit_toml_fixed_does_not_warn_or_kick` test
(which asserted that the pathological overlap was silent) with
`fit_toml_fixed_does_not_emit_kickedfromestimate_warning` (asserts
only that tier-3 doesn't trigger the *kick-out* warning — the new
overlap warning handles the overlap case separately).

Total resolver tests: 26 passing. Workspace-wide
`cargo test --release --workspace --tests --exclude camdl-tests`:
586 + 200 + ... pass; only the 4 pre-existing
`time::tests::*_panics_in_debug` failures remain (expected, debug
asserts compiled out in release).

This commit is additive — does not touch precedence ordering, does
not regress the `bbc4d8d` spec-vs-proposal fix, does not change the
public resolver API for existing callers (eval.rs, pfilter.rs). All
that changed is the resolver now records and reports more
provenance/warning detail.

Remaining steps unchanged from the original status note below
(`Step 2: simulate + lineage migration`, `Step 4: survey`, `Step 5:
if2/profile/fit-run`, `Step 6: InitMethod variants`, `Step 7: M-1
CLI break`, `Step 8: shared help`, `Step 9: run.json provenance —
will read overrode_scenario`, `Steps 10/11/12: doc churn`).

## Update 2026-05-25 (Step 2 done): simulate + lineage migrated

`util::resolve_run_model` (the shared 200-line value-resolution
function used by both `simulate` and `lineage`) is now a thin
wrapper around `params_resolver::resolve_parameters`. The inline
tier-2-through-5 layering, the unknown-param checks, the
finite/bounds validation, and the external-table resolution all
live inside the resolver now — `resolve_run_model` just builds
`ParameterInputs` and forwards.

Three correctness issues surfaced during the migration and are
fixed in the same commit (commit message captures the detail):

1. **Intervention filter must run unconditionally** (resolver bug,
   silent-wrong-answer class). The resolver was skipping
   `apply_scenario_filter` when both scenario and adhoc enable/disable
   were empty, leaving toggleable interventions live. Both an
   integration test (`intervention_event_defaults::simulate_default_event_fires_intervention_does_not`)
   and a new unit test
   (`no_scenario_no_adhoc_still_drops_toggleable_interventions`)
   now pin the unconditional-filter contract.
2. **Multi-violation reporting**: `MultipleViolations(Vec<ResolveError>)`
   variant added so multiple bounds/finite errors surface together,
   matching the legacy `validate_parameter_values` ergonomics.
3. **Error-message wording**: single-quoted parameter names + "not
   finite (NaN or ±∞)" wording match the established convention
   across the codebase.

One intentional deviation from legacy precedence: `--param-vec`
moves from "between `--params` and scenario" to "tier 5 alongside
`--param`". Rationale in the commit message; no integration tests
pinned the old order, and the new mapping is consistent with the
proposal's "`--fixed` is highest" stance.

Test posture after Step 2:
- Resolver unit tests: 28/28 pass.
- `intervention_event_defaults`: 7/7 (was 6/7).
- `parameter_bounds_validation`: 10/10 (was 8/10).
- `scenario_runtime_application`: 2/2 (spec §1.3 pins).
- Workspace: 28 of 29 test groups green; only the pre-existing
  `time::tests::*_panics_in_debug` failures remain.

## Audit-checklist snapshot at end of this session

Per the proposal's §"Post-implementation audit" — partial because
Steps 4/5/6 aren't done yet.

1. **Sole writer of `model.parameters[i].value`.**
   `rg 'p\.value\s*=\s*Some' --type rust crates/cli/src/` → 20 hits.
   Of those, 5 are inside `params_resolver.rs` itself (the resolver,
   as expected). The remaining 15 live in `if2.rs` (Step 5),
   `main.rs` (the CAS-cache helper — parallel migration target),
   `profile.rs` (Step 5), and a few legitimate test-setup sites
   under `crates/sim/tests/`.
2. **Old resolvers fully removed.** Not yet.
   - `apply_params_file`: 3 active CLI callers (if2.rs, main.rs,
     profile.rs).
   - `FixedParams::resolve_with_model`: 2 active callers (survey.rs,
     profile.rs).
3. **Sole entry point per subcommand.** Done for `eval`, `pfilter`,
   `simulate`, `lineage`. Pending for `survey`, `if2`, `profile`,
   `fit run`, and the CAS helper in main.rs.
4. **Provenance round-trip.** Not yet — Step 9 introduces
   `parameters_provenance` block in `run.json`. The resolver now
   carries `overrode_scenario`, ready to thread into Step 9.
5. **Init-source coverage.** Not yet — Step 6.
6. **No alias shims.** Not yet — Step 7 (M-1 CLI break).

## Remaining work for next session

In priority order:

- **Step 4: survey migration.** Tricky because `survey` interleaves
  `estimate.start` bounds-drawing with `fixed` resolution and has
  its own `expand_from_scenario` helper. Read the survey body
  carefully before touching; the resolver may need a new tier for
  `[estimate].start` or the caller needs to layer fit-toml start
  values onto the resolver result.
- **Step 5: if2 + profile + fit run.** High inference-risk per
  CLAUDE.md. The `if2.rs:109-168` block has its own scenario-aware
  parameter pinning. Migration needs an LSE (load → resolve →
  scenarize → re-resolve?) re-read or a resolver extension to
  expose the post-scenario-pre-CLI value snapshot.
- **Step 6: InitMethod ADT.** `FromPrior`, `FromPosterior`,
  `FromMle`, `FromParams` variants + per-loader file schemas. The
  resolved-decision A (from-prior falls back to bounds-uniform)
  belongs here.
- **Step 9: run.json provenance.** `parameters_provenance` block
  using the existing `ResolvedParameter` / `ScenarioOverride`
  shapes. Should be straightforward — the data is already in the
  resolver output, just needs a serialization layer.
- **Step 7: M-1 CLI break.** After all subcommands are migrated.
- **Steps 8, 10, 11, 12: docs.** Mechanical.

The `--param-vec` precedence deviation (now tier 5) is the one
inconsistency that should be confirmed with the maintainer. No
integration tests pinned the old between-tiers position, but a
user relying on `scenario > --param-vec` semantics would see a
behaviour change. Recorded in commit `1de2cd2`'s message; flag
for review.

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

### Decision D — Scenario-vs-`--fixed-file` precedence: proposal contradicts spec

**This blocks Step 2 (`simulate` / `lineage` migration).** Flagging
explicitly because it is the highest-leverage ambiguity I hit.

The proposal lists the precedence as (last-wins):

  1. Model default
  2. Scenario
  3. fit-toml [fixed]
  4. `--fixed-file`
  5. `--fixed` CLI

But `docs/camdl-run-spec.md §1.3` documents the *current* simulate
precedence as:

```
params.toml (=`--fixed-file`)
  ↓ overridden by
sweep point
  ↓ overridden by
scenario params
  ↓ overridden by
--param CLI flags (=`--fixed` CLI)
```

i.e. **scenario beats `--params FILE` today**. The proposal §"What
this proposal does NOT touch" says: "The forward-sim precedence
order documented in `docs/camdl-run-spec.md §1.3` is preserved
exactly — the resolver is a refactor, not a semantic change to the
order." But the resolver's stated order above changes scenario's
position from tier 4-ish to tier 2.

This isn't a typo — the proposal's full text includes the
profile-likelihood ergonomic ("--fixed gamma=0.1 to hold gamma while
sweeping tau") which only makes sense if `--fixed` beats scenario.
But the spec test `scenario_runtime_application.rs::scenario_set_replaces_mu_value`
locks in the *opposite* order for `--params` vs scenario: it confirms
scenario beats `--params`.

Two ways to interpret the proposal:

  - **(A) The proposal IS a semantic change.** The "preserved
    exactly" sentence is overpromising — the proposal does change
    precedence to give CLI verbs (`--fixed-file`, `--fixed`)
    primacy over scenario. The scenario test must be re-baselined.
  - **(B) The proposal preserves spec semantics.** The stated tier
    list is loose; the real intent is to keep scenario above
    `--fixed-file` (matching the documented spec) and only put
    `--fixed CLI` above scenario.

My resolver currently implements interpretation (A) — the literal
proposal order. Test `scenario_runtime_application.rs` will
**fail** when `simulate` is migrated unless either the order is
changed to (B) or the test is updated to match (A).

This is a maintainer decision because the bug pre-existed and was
explicitly fixed (see `util.rs:824-826` comment: "The old code
applied scenario params first and let --params silently overwrite
them — a silent-wrong-answer bug caught by tests"). The proposal
is implicitly reverting that fix, which may be intentional (the
profile ergonomic warrants the change) or accidental (the
proposal's tier-list was written without consulting the spec).

**Provisional choice in resolver:** interpretation (A). Override
is a one-line swap (reorder tiers 2 and 4 in `resolve_parameters`)
if the maintainer prefers (B).

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
