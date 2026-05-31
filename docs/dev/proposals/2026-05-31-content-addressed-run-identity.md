---
status: proposal
date: 2026-05-31
tracking: gh#147
---

# Content-addressed run identity: pure functions, factored hashes, readable paths

Implementation brief. Every artifact camdl produces is the output of a pure
function of a complete, typed input set; its identity is the structural hash of
that set. The store path is a **readable, nested factoring** of that hash — not
a flat blob dir — so `list`/`show`/`cat` keep working as they do today. This
refactor also overhauls the **write path**: all outputs are committed through
one atomic, checksummed store, replacing the ad-hoc existence checks and
non-atomic writes that exist today.

## The model

```
f : Inputs → Artifacts
```

- **Pure** — output depends only on declared inputs; no hidden reads of clock,
  environment, working dir, or unseeded randomness.
- **Total** — defined for every valid input; failures are values, not panics.
- **Deterministic** — identical inputs → byte-identical output.

Given those, an artifact's identity is the structural hash of its inputs, and
the cache key *is* that hash:

```
resolved, complete, typed input value  ==  cache key
```

Content-addressing is sound **iff f is pure**: the failure mode is always "f
reads an input the schema didn't enumerate," so two materially different runs
hash equal and the second is served the first's output — a silent wrong answer.
The derive macro makes "enumerate every input" an invariant of the type system,
not a thing humans maintain.

### Identity is a *factored* hash, not one flat blob

The leaf's identity is the **ordered tuple of per-level hashes along its path**.
Each path level hashes a disjoint slice of the input set; the union of all
levels is the complete input set (so it is still "hash everything," just
partitioned for legibility and grouping). Each path segment is
`{readable-label}-{hash8}`: the label is **provenance** (a rename → a new dir →
a cache miss, which is harmless — you recompute, never serve a wrong answer);
the `hash8` is identity. Eight hex chars per segment is enough — a collision
needs *every* level on the path to collide simultaneously, and `run.json`
records the full 64-char hashes for verification. Readability beats
rename-reuse, so labels stay in the path.

### Determinism: forward sim + obs now; inference after two engine fixes

Forward simulation (gillespie/tau-leap/chain-binomial/ode) and synthetic
observations are byte-deterministic and pinned by `gate_trajectory_baseline.rs`
— content-addressable as soon as the hashing is fixed. Inference is **not yet
safe to content-address** until two engine impurities are removed (milestone M3
below):

- **Wall-clock degeneracy watchdog** — `degeneracy.rs:119,141` aborts the
  particle filter based on *elapsed time* even when the effective sample size is
  healthy, so a fit's log-likelihood depends on machine speed and thread count.
  Two runs with identical inputs can diverge. CAS runs must disable this
  watchdog (run with budget `None`); the safety it provides (catching a wedged
  filter) is recovered by an *iteration*-based bound, which is deterministic.
- **Resume is not reproducible** — PGAS resets the master RNG to t=0 on resume
  and skips `simulate_reference` (`pgas.rs:1279,1320`); PMMH forks the stream
  (`pmmh.rs:333`). A resumed run is therefore *not* byte-identical to a one-shot
  run of the same total length. **Resolution: a resumed run is a distinct
  artifact.** `--resume` + the new `target_length` are part of the input, so the
  resumed run gets its own identity and the original completed run is left
  untouched (the whole original trajectory is preserved). We do *not* attempt to
  make resume byte-equal a one-shot — distinctness is the safe choice.

Until M3 lands, every inference artifact — fit stages, `if2`, `pfilter`,
`survey`, `profile` — is wired but gated behind these fixes; forward-sim + obs
go first (M2).

## The general theory of invalidation

A field is **invalidating** iff changing it changes the output. **Default =
include; exclusion is explicit and audited** (`#[run_input(provenance)]`),
because the cost is asymmetric — over-invalidation is a bounded, visible,
self-correcting recompute; under-invalidation is an unbounded, invisible,
corrupting wrong answer. Changing a field's policy bumps its type's
`SCHEMA_VERSION` (folded into that type's hash).

- **Semantic** (hashed): model structure, params, scenario delta, output
  schedule + horizon, seed, engine version, upstream artifact identities, and
  any flag that changes computed values — including **`--allow-degenerate-rates`**
  (it changes collapse handling from hard-error to silent-zero, changing
  trajectory values; a control-looking flag that is genuinely semantic).
- **Provenance** (recorded in `run.json`, never hashed): argv, source paths,
  labels, timestamps, thread count (`--parallel`), cache-control (`--force`,
  `--dry-run`), output destination (`-o`, `--stdout`), and **pure presentation**
  — `--dates`, obs wide-vs-dir layout, `--format`. A path used only to *load* a
  value is provenance; the loaded value is semantic.
- **Presentation rule:** identity is the canonical *values*, not a rendered
  file. `--dates`/format/obs-layout are rendered on `cat` from the stored
  values; they never enter a hash. (What *is* semantic is the output *schedule*
  + horizon and the obs *streams* + *schedule* — which values exist.)

## The canonical hashing algorithm

This is the load-bearing contract: get it wrong and hashes are unstable or
unsound. One fixed 256-bit hash, pinned as `runid::HASHER` (BLAKE3 recommended;
SHA-256 acceptable if `sha2` is already in the tree). A global
`HASH_VERSION: u16` is folded into every root hash, so the function or encoding
can be migrated with a single bump (which invalidates the whole store — fine at
alpha). `ContentHash` is `[u8; 32]`.

The `CanonicalHasher` wraps the hash state; the derive macro and any hand impl
obey these rules so that equal *values* always produce equal bytes:

- **Domain separation.** Each `RunInput` type writes, first, a stable type tag
  (its fully-qualified type name, length-prefixed) then its `SCHEMA_VERSION`.
  Two structs with coincidentally-identical field bytes cannot collide, and a
  per-type policy change bumps only that type.
- **Length-prefixing.** Every variable-length value (string, byte slice, `Vec`,
  map, set) writes its element count as `u64` LE before its elements. This kills
  the concatenation ambiguity `("ab","c") == ("a","bc")`.
- **Primitives.** Integers as fixed-width little-endian; `bool` as a single
  `0`/`1` byte; `char` as `u32`.
- **Floats.** Only via `FiniteF64`, which rejects `NaN`/`±Inf` at construction
  and normalizes `-0.0 → +0.0`; hashed as its 8 IEEE-754 bits (LE).
- **`Option`.** Tag byte `0` = `None`; `1` then payload = `Some`.
- **Enums.** Variant index (`u32` LE, declaration order) then payload.
- **Maps & sets.** Iterated in **sorted key order**, count-prefixed.
  Load-bearing: `rate_grad` is a `HashMap` inside the IR, so any `HashMap` is
  collected and sorted before hashing.
- **Nested `RunInput` fields.** Hashed compositionally via `hash_into` — one
  pass, no intermediate digest.
- **`ArtifactRef` (lineage).** Hashed as its 32 identity bytes, *not* by
  re-walking the upstream inputs — the upstream's identity already transitively
  summarizes them. This is what makes `deps` cheap and statically computable.
- **`#[run_input(provenance)]` fields.** Skipped entirely.

Three derived quantities:

- **Level hash** — each path level (`ModelDigest`, `SimConfig`, …) has its own
  `ContentHash`; the segment's `hash8` is its first 4 bytes as hex.
- **`run_id`** — the leaf's address: `hash(HASH_VERSION ++ kind_tag ++ [level
  hashes in path order])`. One 32-byte id per leaf, stored in `run.json`.
- **`hash8`** is for the path (human reading + grouping); **`run_id`** is for
  addressing (`show`/`cat` prefix match). Both, plus all full level hashes, live
  in `run.json`.

## The store: readable factored paths

`results/` partitions by artifact kind (`sims/`, `fits/`, `pfilters/`,
`profiles/`, `surveys/`) — the top "type" level. Below it, levels nest by the
natural config → parameters → scenario → seed hierarchy. The leaf identity is
the tuple of segment hashes; `run.json` at the leaf carries the full hashes,
deps, and provenance.

### Compiled IR lives at the model level — compile once, never recompile

`compile : (source.camdl, camdlc_version, compile-time table digests) → IR` is a
pure function, so the compiled IR is content-addressed like anything else. A
**compile cache** keyed by **source hash** stores the IR:

```
results/models/
  {source_h8}/                  # source_h = hash(.camdl bytes + camdlc_version + inlined-table digests)
    model.ir.json               # the compiled IR  (camdlc runs ONCE per distinct source, ever)
    model_hash                  # = hash(canonical IR); the run-tree's model-level identity
```

`Resolve` computes `source_h`, looks here first, and invokes camdlc **only on a
miss**. Every run then loads `model.ir.json` from the cache — no recompile, no
double-compile, and `Run` consumes the same IR bytes that were hashed. The
run-tree model level (`{stem}-{model_h8}`) references this IR by `model_hash`.

### `simulate` (and `batch`, which is many of these leaves)

```
results/sims/
  {model_stem}-{model_h8}/          # MODEL
    {backend_dt}-{config_h8}/       # CONFIG
      {param_label}-{params_h8}/    # PARAMETERS
        {scenario_slug}-{scen_h8}/  # SCENARIO   (baseline = 00000000)
          seed_{n}/                 # SEED (literal)
            run.json  traj.tsv
            obs/{stream}-{obs_h8}/  # obs sub-artifact, nested under its trajectory
```

| Level | Label (provenance) | Hash inputs (semantic, include-by-default) |
|---|---|---|
| model | model file stem | the **whole canonical IR** (incl. `output`, `simulation`, `origin`, `origin_rata_die`, `time_unit`) + `ir_version` + `engine_version` |
| config | `chain_binomial-dt1` | backend, dt, t_start, t_end, output schedule, calendar mode, `allow_degenerate_rates` |
| parameters | a param-set label | resolved base param **values** (canonical) + `--table`/`--param-vec` content digests |
| scenario | scenario slug | resolved enable/disable/set **delta** (id-set + canonical patch) |
| seed | — | u64 seed (literal in dir; also in the leaf hash) |
| obs (sub) | stream name | obs **streams** + **schedule** + `obs_seed` (layout/`--dates`/format are provenance) |

The ensemble (`--seeds`) is the set of `seed_*` dirs under one scenario node —
grouping falls out of the tree; no `GroupInput` needed for the common case.

### `fit` — staged pipelines, with execution order visible

Every fit is a pipeline of ≥1 named stages under one fit-wide directory, with
the stage level **ordinal-prefixed** so `ls` shows execution order:

```
results/fits/
  {fit_stem}-{fit_h8}/             # FIT-WIDE
    {NN}-{stage}-{stage_h8}/       # STAGE: NN = zero-padded topological position (01, 02, …)
      seed_{n}/
        run.json  chains…
```

| Level | Label | Hash inputs |
|---|---|---|
| fit | fit.toml stem | model (whole IR), data **content digests**, estimate spec, fixed, resolved priors, `engine_version` |
| stage | `NN-{name}` (`01-scout`, `02-posterior`) | stage config (algorithm, backend, dt, cooling, gates, resolved `--init`/`--regime`/`--rw-sd`), `target_length`, **`deps: [upstream stage identities]`** |
| seed | — | base seed |

`camdl if2` (and `camdl pmmh`-free, prior-free standalone fits) **desugar to a
one-stage fit**: a bare `camdl if2 …` builds an in-memory one-`if2`-stage fit
and runs the identical runner, writing `fits/{stem}-{fit_h8}/01-if2-{h8}/`. The
standalone command is thin sugar over the same code path — there is no separate
"one-off" shape and no `OneOffFitInput`. Every fit, however invoked, has the
same structure: ≥1 ordered `NN-stage` levels.

The **recursion is in `stage_h`, not the path**: `02-posterior`'s hash folds in
`01-scout`'s *identity* (`deps`), so the posterior invalidates whenever the
scout's inputs change. An upstream stage's identity is `hash(its inputs)` —
computable statically — so the whole stage DAG hashes up front; execution still
runs stages in topological order (a stage reads its upstream's θ̂ at runtime).
Sound only when the engine is pure (the M3 fixes), so an upstream identity
uniquely determines its θ̂.

`NN` is **provenance** — a readable linearization of the stage DAG for sorting;
the identity-bearing order lives in each stage's `deps`. Reordering two
independent stages doesn't change any hash, only their display ordinals.

### `pfilter` — loglik evaluation at fixed params

`pfilter` *scores* a model at given parameters; it does not estimate them. It is
its own kind — keeping "estimate params" (`fit`) and "score at fixed params"
unconflated, as `survey`/`eval` are already separate:

```
results/pfilters/
  {model_stem}-{model_h8}/
    {filter_spec}-{spec_h8}/       # data digests, fixed params, particles, backend, dt
      seed_{n}/
        run.json  filtering.tsv
```

### `profile` — grid nests by point/start

```
results/profiles/
  {model_stem}-{model_h8}/
    {profile_spec}-{spec_h8}/      # backend, dt, data digests, estimate/fixed, algorithm, cooling, sweep axes, iterations, starts
      seed_{n}/
        {param}={value}/           # one dir per grid point (readable)
          start_{k}/
            run.json
```

### `survey` — single leaf

```
results/surveys/
  {model_stem}-{model_h8}/
    {survey}-{survey_h8}/          # estimate bounds + fixed + scenario + eval(method,particles,reps) + n_points + seed
      run.json  landscape.tsv
```

## `run.json` — the run record

`run.json` is the leaf's metadata and the cache-validity gate. Hashes address
and verify; `provenance` and `inputs` are recorded-not-hashed (the readable
mirror that `show` renders). Concrete shape:

```jsonc
{
  "format_version": 1,
  "kind": "sim",                       // sim | fit_stage | pfilter | survey | profile_point | obs | projection
  "run_id": "9f3a…(64 hex)",
  "hash_version": 1,
  "ir_version": 7,
  "engine_version": "0.3.0+abc1234",
  "levels": [                          // the factored identity, in path order
    { "name": "model",  "label": "sir_basic",       "hash": "a1b2…", "schema_version": 1 },
    { "name": "config", "label": "chain_binomial-dt1","hash": "c3d4…", "schema_version": 1 },
    { "name": "params", "label": "base",            "hash": "e5f6…", "schema_version": 1 },
    { "name": "scenario","label": "baseline",        "hash": "00000000…", "schema_version": 1 },
    { "name": "seed",   "label": "1",               "hash": "7788…", "schema_version": 1 }
  ],
  "deps": [],                          // [{run_id, kind, path}] — lineage; fit stages list upstream stage ids
  "status": "completed",               // running | completed | failed
  "artifacts": {                       // checksum manifest — the integrity gate on lookup
    "traj.tsv": { "bytes": 40213, "blake3": "…" }
  },
  "inputs": { /* resolved param values, scenario delta, config — for display/audit, not hashed */ },
  "provenance": {
    "argv": ["camdl","simulate","…"],
    "label": null,
    "created_at": "2026-05-31T12:00:00Z",
    "finished_at": "2026-05-31T12:00:03Z",
    "host": "…", "camdl_version": "…", "thread_count": 8,
    "source_paths": ["models/sir.camdl"]
  }
}
```

```rust
pub struct RunRecord {
    pub format_version: u16,
    pub kind: ArtifactKind,
    pub run_id: ContentHash,
    pub hash_version: u16,
    pub ir_version: u32,
    pub engine_version: String,
    pub levels: Vec<LevelId>,            // { name, label, hash: ContentHash, schema_version: u16 }
    pub deps: Vec<ArtifactRef>,          // { run_id, kind, path }
    pub status: RunStatus,               // Running | Completed | Failed
    pub artifacts: BTreeMap<String, FileChecksum>,  // { bytes, blake3 }
    pub inputs: serde_json::Value,       // resolved-input summary; provenance, not hashed
    pub provenance: Provenance,          // argv, label, timestamps, host, version, threads, source paths
}
```

## `CasStore` — lookup and the atomic commit protocol

```rust
pub enum Lookup { Hit(RunRecord), Miss, Stale(StaleReason) }

pub trait CasStore {
    fn lookup(&self, path: &Path, expected: &LeafIdentity) -> Lookup;
    fn commit(&self, path: &Path, record: RunRecord, artifacts: Artifacts) -> Result<(), CasError>;
}
```

**Lookup.** A hit requires *all* of: `run.json` present; `status == Completed`;
recorded `run_id` + level hashes equal `expected`; every listed artifact present
with a matching checksum; `hash_version`/`schema_version` current. Any failure →
`Stale(reason)` (`Incomplete` | `HashMismatch` | `Corrupt` | `SchemaDrift`).
`Miss` and `Stale` both ⇒ recompute. `--force` skips lookup entirely and
recomputes + overwrites.

**Commit — mode A: atomic stage-then-rename** (sim, obs, pfilter, survey,
profile-point — single-shot). Write all artifacts + `run.json`
(status `Completed`, checksums) into a unique staging dir `results/.staging/{run_id}`,
fsync, then `rename(staging, final)`. Rename is atomic within a filesystem, so a
reader never sees a half-written leaf. If `final` already exists (lost a race) →
discard staging, treat as Hit. This *replaces* today's `traj.tsv`-existence
check and non-atomic batch writes.

**Commit — mode B: in-place `Running → Completed`** (fit stages — long,
streaming, resumable). Create the stage dir, write `run.json` with status
`Running` first (so a crash is detectable and the dir is greppable mid-run),
stream chains in as produced, then commit by writing `run.json.tmp` (status
`Completed`, checksums), fsync, and `rename` over `run.json`. The single-file
`run.json` rename is the commit point: the cache becomes valid exactly when
status flips to `Completed`. A crash leaves `Running` → next lookup returns
`Stale(Incomplete)` → recompute, or `--resume` reads the partial state and
produces a *distinct* resumed artifact (see Determinism above).

**Concurrency.** Mode A is race-safe by rename. Mode B does not deduplicate two
concurrent *identical* fits (rare); the loser recomputes — documented, not
guarded. No TTLs, no background GC.

## `Resolve` — the resolution pipeline

`Resolve::resolve(ctx) -> Vec<RunInput>` turns raw CLI/TOML into a vector of
fully-resolved leaf inputs. Shared steps:

1. **Compile-or-cache IR.** `source_h = hash(.camdl bytes + camdlc_version +
   inlined compile-time table digests)`. Look up `results/models/{source_h8}/`;
   on miss run camdlc once and store. Load the canonical IR → `ModelDigest`.
2. **Params.** Merge sources in precedence order (file < `--params` < `--param`
   < resolved `--param-vec`) into a canonical `BTreeMap<ParamId, FiniteF64>`;
   `--table PATH` → `DataDigest` (hash of *content*, not path). `NaN`/`Inf` →
   `ResolveError::NonFiniteParam`.
3. **Scenario.** Named scenario from the IR, or ad-hoc `--enable`/`--disable`/
   `--set` → a canonical `ResolvedScenario` delta (sorted id-sets + sorted
   patch). Baseline = empty delta → `scen_h8 = 00000000`.
4. **Config.** backend, dt, t_start, t_end, output schedule resolved to concrete
   cadence/times, calendar mode, `allow_degenerate_rates`.
5. **Expand.** Sweep grids / design draws / `--seeds` → the Cartesian product →
   one `RunInput` per leaf. `--draws {prior,lhs,sobol}` is hashed as its *design
   spec* (method, n, design seed, bounds) at the params level — the design (not
   the post-hoc sampled values) is the identity; the drawn values are recorded.

Every resolved value, never a raw path or unresolved preset, enters a hash.

## Addressing and the reader

- **`run_id` is the canonical address.** `show abc123` / `cat abc123` match a
  hex prefix of `run_id`; an ambiguous prefix errors with the candidates.
- **Generic tree walk.** Each kind declares its level names/depth via `Layout`,
  so the reader walks `results/` data-driven — no hard-coded 3-level depth
  (today's `browse.rs:690`), no dual-keying on `sim_hash` + path
  (`browse.rs:956`). Navigation reads `run.json`, never path segments.
- **Derived index.** A rebuildable `index.json` caches `(run_id → path, kind,
  label, status, created_at)` for fast `list`; the `run.json` files are truth,
  the index is a cache. This is what `manifest.json` becomes.

### Path-shape contract for downstream consumers

The path *shape* and level *contents* both change; consumers that parse paths
must adopt the contract (handed as a one-page diff; the maintainer applies the
separate repos):

- fit stage dirs gain the `NN-` ordinal prefix (`01-scout-{h8}`, not `scout-{h8}`);
- `pfilter` moves to its own `pfilters/` kind;
- `camdl if2` writes a one-stage fit (`fits/{stem}-{h8}/01-if2-{h8}/`);
- level hash *contents* change (model hash now covers the whole IR, etc.), so
  **all existing `results/` are cache-misses** — clear and re-run, no migration;
- **resolve runs by reading `run.json`** (`run_id`, `levels`, `kind`), not by
  parsing path segments. Known consumers: camdl-book `scenarios.qmd`,
  camdl-viewer `cas.py`.

## The type chain (inputs → store)

```
RawSimulateCli │ RawBatchToml │ RawFitCli │ …
        │  Resolve::resolve(ctx)        // compile-or-cache IR, resolve params/scenario/config, expand sweeps/draws/seeds
        ▼
   Vec<RunInput>                        // resolved leaf inputs; each carries its per-level digests
        │  ContentAddressed::content_hash()      // total pure fn (DERIVED), per level
        ▼
   Layout::store_path(root)             // factored, readable, nested {label}-{hash8} segments
        │  CasStore::lookup(path, leaf_identity)  // Hit{record} | Miss | Stale
        ▼  (Miss│Stale)
        │  Run::run(&input) -> Artifacts          // pure f → bundle of named outputs
        ▼
   Artifacts + RunRecord  →  CasStore::commit(...)  // atomic (mode A) or Running→Completed (mode B)
```

One generic driver runs every leaf; `fit` is a topo-ordered fold that threads
each completed stage's identity into the next stage's `deps`.

### Traits + per-level input structs (crate `runid`)

```rust
pub trait Resolve {
    fn resolve(self, ctx: &ResolveCtx) -> Result<Vec<RunInput>, ResolveError>;
}
pub trait ContentAddressed {
    fn hash_into(&self, h: &mut CanonicalHasher);
    fn content_hash(&self) -> ContentHash;
}
pub trait Layout {
    fn store_path(&self, root: &Path) -> PathBuf;   // factored {label}-{hash8} tree
}
pub trait Run {
    fn run(&self) -> Result<Artifacts, RunError>;   // one input → a bundle of ≥1 named outputs
}

// Per-level digests — each derives the macro, hashed include-by-default.
#[derive(RunInput)]
pub struct ModelDigest {
    pub ir: CanonicalIr,
    pub ir_version: IrVersion,
    pub engine: EngineVersion,
}

#[derive(RunInput)]
pub struct SimConfig {
    pub backend: Backend,
    pub dt: FiniteF64,
    pub t_start: FiniteF64,
    pub t_end: FiniteF64,
    pub output: OutputSchedule,
    pub calendar: CalendarMode,
    pub allow_degenerate_rates: bool,
}

#[derive(RunInput)]
pub struct ResolvedParams {
    pub values: BTreeMap<ParamId, FiniteF64>,
    pub tables: Vec<DataDigest>,
}

#[derive(RunInput)]
pub struct ResolvedScenario {
    pub enabled: BTreeSet<InterventionId>,
    pub disabled: BTreeSet<InterventionId>,
    pub patch: BTreeMap<ParamId, FiniteF64>,
}

// Leaf inputs compose the level digests; provenance fields are excluded from the hash.
#[derive(RunInput)]
pub struct TrajectoryInput {
    pub model: ModelDigest,
    pub config: SimConfig,
    pub params: ResolvedParams,
    pub scenario: ResolvedScenario,
    pub seed: Seed,
    #[run_input(provenance)]
    pub display: RunProvenance,
}

// Other leaves, same pattern:
//   SyntheticObsInput { trajectory: ArtifactRef, streams, schedule, obs_seed, .. }
//   FitStageInput     { fit: FitDigest, stage: StageConfig, deps: Vec<ArtifactRef>, target_length, seed, .. }
//   PfilterEvalInput  { model: ModelDigest, data: Vec<DataDigest>, params: ResolvedParams, particles, config, seed }
//   SurveyInput, ProjectionInput (lineage), GroupInput (derived summaries only)
```

`ArtifactRef` is another artifact's identity used as an input field (lineage).
`Artifacts` is a bundle (e.g. `{traj, event_log}` or
`{paths, filtering, prequential}`). `RunRecord` (above) is read by prefix
resolution and `show`/`cat`, never the path.

### `#[derive(RunInput)]` (crate `runid-derive`, `proc-macro = true`)

Generates `ContentAddressed` over all fields, include-by-default, honoring
`#[run_input(provenance)]` (skip) and `#[run_input(schema_version = N)]`
(per-type policy version). It emits `hash_into` following the canonical-hashing
rules above (type tag, schema version, length-prefixing, sorted maps, finite
floats, enum tags, `ArtifactRef`-by-identity) and a `content_hash` that finalizes
from `HASH_VERSION`. A field whose type is not `ContentAddressed` is a compile
error — you cannot forget to make an input hashable. The macro **replaces** the
hand-written `hashing.rs` functions; there is never a second implementation.
`runid` depends only on `ir`; `cli` depends on `runid`. (There is no `observe`
crate; obs logic lives in `sim` + `cli`.)

## Grouping

The path tree handles the common groupings (seed ensembles, profile grids)
natively. `GroupInput` is reserved for **derived summaries** whose inputs are
member identities — `compare` (`FoldElpd` over fit-stage prequential refs),
ensemble quantiles — added one `GroupKind` at a time, never a summary DSL.
Per-chain convergence (R̂/ESS) is computed *inside* the fit-stage artifact, not
as a group (chains aren't separate artifacts).

## What this replaces / fixes (current state, verified — gh#147)

- `cli/src/hashing.rs:31-35` — the `model_hash` allowlist omits `output`,
  `simulation`, `origin`, `origin_rata_die`, `time_unit` (live
  silent-wrong-trajectory: two models differing only in output cadence/`t_end`
  collide). Replaced by `ModelDigest` hashing the **whole** canonical IR.
- Hand-written `sim_hash`/`scen_hash`/`fit_stage_hash` collapse into derived
  per-level digests.
- Fit stale-reuse: `StartsFrom::Stage` serializes to the bare name
  (`fit/config_v2.rs:1360`); `fit_stage_hash` (`fit/provenance.rs:303`) keys on
  the name, not the produced θ̂ (computed at `fit/mod.rs:1452`, only recorded).
  Fixed by `deps`.
- Cache-hit divergence: single-run verifies the full hash (`main.rs:685`); batch
  trusts `traj.tsv`-existence (`batch.rs:862`) with non-atomic writes
  (`batch.rs:892`). Both become `CasStore::lookup` + an atomic commit.
- Stale comment `if2.rs:349` references a removed `camdl fit if2` subcommand —
  delete during M3 when `if2` desugars to a one-stage fit.

## Invariants the implementer MUST preserve

1. Identity is the factored hash of the *resolved* input. No raw path, `Option`
   on a required semantic field, or unresolved name/preset enters any level —
   `--param-vec`/`--table`/`--regime`/`--rw-sd auto`/`--time-format auto`/
   `--draws`/`[design.*]` are resolved to values/digests before hashing.
2. Include-by-default per level; provenance is the only exclusion, annotated.
3. Paths are readable nested `{label}-{hash8}`; labels are provenance, hashes are
   identity; navigation + display read `run.json`, never the path shape.
4. A cache hit requires `run.json` present, `status == Completed`, hashes match,
   artifacts present, checksums match, schema/hash versions current. Commit is
   atomic (mode A) or `Running → Completed` via single-file rename (mode B).
5. Forward-sim/obs determinism stays gate-green; inference is content-addressed
   only with the watchdog disabled and resume treated as a distinct artifact.

## What NOT to do

- No flat hash-only store — keep the readable nested tree.
- No 16-char path migration — 8-char segments + full hashes in `run.json`.
- No second hashing implementation; the macro replaces `hashing.rs`.
- No grouping engine / summary DSL — reserve `GroupInput`, add kinds singly.
- No bespoke "one-off fit" shape — `camdl if2` is a one-stage fit.
- Do not content-address fits before the M3 engine fixes land.
- Do not change `HASHER`/`HASH_VERSION` casually — it invalidates the store.

## Build order — one cleanup, four milestones

This lands as a single coordinated cleanup (alpha software: no interim stopgap
PR — gh#147 tracks the correctness hole until merge). The run-spec
CAS-default-output / `--stdout` work lands *after*, separately.

**M1 — the `runid` crate.** `CanonicalHasher` + `ContentHash` + `run_id` + the
`runid-derive` macro + `RunRecord`/`run.json` schema + `CasStore` (both commit
modes) + the per-level digest types. Property and golden-hash tests. No CLI
wiring yet.

**M2 — forward sim + obs.** Wire `simulate` and `batch` through
`Resolve → Layout → CasStore`; obs sub-artifacts; the compile cache. Delete the
`hashing.rs` hand functions and the batch existence-check path. Regenerate
golden files. Red→green for the gh#147 sim cache-key tests.

**M3 — engine purity + inference.** Disable the wall-clock watchdog for CAS runs
(iteration-based bound instead); make `--resume` a distinct artifact. Wire fit
stages (`NN-stage`, `deps`), `pfilter` (`pfilters/`), `survey`, `profile`;
`if2`/`pmmh`-free standalones desugar to one-stage fits. Fit-reproducibility and
resume-distinctness tests.

**M4 — addressing + reader.** Generic tree walker, `run_id` prefix resolution,
derived `index.json` (replaces `manifest.json`). Emit the path-shape contract
diff for camdl-book and camdl-viewer.

Deferred: additional `GroupKind`s, a compiled-IR GC, run-spec CAS-default output.

## Testing plan

- **Canonical invariance (property).** Permuting `HashMap`/insertion order of any
  map/set field → identical hash. Pins the sorted-iteration rule.
- **Float canonicalization.** `-0.0` and `+0.0` → same hash; `NaN`/`Inf` →
  construction error (no hash produced).
- **Golden-hash regression.** A fixed `RunInput` → a committed 64-hex; guards
  accidental encoding drift. The *only* legitimate way to change a golden hash is
  a `HASH_VERSION` bump.
- **Correctness (gh#147, red→green).** Model differing only in
  `output`/`t_end`/`origin`/`time_unit` → distinct key; `--allow-degenerate-rates`
  on a collapse-firing model → distinct key; `--dates` → same key; `from_mle`
  with a changed upstream θ̂ → miss; partial `traj.tsv` without a Completed
  `run.json` → not a hit (batch + single agree).
- **Concurrency.** Two threads commit the same sim → one wins the rename, both
  observe `Completed`, artifacts intact.
- **Stale handling.** Corrupt an artifact / truncate `run.json` / flip status to
  `running` → lookup returns `Stale` → recompute.
- **Determinism gates.** `gate_trajectory_baseline.rs` stays green; add a
  fit-reproducibility pin (same inputs, watchdog off → identical θ̂) and a
  resume-distinctness pin (resume → different `run_id`, base run untouched).
- **Addressing round-trip.** Commit N runs, resolve each by `run_id` prefix;
  ambiguous prefix errors with candidates.

## Error taxonomy

- `ResolveError` — `CompileFailed`, `ParamParse`, `NonFiniteParam`,
  `UnknownScenario`, `FileNotFound`, `BadDesignSpec`.
- `RunError` — wraps engine/backend errors with the leaf identity attached.
- `CasError` — `Io`, `ChecksumMismatch`, `SchemaDrift`. (A lost rename race is
  handled internally as a Hit, not surfaced.)

## Appendix A — CLI surface inventory

Every *semantic* argument has a home in a per-level digest; every
control/IO/presentation argument is provenance.

| Subcommand | Leaf | Notes |
|---|---|---|
| `simulate`/`sim` | `Trajectory` (+ seed ensemble = the `seed_*` set) | `--event-log` → an event-log sub-artifact in the bundle. |
| `simulate --obs*` | `SyntheticObs` under the trajectory | path + wide-vs-dir = provenance; streams+schedule = semantic. |
| `batch run` | many `Trajectory` leaves | sweep×scenario×seed = the tree; `[design.*]` → resolved draws; `geo` = provenance. |
| `fit run` | `FitStage` (pipeline → ordered `NN-stage` dirs) | `--init from_mle` → `deps`; `--resume` → `target_length`, distinct artifact. M3. |
| `if2` | a one-stage fit (`fits/{stem}-{h8}/01-if2-{h8}/`) | sugar over the fit runner; method/algorithm in the hash. M3. |
| `pfilter` | `PfilterEval` under `pfilters/` | scores at fixed params (not estimation); sibling kind to `survey`/`eval`. M3. |
| `profile` | grid leaves nested point/start | `--starts`/`--seeds`/`--sweep` are tree levels. M3. |
| `survey` | `Survey` (one landscape leaf) | per-point logliks are rows, not runs. M3. |
| `lineage realize/tree/sojourn/cohort` | `Projection` of an upstream artifact | `--identity-seed`/`--sample-seed`/scheme/window = semantic. |
| `eval` | `Eval` | pure fn of (model, params, `--expr`, grid). |
| `compare` | `Group(FoldElpd)` over fit prequential refs | `--baseline` = provenance. |

Non-CAS: `data split`, `label`/`list`/`show`/`cat`/status/metadata commands,
`compile`/`check`/`inspect` (delegate to camdlc) — these consume or display
artifacts rather than produce new ones.

Semantic-vs-provenance of the load-bearing flags: **semantic** — model, params
(`--param`/`--params`/`--param-vec`/`--draws`), `--table` content, scenario,
`--backend`, `--dt`, `--seed`, output schedule + horizon, obs streams+schedule,
**`--allow-degenerate-rates`**, `--identity-seed`/`--sample-seed`, and resolved
`--regime`/`--rw-sd`/`--init`/`--time-format`. **Provenance** — `--parallel`,
`--force`, `--dry-run`, `-o`/`--output`, `--stdout`, `--label`, `geo`,
`--resume` flag itself (the resulting `target_length` is semantic), `--dates`,
obs wide-vs-dir, `--format`/`--tsv`, `--no-dt-check`, `--suppress-warnings`,
`--progress`, `--verbosity`.
