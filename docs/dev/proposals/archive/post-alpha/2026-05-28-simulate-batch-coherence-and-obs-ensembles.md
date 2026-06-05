# Proposal: unify `simulate` and `batch run` under `SimulateJob`

Date: 2026-05-28 Status: Draft Area: CLI / CAS orchestration (no IR schema
change) Normative refs read before drafting:
[`docs/camdl-run-spec.md`](../../camdl-run-spec.md) §3.1 (`SimulateJob`), §3.1.1
(`ObsOutput`), §3.2 (`ParamSource`), §5 (batch invocations);
`rust/crates/cli/src/cas/mod.rs` (CAS layout);
`rust/crates/cli/src/params_resolver.rs` (scenario resolution);
`rust/crates/cli/src/batch.rs` (batch runner); `rust/crates/cli/src/main.rs`
(simulate path); `CLAUDE.md` §"RNG and paired-seed coupling";
`docs/dev/observations-system.md` (obs RNG derivation);
`ocaml/lib/compiler/parser.mly` (scenario `set`/`scale` grammar). Cross-checked
against the teaching narrative in `../../../camdl-book/guide/experiments.qmd`.

## Thesis

`camdl simulate` and `camdl batch run` should be **one engine over one job
type**, exactly as the run-spec already designed: `SimulateJob` is described in
§3.1 as _"THE type — CLI and file both produce this."_ Today they are two
hand-rolled arg→`SimRun` paths that have drifted, and that drift _is_ the bug
surface this proposal addresses. This proposal unifies them. The five findings
below are not five separate fixes — four of them are symptoms of non-unification
and fall out of the convergence; one is an orthogonal parser-error fix.

## Origin: five findings from a CLI-surface review

A review of the simulate/batch CLI surface against the CAS surfaced five issues.
The design test: simulate and `batch run` should both deposit _scannable,
consistent_ artifacts in the CAS, and downstream tooling should be able to read
both uniformly. Where that broke down, the design had a seam that didn't line
up.

| #  | Finding                                                                                | Class                       | Disposition under unification                               |
| -- | -------------------------------------------------------------------------------------- | --------------------------- | ----------------------------------------------------------- |
| 3  | `batch run` ignores model `scenarios{}`; `simulate --scenario` honors it               | code-vs-code bug            | **Fixed structurally** — shared resolution path             |
| 4  | No per-replicate observations in the CAS (batch has no `--obs`; `--cas` is single-run) | missing feature             | **Fixed** — `SimulateJob.obs` + CAS obs writer              |
| 5  | `--obs-only` single file can't hold multi-cadence streams                              | by-design + discoverability | **Partly fixed** — `ObsOutput::OnlyDir` lands with the enum |
| m1 | `set = { a=1, b=2 }` one-line → bare `E001`                                            | error-quality bug           | **Orthogonal** — parser fix, independent                    |
| m2 | `--obs-only` reads like a boolean                                                      | UX wart                     | **Folded into #5** — explicit `--obs-only-dir` sibling      |

Each finding is verified against the code below, with the reproduction and the
file:line cause, so the acceptance tests are concrete.

---

## Root cause (findings #3, #4, #5/m2)

The two entry points do not share a job type. The run-spec designed the
convergence — `SimulateJob` (§3.1) carries `source: ParamSource`,
`scenarios: Vec<ScenarioRef>`, `seeds: Seeds`, and `obs: ObsOutput` — but **none
of it is implemented**: grepping `rust/crates/` for `SimulateJob` / `ObsOutput`
/ `ScenarioRef` returns only a `batch.rs:11` comment calling them _"A future…"_.
So:

- **simulate** resolves named model scenarios (`params_resolver.rs:398-427`) and
  supports obs output (`--obs`/`--obs-dir`/`--obs-only`, `main.rs`).
- **batch** does neither: `batch.rs:560-576` hardcodes `scenario_name: None`
  (line 569) and only ever calls `write_traj_tsv` (line 589).

Two paths, divergent capabilities. That asymmetry is findings #3, #4, and the
missing `OnlyDir` half of #5.

### Verified causes (the acceptance tests)

**#3 — batch drops model scenarios.** A model whose `baseline` scenario supplies
`R0` via its `set { }` block:

```
camdl simulate model.camdl --scenario baseline           # works
camdl batch run b.toml   # [[scenario]] name="baseline"  # fails:
  Validation("parameter 'R0' has no value; supply it via --params or --param")
```

Cause: simulate with `scenario_name = Some` looks up `model.presets` and applies
the preset's `params`/`set` (`params_resolver.rs:398-427`); batch forces
`scenario_name: None` (`batch.rs:569`), so `params_resolver.rs:428-431` takes
the empty-scenario branch and the preset is never consulted. The dry-run still
prints `baseline
(baseline)`, implying a resolution that did not happen — a
misleading-output bug in its own right.

**#4 — no obs ensembles in the CAS.** `--cas` is single-run only
(`main.rs:503-513`); `batch run` has no obs fields (`args/mod.rs:583-609`) and
never samples observations (`batch.rs:589` writes only `traj.tsv`). So you can
get an ensemble of trajectories into the CAS but never an ensemble of
observations — which blocks the posterior-predictive /
fan-chart-on-the-observable view — the headline use case for reading the CAS.

**#5 — multi-cadence obs-only.** `main.rs:608-619` rejects multi-schedule
streams for single-file obs (correct). But `--obs-only DIR/` already routes to
dir mode (`main.rs:461-465`); it is just undocumented and the explicit
`ObsOutput::OnlyDir` flag (`--obs-only-dir`, run-spec §3.1.1) is unimplemented
(`args/mod.rs` has `obs`/`obs_dir`/`obs_only` only).

---

## The target design

### `SimulateJob` (run-spec §3.1)

Both entry points construct the same value:

- `camdl simulate …` CLI args → `SimulateJob` (run-spec §5.2 mapping).
- `batch run FILE` → deserialize TOML → `SimulateJob`.

A single `run_job(job: SimulateJob)` engine expands
`source × scenarios × seeds`, runs each cell, and writes the CAS tree.
`batch run` becomes a thin TOML front-end; the multi-run logic lives in one
place.

### Scenario references (`ScenarioRef`) — resolves #3

A `ScenarioRef` is **either** a model-preset name **or** an ad-hoc patch, never
both (mirroring simulate's `--scenario` vs `--enable/--disable` exclusivity at
`main.rs:486`, and matching the book, which treats `baseline` as a named model
scenario throughout `experiments.qmd`):

1. name matches a preset → resolve via the existing `params_resolver` preset
   path (no new logic — just stop forcing `None`);
2. name matches nothing but inline `enable`/`disable`/`params` present → ad-hoc
   patch;
3. name matches nothing and no inline fields → **hard error** listing available
   presets (catches the typo that is silently mislabeled today);
4. name matches a preset _and_ carries inline fields → **hard error** (the model
   scenario is the source of truth; edit it or rename).

Dry-run prints the _resolved_ params for named refs so it stops implying a
resolution that didn't occur.

### Observation output (`ObsOutput`) + CAS layout — resolves #4, #5

The CAS obs location is **already designed** in `cas/mod.rs:11-25`:

```text
seed_{n}/
  traj.tsv                      # trajectory (canonical, cached by sim+scen+seed)
  run.json                      # run metadata
  obs/                          # optional, one dir per (obs-model, obs-seed)
    {obs_hash}-{obs_seed}/
      <stream>.tsv              # one file per stream (multi-cadence safe)
```

The `obs_hash`/`obs_seed` split is deliberate and load-bearing: the trajectory
is the expensive cached artifact; the measurement model is a cheap re-sampleable
layer. Varying `obs_seed` draws fresh synthetic observations from one stored
trajectory without recomputing dynamics — exactly what a PPC fan chart wants.
`obs_hash` = hash of the resolved obs block only, so changing a reporting
parameter (`rho`) re-samples obs without invalidating the cached trajectory.

`run_job` implements the full `ObsOutput` enum
(`None`/`File`/`Dir`/`OnlyFile`/`OnlyDir`) once, for both entry points, reusing
`multi_stream_obs` / `sample_obs_resolved` (the code simulate's `--obs` path
already uses). Multi-cadence → one file per stream (no single-file kludge).
`browse.rs` (today aware only of `traj.tsv`/`summary.tsv`/`profile.tsv`) learns
to surface `obs/`.

This means an ensemble PPC view is just: walk
`sims/<model>/<scenario>/seed_*/obs/*/`.

---

## Gating risk: determinism must be byte-preserved

This is the careful-counterpart flag, and it is the reason this is a _staged_
landing and not a big-bang rewrite. **Rerouting `simulate` through a new engine
must not reorder RNG draws.** Per CLAUDE.md §"RNG and paired-seed coupling":
paired scenarios are byte-identical only while the RNG is consumed in the same
order on both sides, and _"any structural change that reorders draws also breaks
the coupling."_

Consequences for the plan:

- The golden trajectories in `ir/expected/*.tsv` are the tripwire. The
  unification commit that reroutes `simulate` **must leave every golden
  byte-identical without regenerating them.** If a golden changes, draws were
  reordered — stop and find the cause. Do **not** `make update-expected` to make
  it pass (CLAUDE.md: never lower the bar).
- Obs sampling already has a defined, separate RNG derivation —
  `obs_rng = StatefulRng::new(process_seed ^ SEED_MIX_OBS)`
  (`observations-system.md:126`). Item 2 reuses this; it does **not** invent an
  `obs_seed` scheme. `obs_seed` in the CAS path = the process seed (one
  realization per trajectory); a future `[obs] replicates = K` can fan the
  measurement layer by mixing K distinct obs seeds, still leaving the trajectory
  RNG untouched.
- The engine must thread one `StatefulRng` per (params, scenario, seed) cell in
  exactly the order the current per-path code does. The shared `run_job` is a
  _refactor of orchestration_, not of the per-step draw sequence.

---

## Staged landing plan

Each stage is independently committable and green; goldens lock behavior between
stages.

- **Stage 0 — types, no behavior change.** Introduce `SimulateJob`,
  `ParamSource`, `Seeds`, `ScenarioRef`, `ObsOutput` per run-spec §3. Pure
  additions; nothing routes through them yet. `cargo test` green.
- **Stage 1 — simulate → `run_job`.** `simulate` CLI constructs a `SimulateJob`
  and runs through the new single engine. **Acceptance: `ir/expected/*.tsv`
  byte-identical, untouched.** This is the determinism lock. Paired-seed CRN
  tests must still pass.
- **Stage 2 — batch → `run_job`.** `batch run` deserializes TOML into
  `SimulateJob` and uses the same engine; implement `ScenarioRef` resolution.
  **Acceptance: finding #3 reproduction now succeeds** and produces the same
  trajectory as `simulate --scenario` at equal seed (TDD red→green). Dry-run
  shows resolved params.
- **Stage 3 — obs across the engine + CAS.** Implement the full `ObsOutput` enum
  and the `seed_N/obs/{obs_hash}-{obs_seed}/` writer + `run.json` obs
  provenance + `browse.rs` surfacing + `--obs-only-dir` flag and the improved
  multi-cadence error. **Acceptance: finding #4 reproduction** — `batch run`
  with obs enabled deposits an ensemble of observation files discoverable from
  the CAS; #5/m2 resolved.
- **Stage 4 — orthogonal parser fix (m1).** Independent of everything above; can
  land any time. See below.

---

## Orthogonal: better error for comma-separated `set`/`scale` (m1)

**Class: error-quality bug.** `set = { a = 1, b = 2 }` on one line emits a bare
`E001` pointing at `b`, with no separator hint. Cause: `parser.mly:847` parses
the block as `list(scenario_kv_item)`; menhir `list(...)` has no separator, and
`scenario_kv_item` (`parser.mly:880-883`) is `IDENT [ [idx,…] ] EQ expr`.
Entries are newline-separated by convention (matching `recurring_body` at
`parser.mly:563` and the `parameters`/`compartments` blocks). A `COMMA` matches
no production.

Fix: keep newline separation (commas are reserved for `[...]` lists and `(...)`
arg lists — do not blur the list-vs-block distinction; "no loose semantics"),
and emit a hint when a `COMMA` appears where the next item is expected:

```
error[E001]: unexpected ',' in set { } block
  separate entries with newlines, not commas:
    set {
      a = 1
      b = 2
    }
```

TDD: negative golden in the OCaml dimcheck/parser suite.

---

## Acceptance criteria (all five findings)

1. `batch run` with `[[scenario]] name="baseline"` resolves the model preset and
   produces a trajectory byte-identical to `simulate --scenario baseline` at the
   same seed.
2. `batch run` (or `simulate` over an ensemble) deposits per-seed synthetic
   observations under `seed_N/obs/{obs_hash}-{obs_seed}/`, discoverable by
   walking the CAS.
3. `--obs-only-dir DIR` exists; the multi-cadence error names it and
   `--obs-dir`; `--help` documents that `--obs-only DIR/` also works.
4. `set { a = 1, b = 2 }` produces an `E001` with the newline-separator hint and
   corrected form.
5. Every pre-existing golden in `ir/expected/*.tsv` is unchanged across the
   entire series.

## Out of scope

- Lifting the `--cas` single-run restriction is _enabled_ by this work (the
  engine becomes multi-run-capable) but the CLI ergonomics of
  `simulate --scenario a,b --seeds 1:1000 --sweep …` landing in the CAS directly
  (run-spec §5) can follow as a separate ergonomics pass.
- No IR schema change. If the obs provenance in `run.json` needs new fields,
  that is a `run_meta` change, not an `ir/schema.json` change.
