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

> **This is an extract-and-harden, not a greenfield build.** A working CAS
> already exists: `cli/src/cas/typed.rs` (`ContentHash`, the `CasInputs` trait,
> `ReplicateSet`), `cli/src/cas/{sim,fit}_inputs.rs`, and `cli/src/run_meta.rs`
> (`Run`, `RunStatus`, `RunKind`, `CacheStatus`, `check_cache`, and an atomic
> `run.json.tmp → rename` writer). The `runid` crate is built by *extracting and
> hardening* these — replacing the canonical-string hashing in
> `cas/typed.rs::hash_canonical` with the structural `CanonicalHasher` below, and
> the existence/`check_cache` logic with `lookup`. Building a parallel hashing
> layer would violate the "no second implementation" invariant against
> `cas/typed.rs`, not just `hashing.rs`. See "Existing scaffolding" below.

> **The durability and exclusivity primitives this design assumes do not exist
> yet.** `rg 'sync_all|sync_data|fsync' rust/` returns nothing; the current
> "atomic" writer (`run_meta.rs:723-734`) is `fs::write + rename` with no
> barrier. Every "fsync" and "exclusive claim" in this document is *new code a
> milestone must write*, not a property of `rename`. Invariant #6 makes this
> explicit.

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
  (`pmmh.rs:330`, `seed ^ start_step`). A resumed run is therefore *not*
  byte-identical to a one-shot run of the same total length. **Resolution: a
  resumed run is a distinct artifact.** `--resume` + the new `target_length` are
  part of the input, so the resumed run gets its own identity. **This is a
  rewrite of the resume I/O, not a relabel:** today resume *mutates the original
  stage dir in place* — it appends to `trace.tsv` (`pgas.rs:492`,
  `trace_writer.rs:34-37`) and overwrites `resume_state.bin` (`pgas.rs:593`), so
  "the original is preserved" is **false against current code**. M3 must read the
  prior artifact's state *read-only* and write the resumed run into a *new* leaf
  (with a `dep` on the prior). The acceptance test is not just "resume →
  different `run_id`" but "the base run's bytes are identical before and after the
  resume."

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
- **Presentation rule — only sound because storage is canonical.** The stored
  artifact is a **canonical representation** (canonical TSV: numeric time, fixed
  column order, full precision), *not* the user-requested rendered file.
  `--dates`, `--format` (parquet/…), and obs wide-vs-dir layout render *views* of
  that canonical artifact at `cat`/`-o` time; they never enter a hash and never
  produce a distinct cached artifact. This classification is **valid only given
  canonical storage** — if the CAS stored the literal `--format parquet` bytes,
  those flags would be semantic. (What *is* semantic is the output *schedule* +
  horizon and the obs *streams* + *schedule* — which values exist.)

## The canonical hashing algorithm

This is the load-bearing contract: get it wrong and hashes are unstable or
unsound. One fixed 256-bit hash, pinned as `runid::HASHER` — default SHA-256
(`sha2` is already the only hashing dep in the tree; add `blake3` only if its
speed is needed for large-artifact digests). The *same* pinned function produces
both the input `ContentHash` and the artifact-manifest digests; the choice is
recorded by `HASH_VERSION: u16`, folded into every root hash, so the function or
encoding migrates with a single bump (which invalidates the whole store — fine at
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
- **Floats — two intentional policies, not two implementations.**
  - *Resolved user inputs* (params, dt, t_start, t_end, bounds): via
    `FiniteF64`, which rejects `NaN`/`±Inf` at construction (a non-finite param
    is a `ResolveError`, surfaced before hashing) and normalizes `-0.0 → +0.0`;
    hashed as its 8 IEEE-754 bits (LE). A user typing two spellings of zero
    should hit the same cache.
  - *Structural IR floats* (`ConstExpr.value`, init conditions, presets, prior
    params — all raw `f64` in the `ir` crate): hashed as raw `to_bits()` (LE),
    **distinguishing `±0.0` and NaN payloads**. This matches the IR's own
    `ConstExpr::PartialEq` (`expr.rs:81-91`), which uses `to_bits()` precisely so
    two ASTs differing only in zero sign or NaN payload are observably distinct.
    Routing IR floats through `FiniteF64` would erase a distinction the IR treats
    as real (a collision) and would reject NaN-bearing consts at hash time (a
    totality break). These are one hasher with a field-level policy, not a second
    implementation — the IR-float rule lives in the hand-written
    `ContentAddressed` impls for the `ir` type tree (see the macro section).
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
- **`run_id`** — the leaf's address: `hash(HASH_VERSION ++ kind_tag ++ count ++
  [level hashes in path order])`. The root derivation obeys the same framing
  rules as everything else: `kind_tag` is a **fixed-width enum index** (not a
  bare string) and the level-hash list is **count-prefixed** (`u64` LE), so two
  kinds with coincidentally-equal level sequences cannot alias. One 32-byte id
  per leaf, stored in `run.json`; a golden test pins this injectivity.
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

`compile : (source.camdl, camdlc_version, IR-affecting argv, files read) → IR` is
a pure function, so the compiled IR is content-addressed like anything else. The
**compile cache**:

```
results/models/
  {source_h8}/                  # source_h = hash(.camdl bytes + camdlc_version + IR-affecting camdlc argv)
    model.ir.json               # the compiled IR  (camdlc runs ONCE per distinct (source, version, argv))
    model_hash                  # = hash(canonical IR); the run-tree's model-level identity
    reads.json                  # camdlc side output: every file it read → its content digest
```

Two subtleties, both under-invalidation hazards if mishandled:

- **camdlc argv is part of the key.** camdlc accepts IR-mutating flags (`--set
  NAME=VALUE`, `--no-dim-check`), so `source_h` folds in the IR-affecting argv;
  inert flags (`-o`/`--pretty`/`--json-errors`/`--output`) are whitelisted out.
  Today the Rust CLI invokes `camdlc <path>` with no flags (`util.rs:344`); that
  invariant is pinned by a test so a future `--set` can't silently collide.
- **Table-file digests are discovered, not predicted.** A `.camdl` can `read(...)`
  external tables/CSVs at compile time, so the IR depends on those file *contents*
  — but Resolve cannot know *which* files without re-implementing camdlc's path
  resolution (the natural guess, and it is unsound). **Invert it:** camdlc emits
  `reads.json` (every path it opened + its content digest) as a side output. A
  cache lookup re-checks those recorded digests; a changed table file → miss →
  recompile. The read-set can only *grow* via a source edit (which changes
  `source_h`), so re-verifying the recorded set is sufficient. M2's compile-cache
  gate includes a **table-file-change → miss** red→green test.

`Resolve` computes `source_h`, finds a candidate, re-verifies `reads.json`
digests, and invokes camdlc **only on a miss**. Every run then loads
`model.ir.json` — no recompile, no double-compile, and `Run` consumes the same IR
bytes that were hashed. The run-tree model level references this IR by
`model_hash`.

**Compile-cache version ≠ run-identity version.** The compile cache keys on
`camdlc_version` (the *compiler*); the run-identity model digest keys on
`engine_version` (the *runtime*). A runtime-only engine change re-keys run
identity (resimulate) but does **not** invalidate the compile cache (no
recompile) — the compiled IR is unchanged. Keep these two versions separate so a
Rust-side change never reruns camdlc.

### `simulate` (and `batch`, which is many of these leaves)

```
results/sims/
  {model_stem}-{model_h8}/          # MODEL
    {backend_dt}-{config_h8}/       # CONFIG
      {param_label}-{params_h8}/    # PARAMETERS
        {scenario_slug}-{scen_h8}/  # SCENARIO   (empty delta → label "baseline")
          seed_{base}-{seed_h8}/    # SEED: segment carries the resolved-process_seed hash
            run.json  traj.tsv      # the leaf's OWN files (exact-set applies to these)
            obs/{stream}-{obs_h8}/  # DECLARED CHILD sub-artifact (own run.json; not an orphan)
```

**Every semantic level carries its hash in the segment — no level is label-only.**
The seed level was the lone offender: it hashes the resolved `process_seed`
(below), so its segment must be `seed_{base}-{seed_h8}`, not `seed_{n}`. Otherwise
lone `--seed 42` and the `beta=2` sweep-point with `--seed 42` — different
`process_seed`, different `run_id` — would map to the *same* `seed_42/` path, with
nowhere to store both and a stale-loop/deletion hazard (see PathPrefixCollision).
Likewise the empty-delta scenario uses `baseline-{scen_h8}` (the real hash of the
empty delta); `baseline`/`00000000` is a *display* convenience only, never a hash
folded into `run_id`.

| Level | Label (provenance) | Hash inputs (semantic, include-by-default) |
|---|---|---|
| model | model file stem | `ModelDynamicsDigest` + `OutputDigest` (the obs model lives in the obs sub-artifact, not here — so `--obs` stays passive). M2 interim: whole IR; M2.5 splits. + `ir_version` + `engine_version` |
| config | `chain_binomial-dt1` | backend, dt, t_start, t_end, output schedule, calendar mode, `allow_degenerate_rates` |
| parameters | a param-set label | resolved base param **values** (canonical) + `--table`/`--param-vec` content digests |
| scenario | scenario slug | resolved enable/disable/set **delta** (id-set + canonical patch); empty delta → label `baseline`, real hash |
| seed | `seed_42` (the base seed, readable) | the **resolved `process_seed`** — NOT the user `--seed` (segment carries `seed_h8`) |
| obs (sub) | stream name | full **`ObservationDigest`** (projections, likelihood families+params, corrections, aux, schedule, missing/window) + requested streams + plan + resolved `obs_seed` (layout/`--dates`/format are provenance) |

**Hash the resolved seed, not the base seed.** The trajectory is driven by
`process_seed = mix_cell_seed(base, point_idx, rep)` (`engine.rs:52-66`,
`util.rs:35-36`), and `obs_seed = process_seed ^ SEED_MIX_OBS` (`engine.rs:169`).
The *same* base seed maps to a *different* `process_seed` depending on grid shape
and cell position, so a lone `--param beta=2 --seed 42` (`process_seed = 42`) and
the `beta=2` point of `--sweep beta=1,2 --seed 42`
(`process_seed = 42 ^ 1·M_DRAW`) produce different trajectories. If the seed
level hashed the *base* seed they would share a full hash — a silent wrong
answer the `run.json` gate cannot catch (both compute the identical wrong hash).
The dir *label* stays readable (`seed_42`) but the segment is
`seed_42-{seed_h8}`; the hash is over the resolved `process_seed`. Red test:
lone-run vs sweep-point with the same base seed → **distinct paths** (not just
distinct `run_id`).

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
| fit | fit.toml stem | model (whole IR), data (resolved-obs digests), **the whole canonicalized fit.toml**, resolved priors, `engine_version` |
| stage | `NN-{name}` (`01-scout`, `02-posterior`) | the **whole `Stage` config struct** + the **resolved obs-block + flow indices** + `target_length` + **`deps: [upstream stage identities]`** |
| seed | — | base seed |

**Hash the whole fit.toml and the whole `Stage` struct — do not re-enumerate.**
Fit identity has many fit-level fields that change θ̂ and are easy to drop by
hand: `ic_free` (skips obs 1 from the loglik), `holdout`/`holdout_after`, the
*fit-level* `dt` and `backend` (`dt` lives on `[config]`, **not** on `Stage`),
`simplex_groups`, `synthetic`, fit-level scenario. A `Stage` likewise carries
`tempering`, `max_tree_depth`, `csmc_sweeps_per_nuts`, `dense_mass`, `use_nuts`,
`adapt`, `rho`, `burn_in`, `thin`, the gate/loglik-eval config — all
output-determining. Enumerating a subset is the same hash-a-recipe antipattern as
the gh#147 bug; hashing the canonicalized document/struct is the include-by-default
posture applied to the fit. The two genuinely *non*-fit-config inputs added on
top are the resolved obs-block name and flow-index set (the `--obs`/`--flow`
selection, which selects which series drives the likelihood and is *not* in the
toml).

Knobs that change a *saved sub-artifact's* bytes without changing θ̂ key their own
sub-artifact, not the stage leaf: `n_trajectories` (posterior-trajectory rows
written) → a `trajectories/` sub-artifact keyed on it; the dt-check → a
`dt_check/` sub-artifact keyed on `(enabled, n_halvings, strict_threshold,
dt_check_seed)`. So `--no-dt-check`, `--dt-check-halvings`, `--dt-check-strict`,
and `n_trajectories` stay provenance for the θ̂ leaf while remaining semantic for
their own sub-artifact — the same split as obs-under-trajectory.

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
    {filter_spec}-{spec_h8}/       # resolved-obs digests, fixed params, particles, backend, dt,
      seed_{n}/                    #   resolved obs-block + flow indices, time_format
        run.json  filtering.tsv
        paths/…  prequential/…     # --save-paths/--n-paths, --save-prequential → keyed sub-artifacts
```

Output-shaping flags that change a *sub-artifact's* bytes without changing the
loglik (`--n-paths`, `--save-paths`, `--save-prequential`, `--record-ancestry`)
key their **own sub-artifact**, the obs-under-trajectory pattern — they stay
provenance for the main `filtering.tsv` leaf.

### `profile` — grid nests by point/start

```
results/profiles/
  {model_stem}-{model_h8}/
    {profile_spec}-{spec_h8}/      # backend, dt, resolved-obs digests, resolved obs-block + flow indices,
                                   #   estimate/fixed, algorithm, cooling, sweep axes, iterations, starts
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
    { "name": "scenario","label": "baseline",        "hash": "0c1d…", "schema_version": 1 },  // real hash of empty delta
    { "name": "seed",   "label": "42",              "hash": "7788…", "schema_version": 1 }  // hash = resolved process_seed
  ],
  "deps": [],                          // [{run_id, kind, artifact, digest}] — lineage; fit stages list consumed upstream artifacts
  "status": "completed",               // running | completed | failed
  "artifacts": {                       // EXACT-SET over the leaf's OWN files only
    "traj.tsv": { "bytes": 40213, "mtime": "…", "digest": "…" }  // algo pinned by hash_version
  },
  "children": { "obs": ["<obs run_id>"] },  // declared child sub-artifacts (own run.json); validated recursively, NOT orphans
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
    pub deps: Vec<ArtifactRef>,          // { run_id, kind, artifact, digest } — the consumed artifact
    pub status: RunStatus,               // Running | Completed | Failed
    pub artifacts: BTreeMap<String, FileChecksum>,  // { bytes, mtime, digest } — exact set of OWN files
    pub children: BTreeMap<String, Vec<ContentHash>>,  // reserved child-subdir namespaces (obs/, paths/, …)
    pub inputs: serde_json::Value,       // resolved-input summary; provenance, not hashed
    pub provenance: Provenance,          // argv, label, timestamps, host, version, threads, source paths
}
```

## `CasStore` — lookup and the atomic commit protocol

```rust
pub enum Lookup {
    Hit(RunRecord),       // identity matches, Completed, integrity ok
    Miss,                 // nothing at the path
    Stale(StaleReason),   // SAME identity present but unusable → safe-clear + recompute
    Collision(RunRecord), // DIFFERENT full hash at this path → disambiguate, never touch incumbent
}

pub trait CasStore {
    fn lookup(&self, path: &Path, expected: &LeafIdentity) -> Lookup;
    fn commit(&self, path: &Path, record: RunRecord, artifacts: Artifacts) -> Result<(), CasError>;
}
```

**Lookup — tiered integrity** (so the full check never pressures an implementer
back toward the existence-only bug). A hit requires:

- *Identity gate, first:* `run.json` present and its recorded `run_id` + level
  hashes equal `expected`. **If `run.json` is present but the full hashes differ,
  this is `PathPrefixCollision`, not `Stale`** — a *different* artifact occupies a
  short-hash-colliding path; it must never be cleared or treated as a hit (see
  below).
- *Cheap gate, then:* `status == Completed`; `hash_version`/`schema_version`
  current; **the leaf's OWN files exactly match the manifest** (every listed file
  present at its recorded `bytes` + `mtime`, **and no unlisted files** — *except*
  the declared `children` subdirs, which are validated recursively and are not
  orphans). A crashed run's stray files (not listed, not a declared child) → `Stale`.
- *Full digest, on demand:* re-hash only on `--verify`, on first read after an
  `mtime` change, **and whenever the artifact is actually consumed** (`cat`, or a
  downstream run reading it as a dep). Re-digesting multi-GB fit chains on *every*
  `batch` re-run is the cliff that recreates `batch.rs:862`, so the hot path stops
  at the cheap gate — but be honest about what that buys: size+mtime is a
  *performance* optimization, not the integrity guarantee. mtime is coarse and
  `cp -p`/`rsync`/restore-from-backup preserve it, so a same-size-same-mtime
  tamper passes the cheap gate. The actual "never serve wrong bytes" guarantee is
  the input hash (collision resistance) **plus the digest check at consume time** —
  which is why anything that *reads* an artifact digests it, not just trusts the
  cheap gate.

A `Stale(reason)` (`Incomplete` | `Corrupt` | `OrphanFiles` | `SchemaDrift`)
means *this identity's* leaf is present but unusable → recompute in place (after
the safe-clear check below). A `Miss` means absent → compute. **A
`PathPrefixCollision` is neither** → the path is occupied by a different
identity; allocate a disambiguated path and never touch the incumbent. `--force`
skips the cheap/integrity gate but **not** the identity gate — it recomputes its
*own* identity, never overwrites a mismatched one.

**Path existence never implies identity — the rule that prevents data loss.**
Before any `remove_dir_all`, any "lost-race → Hit," or any overwrite, read the
incumbent `run.json` and compare full level hashes to `expected`:

- *full hashes match, status stale/corrupt* → safe to clear and recompute;
- *full hashes differ* → `PathPrefixCollision`: the 8-char segment aliased two
  distinct full hashes (possible for runs differing in a single level among a
  very large sibling set). Allocate a disambiguated path
  (`{label}-{hash8}~{hash16}/`) for the new run; the incumbent is untouched;
- *no `run.json`* → treat as this identity's incomplete run only if the path is
  unambiguously claimed by `expected` (e.g. a live `O_EXCL` lock for it),
  otherwise quarantine for manual repair rather than clearing.

This applies equally to `results/models/{source_h8}/` (a compile-cache short-hash
collision must disambiguate, not overwrite the other model's IR).

**Durable commit is new code.** Nothing in the tree fsyncs today; `rename`
without a barrier is *not* crash-atomic (a durable dir entry can point at an
inode whose data blocks never flushed). Both modes must implement this exact
ordering: write each artifact then `File::sync_all`; write `run.json` then
`sync_all`; `sync_all` the **containing dir fd**; `rename`; `sync_all` the
**parent of the destination** (the rename itself must be made durable). Steps 3
and 5 are the ones implementers forget.

**Commit — mode A: atomic stage-then-rename** (sim, obs, pfilter, survey,
profile-point — single-shot). Write all artifacts + `run.json`
(status `Completed`, checksums) into a unique staging dir
`results/.staging/{run_id}` *on the same filesystem as `results/`* (a
cross-mount staging dir makes the rename non-atomic — keep `.staging` under
`results/`), apply the fsync ordering above, then `rename(staging, final)`. A
reader never sees a half-written leaf. If `final` exists when the rename is
attempted, **run `lookup(final, expected)` — never assume existence means hit**:
a matching-identity `Completed` → discard staging, return Hit (lost a benign
race); a `PathPrefixCollision` → rename staging to the disambiguated path
instead; a same-identity `Stale` → safe-clear `final` and rename. Orphaned
`.staging/*` from a crash is swept on the next store open. This *replaces*
today's `traj.tsv`-existence check and non-atomic batch writes.

**Commit — mode B: streamed `Running → Completed`** (fit stages — long,
streaming, resumable; can't stage-then-rename because outputs must be visible
and resumable during the hours-long run). The leaf dir is **claimed
exclusively**: create `{stage_dir}/.lock` with `O_EXCL` (`create_new(true)`)
*before* any streaming; a second process gets `AlreadyExists` and fails fast
("fit in progress at PATH (pid …)") rather than interleaving bytes into the
shared chain files — without this, two concurrent identical fits corrupt one
artifact (the proposal previously mis-stated this as "loser recomputes," which
is Mode A's property). A `Running` `run.json` whose lock-holder PID is dead is a
reclaimable stale claim. **Recompute clears first — but only after the
identity check:** on a *same-identity* `Stale`, `remove_dir_all` the leaf and
recreate before streaming (production today has *no* such clean — only
`#[cfg(test)]` does — so a crashed longer run's orphan `trajectory_*.tsv`/`chain_*`
would otherwise survive into the new `Completed` artifact). On a
`PathPrefixCollision` (incumbent `run.json` has different full hashes), **never
clear** — disambiguate to a collision path; deleting it would destroy a valid
artifact that merely shares the 8-char prefix. Then: write `run.json` `Running`, stream chains (each new file
`sync_all`'d), and commit by writing `run.json.tmp` (`Completed`, full manifest)
→ `sync_all` → `rename` over `run.json` (the single-file rename is the commit
point) → `sync_all` the dir. A crash leaves `Running` → next lookup
`Stale(Incomplete)` → clear + recompute, or `--resume` reads the partial state
read-only and writes a *distinct* resumed artifact (see Determinism).

**Concurrency.** Mode A is race-safe by rename. Mode B is race-safe by the
`O_EXCL` lock (fail-fast, not silent corruption); it does not *deduplicate*
concurrent identical fits, but it does not corrupt. No TTLs, no background GC.

## `Resolve` — the resolution pipeline

`Resolve::resolve(ctx) -> Vec<RunInput>` turns raw CLI/TOML into a vector of
fully-resolved leaf inputs. Shared steps:

1. **Compile-or-cache IR.** `source_h = hash(.camdl bytes + camdlc_version +
   IR-affecting camdlc argv)`. Look up `results/models/{source_h8}/`, re-verify
   the recorded `reads.json` table digests; on miss run camdlc once and store
   (IR + `reads.json`). Load the canonical IR → `ModelDynamicsDigest` /
   `ObservationDigest` / `OutputDigest` (M2 interim: one whole-IR `ModelDigest`).
2. **Params.** Merge sources in precedence order (file < `--params` < `--param`
   < resolved `--param-vec`) into a canonical `BTreeMap<ParamId, FiniteF64>`;
   `--table PATH` → `DataDigest` (hash of *content*, not path). `NaN`/`Inf` →
   `ResolveError::NonFiniteParam`.
3. **Scenario.** Named scenario from the IR, or ad-hoc `--enable`/`--disable`/
   `--set` → a canonical `ResolvedScenario` delta (sorted id-sets + sorted
   patch). The empty delta hashes to its **real** `scen_h8`; `baseline` is the
   display label only (never a literal zero hash in `run_id`).
4. **Config.** backend, dt, t_start, t_end, output schedule resolved to concrete
   cadence/times, calendar mode, `allow_degenerate_rates`.
5. **Expand.** Sweep grids / design draws / `--seeds` → the Cartesian product →
   one `RunInput` per leaf. **Each draw row hashes its resolved per-draw param
   *values*** (its own `ResolvedParams` → its own params-level hash), not the
   design recipe. Hashing "the design spec, values recorded" is the same
   hash-the-recipe-not-the-result antipattern as the gh#147 model-hash bug, and
   it's unsafe: `lhs`/`sobol` have *no* design seed (fixed internal constants,
   `sampling.rs:204,223`), and `prior`/`uniform` draws depend on the simulate
   `--seed` (`main.rs:406-407`), so the recipe alone doesn't pin the row. The
   design spec (method, n, bounds) is recorded as provenance; draw-row
   distinctness lives in the params hash where it belongs.

**Data identity is the resolved-observation set, not the raw file digest.**
`--time-format {auto,numeric,date}` reinterprets the *same* data bytes (a column
parsed as numbers vs as ISO dates against the model `origin`/`time_unit`), so two
runs with identical files but different `--time-format` have different
observations and different logliks. For any data-consuming kind the hashed unit
is `(DataDigest, resolved time_format, origin, time_unit)` jointly — `auto`
resolves to its concrete choice before hashing — never the raw `DataDigest`
alone.

Every resolved value, never a raw path, unresolved preset, or generating recipe,
enters a hash.

## Addressing and the reader

- **`run_id` is the canonical address.** `show abc123` / `cat abc123` match a
  hex prefix of `run_id`; an ambiguous prefix errors with the candidates.
- **Generic tree walk.** Each kind declares its level names/depth via `Layout`,
  so the reader walks `results/` data-driven — no hard-coded 3-level depth
  (today's `browse.rs:690`), no dual-keying on `sim_hash` + path
  (`browse.rs:956`). Navigation reads `run.json`, never path segments.
- **Derived index, with run.json as truth — operationally.** A rebuildable
  `index.json` caches `(run_id → path, kind, label, status, created_at)` for fast
  `list` (replacing `manifest.json`). "Truth is run.json" must be *operational*,
  not aspirational: an index **miss falls back to a full tree walk** then repairs
  the index (so an out-of-band-added leaf is still found, not reported "no
  match"); an index entry whose `run.json` is absent is **dropped and the walk
  retried** (so an `rm -rf`'d leaf never resolves to a dead path). Index writes
  use the same atomic-rename + fsync as everything else (today's `manifest.json`
  is a non-atomic `fs::write` that concurrent `batch` processes clobber). A
  `camdl reindex` rebuilds from the run.json files. Tests must add a leaf out of
  band (→ `show` finds it) and remove one (→ `show` doesn't return a dead path).

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
// The model IR splits into three digests so an obs-only edit does not
// re-key (and resimulate) the latent trajectory — preserving "--obs is passive":
#[derive(RunInput)]
pub struct ModelDynamicsDigest {     // what determines the latent trajectory
    pub compartments, pub transitions, pub ode_equations,
    pub events, pub balance, pub initial_conditions,
    pub time_functions, pub tables_used_by_dynamics,
    pub interventions_affecting_dynamics,
    pub origin, pub time_unit,        // only if they drive dynamics/schedules
    pub ir_version: IrVersion, pub engine: EngineVersion,
}
#[derive(RunInput)]
pub struct ObservationDigest {       // the full measurement model (NOT just names)
    pub projections, pub likelihood_families, pub likelihood_params,
    pub diagnostic_corrections, pub aux_columns, pub schedules,
    pub missing_window_semantics, pub synthetic_obs_schema,
}
#[derive(RunInput)]
pub struct OutputDigest { pub trajectory_output_schedule, pub flow_quantities }

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
pub struct TrajectoryInput {          // latent dynamics only — NOT the obs model
    pub dynamics: ModelDynamicsDigest,
    pub output: OutputDigest,
    pub config: SimConfig,
    pub params: ResolvedParams,
    pub scenario: ResolvedScenario,
    pub seed: Seed,                   // hashed = resolved process_seed
    #[run_input(provenance)]
    pub display: RunProvenance,
}

// Other leaves, same pattern:
//   SyntheticObsInput { trajectory: ArtifactRef, obs: ObservationDigest, streams, plan, obs_seed }
//   FitStageInput     { fit: FitDigest, stage: StageConfig, deps: Vec<ArtifactRef>, target_length, seed }
//   PfilterEvalInput  { dynamics, obs: ObservationDigest, data: ResolvedObs, params, particles, config, seed }
//   SurveyInput, ProjectionInput (lineage), GroupInput (derived summaries only)
```

> **M2 may collapse the three model digests into one `ModelDigest = whole IR`**
> for safety (over-invalidates an obs-only edit, never under-invalidates), and
> **split into dynamics/observation/output in M2.5** to recover trajectory reuse.
> State which you shipped; do not leave it ambiguous. Either way the obs leaf
> hashes the full `ObservationDigest`, not just stream names + schedule.

`ArtifactRef` identifies the *consumed* artifact, not just its producing stage:
`{ run_id, kind, artifact: "mle_params.toml", digest }`. Hashing it folds in the
32-byte `digest` of the specific file consumed, so a change to a *sibling*
artifact (a diagnostic, a trace layout) under the same stage does not invalidate
the consumer, and a bug in artifact selection can't slip through. `Artifacts` is
a bundle (e.g. `{traj, event_log}` or `{paths, filtering, prequential}`).
`RunRecord` (above) is read by prefix
resolution and `show`/`cat`, never the path.

### `#[derive(RunInput)]` (crate `runid-derive`, `proc-macro = true`)

Generates `ContentAddressed` over all fields, include-by-default, honoring
`#[run_input(provenance)]` (skip) and `#[run_input(schema_version = N)]`
(per-type policy version). It emits `hash_into` following the canonical-hashing
rules above (type tag, schema version, length-prefixing, sorted maps, finite
floats, enum tags, `ArtifactRef`-by-identity) and a `content_hash` that finalizes
from `HASH_VERSION`. A field whose type is not `ContentAddressed` is a compile
error (the emitted `<FieldTy as ContentAddressed>::hash_into` fails to resolve) —
you cannot forget to make an input hashable.

**Bootstrapping — hand-write the `ir` tree first, extract the macro second.**
`ModelDigest.ir` is `ir::Model`, a *foreign* type with ~20 nested foreign types
(`Compartment`, `Transition`, `Expr`, `Table`, `OutputConfig`, …) pervaded by
`HashMap<String, f64>`, `HashMap<String, Expr>` (`rate_grad`,
`transition.rs:53`), and raw `f64`. You cannot `#[derive]` on types you don't
own, and hashing the IR via serde bytes is unsound (NaN → `null`; `HashMap`
order not guaranteed sorted). So M1 **hand-writes and tests `ContentAddressed`
for every `ir` type reachable from `Model`** (orphan rule allows it: local
trait, foreign type), applying the structural-IR-float rule and sorted-map rule
by hand. `Expr` is a `Box`-recursive tree and serde has hit its
recursion limit on deeply-nested IRs, so the hand-written `hash_into` mirrors
`eval_expr`'s depth strategy (or states its bound) rather than becoming a new
recursion cliff. This hand reference is also what the macro is validated against:
a golden test pins `macro output == hand impl` on a fixed value before the macro
is trusted. The macro **replaces** the hand-written `cas/typed.rs` canonical
hashing and the `hashing.rs` functions; there is never a second implementation.
`runid` depends only on `ir`; `cli` depends on `runid`. (There is no `observe`
crate; obs logic lives in `sim` + `cli`.)

## Grouping

The path tree handles the common groupings (seed ensembles, profile grids)
natively. `GroupInput` is reserved for **derived summaries** whose inputs are
member identities — `compare` (`FoldElpd` over fit-stage prequential refs),
ensemble quantiles — added one `GroupKind` at a time, never a summary DSL.
Per-chain convergence (R̂/ESS) is computed *inside* the fit-stage artifact, not
as a group (chains aren't separate artifacts).

**Storage dedups, analysis preserves multiplicity.** Content-addressing collapses
two draw rows with identical resolved params + seed to one leaf — correct for
storage, wrong for a posterior that needs both to count as two draws. So a
batch/draw result carries an **ordered index** (`draw_index → ArtifactRef`) where
indices 17 and 42 may point to the *same* `run_id`; downstream (posterior
predictive, ensemble weights) reads multiplicity from the index, not from the
distinct-leaf count. Make this index explicit before posterior-predictive
workflows consume the new CAS.

## What this replaces / fixes (current state, verified — gh#147)

- `cli/src/hashing.rs:31-35` — the `model_hash` allowlist omits `output`,
  `simulation`, `origin`, `origin_rata_die`, `time_unit` (live
  silent-wrong-trajectory: two models differing only in output cadence/`t_end`
  collide). (The separate gh#135 "hashes empty envelope" bug is *already* fixed;
  the remaining hole is specifically this omission.) Replaced by `ModelDigest`
  hashing the whole canonical IR (with presentation fields normalized out — see
  below).
- Hand-written `sim_hash`/`scen_hash`/`fit_stage_hash` (in `hashing.rs` and
  `cas/typed.rs`) collapse into derived per-level digests.
- Fit stale-reuse: `StartsFrom::Stage` serializes to the bare name
  (`fit/config_v2.rs:1360`); `fit_stage_hash` (`fit/provenance.rs:303`) keys on
  the name, not the produced θ̂ (recorded but not folded in, `fit/mod.rs:1452`).
  Fixed by `deps`. **Also:** `StartsFrom::Directory` (`config_v2.rs:1341`) starts
  from an external dir by *path* — `deps` must fold in that artifact's *identity*
  (its `run_id`), not the path string, or a regenerated upstream under-invalidates.
- Cache-hit divergence: single-run verifies the full hash (`main.rs:685`); batch
  trusts `traj.tsv`-existence (`batch.rs:862`) with non-atomic writes
  (`batch.rs:892`). Both become `CasStore::lookup` + a durable commit.
- `--allow-degenerate-rates` is a *process-global* `AtomicBool`
  (`eval_stats.rs:118-128`) set once per invocation. It is hashed at the config
  level, so `Resolve` must **forbid per-cell variation** of it within one batch
  (else the config-level hash claims a per-cell distinction the global can't
  honor).
- Stale comment at `cli/src/if2.rs:349` references a removed `camdl fit if2`
  subcommand — delete during M3 when `if2` desugars to a one-stage fit.
- The dt-check flags `--no-dt-check`/`--dt-check-halvings`/`--dt-check-strict`
  (`fit/mod.rs:1042-1047`; the dt-check has its own derived
  `dt_check_seed = seed + 0xd7c4ec5eed`) change *artifact bytes* without changing
  θ̂. Make the dt-check its **own sub-artifact** keyed on
  `(enabled, n_halvings, strict_threshold, dt_check_seed)`, so all three flags
  stay provenance for the θ̂ leaf and semantic only for their sub-artifact. (All
  three must appear in the flag inventory — omitting two is the same foot-gun.)
- `output.format`/`time_semantics` live in `ir::Model` but never affect computed
  values; a **normalization pass strips them from the hashed `CanonicalIr`** so
  "the whole IR" means the whole *value-determining* IR and `--format` is
  genuinely inert (otherwise it busts the model cache despite being provenance).

## Adding a new CAS-backed subcommand

The system is only as sound as its least careful extension. A new artifact-producing
subcommand follows this recipe; the steps exist to make the foot-guns this design
already hit *unrepresentable* for the next author.

1. **Enumerate the complete input set — default everything in.** List every CLI
   flag and every config field the command reads. Each is an input *until proven
   inert*. The bar for "inert" is: changing it changes the bytes of no artifact in
   the bundle. When unsure, include it — over-invalidation recomputes (cheap,
   visible); under-invalidation serves a wrong answer (silent, corrupting). The
   completeness audit (a per-command table of flag → semantic/provenance) is part
   of the PR, not an afterthought.
2. **Define the leaf input type.** A `XInput` struct, `#[derive(RunInput)]`,
   composing the shared per-level digests (`ModelDigest`, `SimConfig`,
   `ResolvedParams`, `ResolvedScenario`) wherever they apply, plus the
   command-specific fields. Provenance fields carry `#[run_input(provenance)]`.
3. **Resolve, never smuggle a recipe or a name.** Implement `Resolve` so every
   field is a resolved *value* or *content digest* before hashing:
   - a path is read to a `DataDigest` (hash of content), never hashed as a path;
   - a preset / named scenario resolves to its values;
   - a *generator* resolves to its *result* — hash the drawn param vector, not the
     `--draws` design; hash the resolved `process_seed`/`obs_seed`, not `--seed`;
   - **if your command has its own stochastic expansion** (per-draw, per-replicate,
     per-stream — not the sweep grid), define a deterministic
     `unit_seed(base, unit_index) → u64` and hash the *per-unit resolved seed* in
     each leaf, exactly as the grid resolves `process_seed = mix_cell_seed(base,
     point_idx, rep)` (`engine.rs:52-66`). Do **not** store the base `--seed` and
     mix at runtime inside `Run` — that reproduces the base-seed collision;
   - a non-finite resolved float is a `ResolveError`, surfaced before any hash.
4. **Classify each flag.** Semantic (hashed) iff it changes artifact bytes. Three
   classes to keep straight:
   - *selects what is computed* (a test statistic, an estimand, `--obs`/`--flow`,
     a summary function) — **semantic**, even though it "just picks an output";
   - *toggles a diagnostic block* — semantic, but better split into its own
     sub-artifact (the obs-under-trajectory pattern) so the flag stays provenance
     for the main artifact;
   - *selects what is displayed* of already-computed values (`--dates`,
     `--format`, wide-vs-dir, a `cat`-time column filter) — **provenance**.
   The test: would two values of the flag, each run to completion, produce a
   different artifact you'd want to cache separately? If yes, semantic; when
   unsure, include it. Presentation values that live in the hashed IR are
   normalized out of the `CanonicalIr`; a presentation field that is your
   *command's own* (not in the IR) goes in the leaf struct with
   `#[run_input(provenance)]` (step 2) and is applied at `cat` time.
5. **Lineage by identity — and know what's an artifact vs a file.** A consumed
   *camdl artifact* (has a `run_id` in the store) is an `ArtifactRef`; a consumed
   *external file* (raw data, a table) is a `DataDigest` of its content (step 3) —
   observed data is a `DataDigest`, not an `ArtifactRef`. An `ArtifactRef` records
   `{run_id, kind, path}` for display but **only the `run_id` (32 bytes) is
   hashed** — the path is recorded-not-hashed, so a regenerated upstream
   invalidates correctly. Multi-hop chains (event-log → line-list → projection)
   carry one `ArtifactRef` per hop.
6. **Register the kind and declare the `Layout`.** Add a variant to
   `ArtifactKind` and register its `Layout` (the kind dir + the factored levels —
   each a disjoint input slice, union = the complete set — + the leaf) so the
   data-driven reader walks it; an unregistered kind is invisible to
   `list`/`show`. Reuse the shared levels so grouping keeps working.
7. **`Run` writes only through `CasStore`.** Never write artifact files directly —
   the store provides the atomic/durable commit, the exact-set checksum manifest,
   and the `Running → Completed` semantics. A command that writes its own files
   loses all three. The sneaky path back in: **reusing an existing engine that
   writes its own files** (e.g. the simulate backend) — capture its outputs into
   the returned `Artifacts` bundle rather than letting it touch disk.
8. **Gate it with the standard tests.** Per output-determining flag: two inputs
   differing only in that flag → *distinct* `run_id`. Per provenance flag: differs
   → *same* `run_id`. Plus a golden-hash pin. These mirror the gh#147 suite and are
   the command's definition of done.

The foot-guns this recipe forecloses, each a real finding from review: hashing a
recipe instead of its result; hashing a base seed instead of the resolved one; a
diagnostic-toggling flag mislabeled provenance; a path-valued lineage dependency;
a presentation field left inside the hashed IR; and writing artifact files outside
the store.

## Existing scaffolding to extract and harden

This refactor migrates and hardens working code; it does not start from zero.
What already exists and what changes:

| Already exists | Becomes |
|---|---|
| `cas/typed.rs`: `ContentHash` (SHA-256), `CasInputs` trait (`content_hash`/`cas_path`/`run_kind`/`to_run`), `hash_canonical`/`compose_with_replicate`, `ReplicateSet` | extracted into `runid`; `CasInputs` splits into `ContentAddressed` + `Layout` + `Run`; **`hash_canonical` (canonical-string) is replaced by the structural `CanonicalHasher`** |
| `cas/sim_inputs.rs::SimulateInputs`, `cas/fit_inputs.rs::{FitInputs,StageInputs}` | reworked into the per-level digest types (`ModelDigest`/`SimConfig`/…); they already produce the readable nested path |
| `run_meta.rs`: `Run`, `RunStatus{Running,Completed}`, `RunKind`, `CacheStatus{Hit,Stale,Miss}`, `check_cache`, atomic `run.json.tmp → rename` writer | extracted into `runid`; `RunStatus` gains `Failed`; `check_cache` becomes `lookup` (tiered integrity + exact-set manifest + fsync); the writer gains the full fsync ordering |

The reuse decisions an implementer must make up front: keep `ReplicateSet`
(camdl-viewer's `cas.py` consumes it) — rename only if the contract diff covers
it; the "second implementation to avoid" is now `cas/typed.rs::hash_canonical`,
not just `hashing.rs`.

## Invariants the implementer MUST preserve

1. Identity is the factored hash of the *resolved* input. No raw path, `Option`
   on a required semantic field, unresolved name/preset, or generating recipe
   enters any level — `--param-vec`/`--table`/`--regime`/`--rw-sd auto`/
   `--time-format auto`/`--draws`/`[design.*]` resolve to values/digests, and
   **the seed level hashes the resolved `process_seed`/`obs_seed`, never the
   base `--seed`**; draw rows hash their resolved param values, not the design.
2. Include-by-default per level; provenance is the only exclusion, annotated.
3. Paths are readable nested `{label}-{hash8}`; **every semantic level carries
   its hash in the segment — no level is label-only** (the seed segment is
   `seed_{base}-{seed_h8}`; the empty scenario is `baseline-{scen_h8}` with its
   real hash, not a literal zero). Labels are provenance, hashes are identity;
   navigation + display read `run.json`, never the path shape.
4. A cache hit requires `run.json` present, `status == Completed`, hashes match,
   the artifact set **exactly** matches the manifest — no missing and no orphan,
   *except declared `children` subdirs* (validated recursively) — at recorded
   size+mtime, schema/hash versions current; the content digest is verified at
   consume time. Commit is durable (mode A) or streamed `Running → Completed`
   (mode B). **Adding a child sub-artifact (e.g. `--obs`) never makes its parent
   leaf stale.**
5. Forward-sim/obs determinism stays gate-green; inference is content-addressed
   only with the watchdog disabled and resume treated as a distinct artifact
   (which means resume is **rewritten** to read the prior artifact read-only and
   write a new leaf — not appended in place).
6. **Durability is explicit code.** Commit fsyncs each artifact + `run.json` +
   the containing dir before `rename`, and fsyncs the destination's parent after.
   Staging is on the same filesystem as `results/`. Mode B claims its leaf via
   `O_EXCL` before streaming. Lineage/`deps` fold in the **consumed artifact's
   identity** (`run_id` + artifact digest), never paths.
7. **Path existence never implies identity.** Before any hit, clear, or
   overwrite, read the incumbent `run.json` and compare full level hashes: match
   → proceed; differ → `PathPrefixCollision`, allocate a disambiguated path and
   **never delete the incumbent**. No `remove_dir_all` without a verified
   same-identity match. This applies to `results/models/` too.

## What NOT to do

- No flat hash-only store — keep the readable nested tree.
- No 16-char path migration — 8-char segments + full hashes in `run.json`.
- No second hashing implementation; the structural `CanonicalHasher` replaces
  *both* `hashing.rs` and `cas/typed.rs::hash_canonical`.
- No `rename` without the fsync ordering — it is not crash-atomic, and nothing
  in the tree fsyncs today.
- No in-place resume — resume reads the prior leaf read-only and writes a new one.
- No existence-only cache check, and no skipping the exact-set manifest to save
  time — use the tiered check (size+mtime cheap, digest at consume time).
- No deleting or overwriting a path whose `run.json` full hashes don't match the
  expected identity — that is a `PathPrefixCollision`, disambiguate it.
- No treating a nested sub-artifact dir as an orphan — declare it in `children`.
- No grouping engine / summary DSL — reserve `GroupInput`, add kinds singly.
- No bespoke "one-off fit" shape — `camdl if2` is a one-stage fit.
- Do not content-address fits before the M3 engine fixes land.
- Do not change `HASHER`/`HASH_VERSION` casually — it invalidates the store.

## Build order — one cleanup, four milestones

This lands as a single coordinated cleanup (alpha software: no interim stopgap
PR — gh#147 tracks the correctness hole until merge). The run-spec
CAS-default-output / `--stdout` work lands *after*, separately.

**M1 — the `runid` crate (extract + harden).** Extract `cas/typed.rs` +
`run_meta.rs` into `runid`; replace `hash_canonical` with the structural
`CanonicalHasher` (`ContentHash`, `run_id`); add `RunRecord`/`run.json` (with
`Failed` + exact-set checksum manifest); harden `check_cache → lookup` (tiered
integrity) and the writer (full fsync ordering); `CasStore` both commit modes.
**Hand-write and test `ContentAddressed` for the digest types and the entire
`ir` type tree reachable from `Model` first; extract `runid-derive` second**,
gated by a `macro-output == hand-impl` golden equivalence test. Also specify the
leaf shapes left as "same pattern" comments: `FitDigest`, `StageConfig`,
`PfilterEvalInput`, `SurveyInput`, `ProjectionInput`. *Gate:* property tests
(canonical invariance, `±0.0` IR distinctness, float canon) + golden-hash
regression + macro equivalence, all green with no CLI wiring.

**M2 — forward sim + obs.** Wire `simulate` and `batch` through
`Resolve → Layout → CasStore`; obs sub-artifacts; the compile cache. Delete
**only** `sim_hash`/`scen_hash` from `hashing.rs` (the sim path);
`model_hash`/`canonical_params` survive until M3 (fit/survey/profile still call
them); relocate utilities `slug`/`path_stem_slug`/`sha256_hex`/`file_hash` to a
util module (do not delete; `external-harness/src/hashing.rs` is a *different*
file, out of scope). Regenerate golden files. *Gate:* gh#147 red→green for the
trajectory key, **the obs-sub-artifact key, and the compile-cache key**; the
`process_seed` lone-vs-sweep-point collision test; concurrent-commit rename test.

**M3 — engine purity + inference.** Replace the wall-clock watchdog with an
*iteration-based* bound for CAS runs (the `WALLCLOCK_ENV=0 → None` disable
already exists; the iteration bound is the new safety). **Rewrite resume** to
read the prior artifact read-only and write a new leaf. Wire fit stages
(`NN-stage`, `deps` by upstream identity), `pfilter` (`pfilters/`), `survey`,
`profile`; `if2` desugars to a one-stage fit; split the dt-check into its own
sub-artifact; delete the stale `cli/src/if2.rs:349` comment. *Gate:*
fit-reproducibility pin (same inputs, watchdog off → identical θ̂); **an
iteration-bound-still-catches-a-wedged-filter pin** (the traded safety property);
`--parallel 1` vs `8` → identical θ̂; resume → distinct `run_id` **and base run
byte-identical before/after**; **two concurrent identical fits → the second fails
fast on the `O_EXCL` claim** (no interleaved chains).

**M4 — addressing + reader.** Generic tree walker (data-driven depth via
`Layout`, replacing the hard-coded 3-level walk), `run_id` prefix resolution with
full-walk fallback on index-miss, atomic+fsync `index.json` (replaces
`manifest.json`), `camdl reindex`. Emit the path-shape contract diff for
camdl-book and camdl-viewer. *Gate:* addressing round-trip + index-staleness
tests (out-of-band add/remove).

Deferred: additional `GroupKind`s, a compiled-IR GC, run-spec CAS-default output.

## Testing plan

*M1 — hasher (no CLI wiring):*
- **Canonical invariance (property).** Permuting `HashMap`/insertion order of any
  map/set field → identical hash. Pins the sorted-iteration rule.
- **Float policy.** Resolved-input `-0.0` and `+0.0` → same hash; resolved `NaN`
  → `ResolveError` (no hash). **IR-float `Const(0.0)` vs `Const(-0.0)` →
  *distinct* model hash** (pins agreement with `ConstExpr::PartialEq`).
- **Macro equivalence.** `#[derive(RunInput)]` output == the hand-written
  `ContentAddressed` impl on a fixed value.
- **Golden-hash regression.** A fixed `RunInput` → a committed 64-hex; only a
  `HASH_VERSION` bump may change it.

*M2 — forward sim + obs:*
- **Correctness (gh#147, red→green).** Model differing only in
  `output`/`t_end`/`origin`/`time_unit` → distinct key; `--allow-degenerate-rates`
  on a collapse-firing model → distinct key; `--dates` → same key; partial
  `traj.tsv` without a Completed `run.json` → not a hit (batch + single agree).
- **Resolved-seed collision.** Lone `--param beta=2 --seed 42` vs the `beta=2`
  point of `--sweep beta=1,2 --seed 42` → **distinct paths** (assert the
  `seed_*-{h8}` dirs differ, not just the `run_id`).
- **Child namespace vs exact-set.** Running `simulate` then `simulate --obs` on
  the same trajectory → the trajectory leaf is **still a Hit** (the `obs/` child
  is not an orphan), and the obs sub-artifact is its own leaf.
- **Path-prefix collision (no data loss).** Force two distinct full hashes onto
  one 8-char segment → the second run gets a disambiguated path, the incumbent's
  bytes are untouched, and neither lookup nor `--force` deletes the other.
- **Keyed sub-artifacts.** Obs-sub-artifact key and compile-cache key each get a
  red→green test, not only the trajectory key.
- **Compile cache.** A changed `read()` table file → miss (recompile); a changed
  IR-affecting camdlc flag → distinct `source_h`.
- **Parallel determinism.** `simulate`, `batch`, and `simulate --obs` at
  `--parallel 1` vs `8` → identical `run_id` and identical bytes (pins that thread
  count is provenance for the forward path too, not only fits).
- **Concurrency / crash.** Two threads commit the same sim → one wins the rename,
  both observe `Completed`, artifacts intact. Crash-injection between each fsync
  step → no `Completed` record ever points at unflushed bytes.
- **Stale / orphan.** Corrupt an artifact / truncate `run.json` / flip status to
  `running` / leave an *unlisted* file in the leaf → lookup returns `Stale` →
  recompute.

*M3 — inference:*
- **Determinism.** `gate_trajectory_baseline.rs` stays green; fit-reproducibility
  pin (same inputs, watchdog off → identical θ̂); `--parallel 1` vs `8` →
  identical θ̂; `from_mle` with a changed upstream θ̂ → miss.
- **Fit completeness (under-invalidation regressions).** Two fits differing only
  in `ic_free` / `holdout` / fit-level `dt` / `--obs` / `--flow` → distinct fit
  `run_id`; changing `n_trajectories` or a dt-check flag → *same* θ̂ leaf,
  *distinct* sub-artifact.
- **Traded safety.** An iteration-based bound still aborts a genuinely wedged
  filter (the property the wall-clock watchdog used to provide).
- **Resume.** Resume → different `run_id` **and** the base run's bytes are
  identical before and after (catches the in-place-mutation bug).
- **Concurrent fit.** Two identical fits → the second fails fast on the `O_EXCL`
  claim, no interleaved/corrupt chains.

*M4 — reader:*
- **Addressing round-trip + staleness.** Commit N runs, resolve each by `run_id`
  prefix (ambiguous → error with candidates); add a leaf out of band → `show`
  finds it; remove a leaf out of band → `show` returns no dead path.

## Error taxonomy

- `ResolveError` — `CompileFailed`, `ParamParse`, `NonFiniteParam`,
  `UnknownScenario`, `FileNotFound`, `BadDesignSpec`.
- `RunError` — wraps engine/backend errors with the leaf identity attached.
- `CasError` — `Io`, `ChecksumMismatch`, `OrphanFiles`, `SchemaDrift`,
  `FitInProgress` (the Mode B `O_EXCL` claim is held by a live process). A lost
  Mode A rename race is handled internally as a Hit, not surfaced.

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
| `lineage realize/tree/sojourn/cohort` | `Projection` (a 1–2-hop chain) | enumerate inputs per subcommand (below); each hop's upstream is an `ArtifactRef`. |
| `eval` | `Eval` | pure fn of (model, params, `--expr`, grid). |
| `compare` | `Group(FoldElpd)` over fit prequential refs | `--baseline` = provenance. |

Non-CAS: `data split`, `label`/`list`/`show`/`cat`/status/metadata commands,
`compile`/`check`/`inspect` (delegate to camdlc) — these consume or display
artifacts rather than produce new ones.

**Lineage projection inputs (exhaustive, per subcommand).** The chain is
`simulate --event-log` → `realize` → line-list → `tree`/`cohort`/`sojourn`. Each
hop's upstream is its own `ArtifactRef`, and *every* output-determining flag is
hashed — the summary "scheme/window = semantic" was incomplete:
- `realize` — `{event-log ArtifactRef, --identity-seed}`.
- `tree` — `{line-list ArtifactRef, --sample-seed, sampling scheme, window}`;
  Newick output format is provenance.
- `cohort` — `{line-list ArtifactRef, --event, window, --align-zero}`. `--event`
  (`args/mod.rs:2062`) selects which transition to bin and `--align-zero`
  (`args/mod.rs:2068`) sets the binning origin — both change the rows, both must
  be hashed.
- `sojourn` — `{line-list ArtifactRef, compartment, window}`.

Semantic-vs-provenance of the load-bearing flags: **semantic** — model, params
(`--param`/`--params`/`--param-vec`/`--draws`), `--table` content, scenario,
`--backend`, `--dt`, `--seed` (hashed as the resolved `process_seed`), output
schedule + horizon, obs streams+schedule, **`--obs`/`--flow`** (selects which
series drives the likelihood), **`--allow-degenerate-rates`**,
`--identity-seed`/`--sample-seed`, and resolved
`--regime`/`--rw-sd`/`--init`/`--time-format` (the last hashed *jointly* with the
data digest). **Provenance for the main leaf, but each keys its own sub-artifact**
— `--no-dt-check`/`--dt-check-halvings`/`--dt-check-strict`, `n_trajectories`,
`--save-paths`/`--n-paths`/`--save-prequential`/`--record-ancestry`. **Pure
provenance** — `--parallel`, `--force`, `--dry-run`, `-o`/`--output`, `--stdout`,
`--label`, `geo`, `--resume` flag itself (the resulting `target_length` is
semantic), `--dates`, obs wide-vs-dir, `--format`/`--tsv`, `--suppress-warnings`,
`--progress`, `--verbosity`.
