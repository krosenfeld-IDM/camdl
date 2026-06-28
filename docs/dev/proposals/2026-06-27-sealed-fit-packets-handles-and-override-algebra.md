# Fitting-workflow ergonomics: fit handles, self-contained runs, and scenario/sweep overlays over the fit ensemble

Status: proposed Supersedes: the initial draft of this file (which proposed a
single "override algebra" unifying scenarios and sweeps; that unification is
unsound against the shipped precedence model — see §4). Relates-to:
`2026-06-27-scenario-aware-fit-predict.md` (the scenario overlay this builds
on), `2026-06-22-predictive-ergonomics.md` (`fit predict`,
`FitResult`/`ParamTreatment`, column stacking),
`2026-06-25-generated-quantities.md` (`quantities {}`),
`2026-06-25-counterfactual-contrasts.md` (the conditioned-fork prerequisites
Phase 4 must reconcile with). Relates-to: gh#322 (runs aren't self-contained —
the keystone enabling sub-piece).

## Problem

The modeler loop is **fit → {summarize, predict, scenario, sweep, compare}**,
referencing the fit each time. Three frictions make it harder than it should be:

1. **You can't name a fit the way you think about it.** A fit is referenced only
   as a run directory or a `fit.toml`; modelers think "my `jigawa-baseline`
   fit," not a path.
2. **A run isn't self-sufficient (gh#322).** `fit predict <run-dir>` recompiles
   the model from the _loose_ `.camdl` recorded in the config
   (`fit/predict.rs:858-860`); if that file moved, downstream verbs break. A run
   records the model's content hash (`run_meta.rs:495`) but not the model
   itself, so a run isn't portable.
3. **Overriding a fitted parameter has no ergonomic, typed story.** Scenarios
   overlay a fit in `predict`/`simulate`; sweeps vary a grid in `simulate`; the
   cVDPV2 modeling work produced substantial Python glue the CLI should absorb —
   extracting posterior point estimates to TOMLs, hand-running
   `pfilter --save-prequential` before `compare`, generating N per-region
   `.camdl`+`fit.toml` and bash-looping fits, re-deriving R̂ and quantities
   `fit summary` already computes. The shape of that glue is the spec.

The unifying idea: **a completed fit is a sealed, self-contained packet you name
once and apply verbs to.** Downstream operations are transforms over the
packet's posterior _ensemble_, never re-specifications of the model.

## What already exists (build on this, don't reinvent)

Grounding the design in the current tree (post-`3edf62b9`):

- **Handle resolution, two of four shapes.** `fit predict`/`fit summary` already
  resolve a run dir directly and match a `fit.toml` to its unique run (listing
  on ambiguity) — `fit/predict.rs:585-650` (`resolve_segment`).
- **Labels are captured and stored.** `fit run --label` (`args/mod.rs:889-896`)
  writes the label into the sidecar and `run.json` `provenance.label`
  (`fit/mod.rs:2226-2252`); there is a standalone `camdl label <hash> "<text>"`
  (`args/mod.rs:1356-1370`). What's missing is a **label→run lookup** for the
  fit verbs.
- **Hash-prefix resolution exists** — `cas_read::resolve_fit_prefix` /
  `resolve_stage_by_hash` with the `cas_index` accelerator
  (`browse.rs:1074-1222`), wired into `browse`/`show` but **not** into
  `fit predict`/`summary`/`compare`.
- **Per-parameter provenance exists and is serialized.** `ValueSource`
  (`ModelDefault | FitTomlFixed | FixedFile | SweepPoint | Scenario(name) | FixedCli`)
  is recorded per parameter and written to `run.json` `parameters_provenance`
  (`params_resolver.rs:127-205`, `run_meta.rs:162-188`).
- **The override layers are already distinct and ordered** (`3edf62b9`): the
  resolver tiers are model-default < `SweepPoint` (draw/sweep) < `Scenario` <
  `FixedCli` (`--param`) (`params_resolver.rs`), with one shared writer
  `resolve_parameters`. The engine already loops `Vec<ScenarioRef>` shared
  across `predict` and `simulate` and stacks a `scenario` output column
  (`engine.rs`, `quantity_output.rs:132-167`).
- **The verbs mostly exist:** `fit summary`, `fit table`, `compare`, `fit new`,
  `fit diff` are implemented (`fit/mod.rs`, `fit_table.rs`, `compare.rs`).
  `fit table` aggregates convergence + estimated params per fit but **not
  quantities** (`table_row.rs`).
- **Draws-source typing exists:** the `predictive-ergonomics`
  `FitResult`/`ParamTreatment` types already distinguish posterior vs plug-in;
  `simulate --draws posterior|prior|uniform|<file>` already enumerates the
  sources.
- **`batch` already archives the compiled IR** into its output dir
  (`batch.rs:592`) — the exact mechanism gh#322 needs for fits.
- **Run identity is factored** (`resolve.rs`, `runid`): `run_id` is a tuple of
  per-level digests; artifacts in a leaf are not hashed. Adding a file to a leaf
  is identity-neutral.

The net-new work is small and concentrated; most of this proposal is _wiring and
one new invariant_, not greenfield types.

## The model: a fit is a sealed packet referenced by a handle

A `SealedFit` is the self-contained bundle — model IR + data + priors +
posterior draws + diagnostics — that a completed fit produces. You reference it
by a `FitRef` handle and apply verbs; the verbs transform the packet's posterior
**ensemble**.

### Types (refined to the real code)

```rust
// 1. Handle — resolution IS the boundary (fallible). No infallible shape-classifier:
//    a bare hex string and a relative path are genuinely ambiguous, so ambiguity is a
//    typed outcome, not a heuristic guess.
enum FitRef {
    RunDir(PathBuf),     // results/fits/<run>/
    Config(PathBuf),     // a fit.toml → its unique run (or list)
    Label(String),       // @jigawa-baseline (leading '@' is the sigil)
    HashPrefix(String),  // b4aa952d
}
enum ResolveError { NotFound(String), Ambiguous(Vec<RunId>), NotSealed(RunId), ModelMissing(RunId) }
fn resolve(s: &str, store: &Store) -> Result<SealedFit, ResolveError>;
//   priority: '@' → Label; *.toml → Config; existing dir → RunDir; else hex → HashPrefix.
//   Ambiguous(candidates) is surfaced and listed git-style, never silently resolved.

// 2. The sealed packet — its constructor cannot succeed without the model. The model is
//    archived in the fit leaf at `fit run` time (§Phase 1), so no loose .camdl is needed
//    and the run is portable. `quantities` is NOT a field — it lives in `model`.
struct SealedFit {
    id:          RunId,
    model:       ModelIr,        // archived in the leaf; absence → ResolveError::ModelMissing
    data:        ObservedData,
    priors:      Priors,
    posterior:   Ensemble,
    diagnostics: Diagnostics,    // R̂, ESS, gate verdict, acceptance
}

// 3. Draws source — "which parameter values" is orthogonal to "which model+data".
//    Posterior carries a handle, not the whole packet (the engine already holds the fit).
enum DrawsSource {
    Posterior(RunId),            // the posterior cloud — joint uncertainty
    Prior { priors: Priors, n: usize },
    Point(ParamVec),             // a single θ (MLE/MAP/manual) — no uncertainty
    File(PathBuf),               // a draws TSV
}
//    Introduce DrawsSource only when it can SUBSUME the existing `ParamSource`
//    (Single/Sweep/Draws) — not as an early third parallel draws-typing enum
//    (it would overlap predictive-ergonomics' `ParamTreatment` + `ParamSource`).

// 4. The ensemble carries uncertainty AND per-PARAMETER provenance (reusing ValueSource,
//    not a parallel enum). Provenance is per-parameter, never per-(draw,param): within an
//    ensemble a parameter's source is the same for every draw.
struct Ensemble {
    draws:      Vec<Map<Param, f64>>,   // the cloud (values only)
    coords:     DesignCoords,           // (scenario?, sweep values?) — the output keying axes
    provenance: Map<Param, ValueSource>,// one tag per parameter, from the resolver
}
```

The **one new invariant** worth the types: a predictive or quantity band can
only be built from an `Ensemble`, never from a collapsed point — **you cannot
silently drop posterior uncertainty.** Banding is `fn band(&Ensemble, ...)`;
there is no `band(point)`.

### 4. Scenarios and sweeps: sibling overlays, one mental model, distinct precedence

The ergonomic goal is that **scenarios and sweeps feel like one thing** —
overlays you _stack over a fit_ and read back as output columns. They share a
primitive and a mechanism, but they are **not the same operation**, and the
design must keep them distinct where the runtime already does:

- **Shared primitive — the value patch.** Both ultimately express `set p = v` /
  `scale p *= f` on a parameter. That primitive is unified:
  `struct ParamPatch { set, scale }`.
- **Shared mechanism — `apply` over the ensemble + output stacking.** Both
  rewrite only the parameters they touch, leaving the rest of the posterior
  cloud intact (so un-overridden parameters propagate their joint uncertainty
  automatically — this already works in `predict`), and both surface as
  `DesignCoords` columns (generalizing the `scenario` column to `sweep:<param>`
  columns).
- **Distinct layer — precedence.** A **scenario** is a counterfactual σ-layer
  overlay (cardinality 1 per parameter) at the **scenario tier**; a **sweep** is
  an automated M-variation grid axis (cardinality N) at the **lower draw/sweep
  tier**. The shipped precedence (`params_resolver.rs`, run-spec §1.3) is model
  < draw/sweep < **scenario** < `--param`. This ordering governs the
  relationship to the _fitted draws_ (any overlay beats the posterior value) and
  the layering of _distinct_ parameters. They are deliberately separate sum-type
  variants (`ParamSource::Sweep` vs the scenario overlay), "never conflated."

**Same-parameter collisions are a hard error, not a silent precedence.** When a
scenario `set`/`scale`s a parameter that a sweep _also_ varies, the request is a
contradiction — "pin `p`" and "vary `p`" at once — and _either_ precedence
silently corrupts it: scenario-wins collapses the sweep to a single value;
sweep-wins silently nullifies a named counterfactual. So this is **rejected with
a located error** naming the parameter, the scenario, and the sweep (mirroring
the explicit-`--draws`-file collision guard already in the engine), telling the
user to drop the parameter from one side. The precedence never silently resolves
a same-parameter contradiction; it only orders _distinct_ parameters and the
overlay-vs-draw relationship.

The "which parameters does a scenario touch" footprint must be computed by **one
shared `scenario_param_footprint(model, scenario)`** that both the resolver's
scenario tier and the collision guard consume — covering `set` ∪ `scale`,
composed presets, and inline overlays. If the guard computes the footprint
separately it can drift from what the resolver actually overwrites, and a
collision slips through (the swept value silently overwritten — the exact
corruption the rule exists to prevent). Factor it once before the guard lands.

So `scenario × sweep` on **distinct** parameters is a **Cartesian product of two
axes** (scenario overlay applied within each grid cell) — exactly what the
engine's `scenario × param-point × replicate` grid already produces, not a
single flattened `Vec<Override>`. Folding `Sweep` into the scenario `set`
variant would erase the precedence the runtime just hardened _and_ hide the
collision; we do not. The unification the user sees is at the **UX +
output-stacking + shared-patch** level, not a single operation.

```rust
struct Scenario {           // the σ-overlay (tier: scenario)
    name:    ScenarioId,
    patch:   ParamPatch,    // set / scale
    enable:  Vec<InterventionRef>,   // structural toggles, applied unconditionally
    disable: Vec<InterventionRef>,
}
// a sweep stays ParamSource::Sweep { points, .. } — the grid axis, the lower tier. unchanged.
```

## `predict` vs `simulate` — the seam

- **`fit predict`** = the _sealed, canonical_ product. The ensemble is
  **always** the posterior (optionally with scenario overlays and, as a later
  phase, sweeps). The observation-space, posterior-only specialization.
- **`simulate`** = the _engine_. Any `DrawsSource` × scenario overlays × sweep
  grid.

```
predict  : (SealedFit, scenarios, [sweeps], horizons, n_draws)
           -> PredictiveArtifact { predictive: Banded by (stream, horizon, scenario[, sweep:*]),
                                   observed: PerStream,
                                   quantities: Banded by (name, scenario[, sweep:*]) }

simulate : (ModelSource, DrawsSource, scenarios, sweep, backend, seed)
           -> SimArtifact { trajectories, obs, quantities: Banded by DesignCoords }
   where ModelSource = Sealed(SealedFit) | Loose { model: ModelIr, fixed: ParamVec }
```

## Output organization (downstream access)

A core goal is that **downstream consumers — R, plotting, another agent — find
and read outputs with zero glue.** Three commitments:

- **Everything lands under the fit and is addressable by the same handle — which
  requires making the fit envelope a discoverable record.** Today `fit predict`
  writes its artifacts into the fit _segment_ dir (`results/fits/<run>/`), which
  has **no `run.json`** and is therefore invisible to `show`/`cat`/`browse`/the
  index. Phase 1 gives the envelope a first-class record so
  `camdl show
  @jigawa-baseline` lists its outputs and
  `camdl cat @jigawa-baseline --stream
  predictive/cases` reads one. Without
  this, "the store is the API" does not hold — it is the keystone, not a
  freebie.
- **One tidy file per artifact, keyed by `DesignCoords` columns — never
  scattered per-scenario files.** `predictive/<stream>.tsv` and
  `quantities/<name>.tsv` carry leading `scenario` / `sweep:<param>` / `horizon`
  columns; a consumer reads the single file and `group_by`s the coordinate
  columns. (This generalizes the `scenario`-column stacking `predict` already
  does — N scenarios × an M-point sweep is still one file per stream/quantity,
  not N×M files.) This holds for `predict` today; the `simulate` sweep path
  still writes per-cell leaves with no scenario/sweep columns, so Phase 3 folds
  those into `ensemble.tsv` columns too.
- **A manifest describes the shape so consumers discover, not parse.**
  `quantities.json` exists today; a companion **`predictive.json` is net-new and
  required** — per stream it records the value kind, the band quantiles, and
  **which columns are coordinates**
  (`["scenario", "sweep:<p>", "horizon",
  "treatment"]`) vs value/band columns,
  so a reader joins without reverse-engineering headers. `run.json` +
  `fit.meta.json` carry diagnostics + per-parameter provenance + the dim/stream
  labels for the join.

The payoff: cross-fit / cross-scenario operations become **store queries, not
Python.** `compare @a @b` reads each sealed fit's stored prequential (Phase 2)
instead of a hand-run `pfilter`; `fit table --quantity R0 'fits/*'` reads `R0`
across a glob of sealed fits from their stored quantities. The store — keyed by
handle, tidy by `DesignCoords`, self-described by manifests — _is_ the
downstream API.

## CLI surface (extend what exists)

| op                     | input                                                         | output                                                        | status                                                              |
| ---------------------- | ------------------------------------------------------------- | ------------------------------------------------------------- | ------------------------------------------------------------------- |
| `fit run`              | `Config`                                                      | `SealedFit` (model IR archived in the leaf)                   | exists; **add IR archival** (Phase 1)                               |
| `fit summary`          | `FitRef`                                                      | `Summary { convergence, posterior_table }`                    | exists; **accept `@label`/hash** (Phase 1)                          |
| `fit predict`          | `(FitRef, scenarios, horizons, n_draws)`                      | `PredictiveArtifact`                                          | exists; **accept `@label`/hash** (Phase 1); **`--sweep`** (Phase 3) |
| `simulate`             | `(ModelSource, DrawsSource, scenarios, sweep, backend, seed)` | `SimArtifact`                                                 | exists                                                              |
| `compare`              | `(Vec<FitRef>, baseline, metrics)`                            | `ComparisonTable { elpd, Δelpd, E_T, CRPS, PIT }`             | exists; **auto-derive prequential** from each sealed fit (Phase 2)  |
| `fit table`            | `(Glob, Vec<QuantitySelector>)`                               | `CrossFitTable { id+label, convergence, params, quantities }` | exists; **add quantities** (Phase 2)                                |
| `fit new` / `fit diff` | —                                                             | `Config` / `ConfigDiff`                                       | **already shipped**                                                 |

## Worked workflows

1. **fit → predict.** `fit run --label jigawa-baseline`;
   `fit predict @jigawa-baseline` → posterior-predictive (free-forward +
   one-step), banded.
2. **fit → scenarios, carrying uncertainty.**
   `fit predict @jigawa-baseline --scenario no_sia
   --scenario earlier_sia` →
   predictive bands per scenario. Each scenario rewrites only its `set`/`scale`
   parameter per draw; the rest of the posterior cloud propagates, so the bands
   are the correct counterfactual posterior-predictive (not a point replay).
   `predict` refuses to silently default a parameter the posterior doesn't
   cover.
3. **scenario × sweep (design grid).**
   `simulate model --fit @jigawa-baseline --draws
   posterior --scenario no_sia --sweep sia_offset=0,30,60,90`
   → quantities keyed by `(scenario, sia_offset)`, banded over the posterior of
   the rest — the scenario overlay applied within each grid cell. Here `no_sia`
   and `sia_offset` are _different_ parameters, so they compose; a scenario and
   a sweep that touch the _same_ parameter is a hard error (the collision rule
   above), never a silent collapse.

## Phased plan (identity-risk noted per phase)

**Phase 1 — self-contained, nameable, discoverable runs (the keystone).**

- **1a — archive the compiled IR in the fit leaf** (mirror `batch.rs:592`). An
  _artifact_ addition, **not** a hashed identity input → **no goldens move, no
  run_id re-key** (only mechanical update: tests asserting a leaf's exact file
  set). `fit predict` resolves the model from the archived IR, falling back to
  the loose `.camdl` only if absent.
- **1b — `@label` / hash-prefix handles.** Replace the per-verb
  `resolve_segment` (`fit/predict.rs:585`) with one fallible
  `resolve(&str) -> Result<SealedFit, ResolveError>` that subsumes its
  RunDir/Config logic and adds `@label` (a new label→run lookup over sidecar
  labels) and hash-prefix (reuse `cas_read::resolve_fit_prefix`); wire it into
  `fit predict` / `summary` / `compare`. The `[FIT]` / `--fit` args change type
  `PathBuf → String` (a `@label`/hash is not a path). Read-side → zero identity
  risk.
- **1c — make the fit envelope discoverable.** The segment dir gets a
  first-class record (a non-hashed `ArtifactKind::Fit` envelope, or teach
  `show`/`cat`/`browse` to project the segment) so a fit's outputs
  (`predictive/`, `quantities/`, …) are listable and readable by handle. This is
  what makes "the store is the downstream API" true; it is a `runid`/`cas`
  change — deliberate, but still identity-neutral for existing runs.

**Phase 2 — kill the glue.**

- **2a (clean) — `compare` auto-derives `Prequential`** from each `SealedFit`
  (run a pfilter from the sealed packet on demand) instead of requiring a manual
  `pfilter --save-prequential`. Main friction: factoring fit-config loading so
  `compare` can reconstruct the filter inputs.
- **2b (bigger than it looks) — quantities in `fit table`.** `fit table` is a
  read-only projection of saved results today; surfacing a quantity per fit
  needs a **trajectory re-simulation per draw** (the evaluator is reusable, the
  re-simulation is not free), plus an IF2-has-no-draws edge case and a
  `TableRow` schema bump. Gate / lazy-evaluate it; this is the phase's real
  lift.

**Phase 3 — scenario/sweep parity in `predict` (medium; extends the just-shipped
resolver).**

- Add `--sweep` to `predict`; add `sweep:<param>` output columns (generalize
  `DesignCoords`).
- Implement by **extending** the shipped resolver tiers and the engine's
  `scenario × param-point` grid — never re-forking precedence. Keep all overlay
  annotation (scenario/sweep columns, provenance) **out of `run_id`** (the
  `scenario` column already sets this precedent — it lives in the path +
  `run.json`, never the identity hash).

**Phase 4 — typed ensemble + the can't-drop-uncertainty invariant (defer;
reconcile first).**

- Promote the draws cloud to `Ensemble` with `band(&Ensemble)` the sole banding
  entry point (the new invariant), reusing `ValueSource` for per-parameter
  provenance.
- This touches the draw representation deep in the runtime and **overlaps the
  deferred counterfactual-contrasts prerequisites** (keyed joint `(θ, X)`
  output). Do not start until that representation is designed. **Hard
  constraint:** provenance/`DesignCoords` are output annotation only; if they
  ever enter a hashed level they re-key the sim store — they must not.

## Decisions recorded

- **gh#322 mechanism: archive the compiled IR in the fit leaf**, not a new CAS
  hash→bytes store. The model _hash_ exists but no store maps it back to bytes;
  leaf-archival is the cheaper path, matches `batch`, is identity-neutral, and
  makes runs portable across machines.
- **Scenarios and sweeps stay distinct layers** sharing a `ParamPatch` primitive
  and the `apply`-over-ensemble + output-stacking mechanism — **not** one
  `Override` operation. The precedence (model < draw/sweep < scenario <
  `--param`) is preserved as shipped, and a **same-parameter scenario × sweep
  collision is a hard error** (a contradiction; neither silent precedence is
  acceptable).
- **Outputs are organized for zero-glue downstream access:** content-addressed
  under the fit, addressable by the same handle, one tidy file per artifact
  keyed by `DesignCoords` columns (never scattered per-scenario files),
  self-described by a manifest. Cross-fit/scenario operations are store queries,
  not Python.
- **Provenance is per-parameter and reuses `ValueSource`** — no parallel
  `Provenance` enum, no per-(draw,param) tags (a parameter's source is constant
  across an ensemble's draws).
- **Handle resolution is the fallible boundary** — one
  `resolve(&str) -> Result<SealedFit, ResolveError>` that **subsumes** the
  current `resolve_segment` (RunDir/Config) and adds `@label`/hash; `@` is the
  label sigil; `Ambiguous` is a typed, listed outcome. The `[FIT]`/`--fit` args
  become `String`, not `PathBuf`.
- **The fit envelope becomes a discoverable record (Phase 1c)** so outputs are
  listable/readable by handle — a deliberate `runid`/`cas` change, identity-
  neutral for existing runs. "Store is the API" is not free; it is the keystone.
- **One shared `scenario_param_footprint(model, scenario)`** feeds both the
  resolver's scenario tier and the collision guard, so the guard can never
  disagree with what the resolver overwrites (covers set ∪ scale, compose,
  inline).
- **A `predictive.json` manifest is added** alongside `quantities.json`, naming
  which columns are coordinates vs value/band — the join contract for
  downstream.
- **`DrawsSource::Posterior` carries a `RunId`**, not the whole `SealedFit` (one
  authoritative path to the model); and `DrawsSource` is introduced only when it
  can **subsume** `ParamSource`, not as an early parallel enum.
- **The typed `Ensemble`/can't-drop-uncertainty invariant is Phase 4**, gated on
  the counterfactual-contrasts representation; everything before it ships on the
  existing draws representation.

## Test plan

- **Phase 1:** a fit run archives its IR; `fit predict @label` and
  `fit predict <hashprefix>` resolve the model from the leaf with the loose
  `.camdl` deleted/moved; an ambiguous label lists candidates and errors;
  `run_id` is byte-identical before/after IR archival (a pinned identity test).
- **Phase 2:** `compare @a @b` with no pre-existing `prequential.json` succeeds;
  `fit table
  --quantity R0` emits a quantity column across a glob of fits.
- **Phase 3:** `predict --scenario S --sweep q=…` (distinct params) keys output
  by `(scenario, sweep:q)` and composes; a scenario and a sweep on the **same**
  parameter is a hard error naming the parameter/scenario/sweep; no sim-leaf
  `run_id` moves from adding the sweep column.
