# CAS path-shape contract (for downstream consumers)

Status: current as of the content-addressed run-identity refactor (gh#147).
Audience: tools that read camdl's `results/` tree — notably camdl-book
(`scenarios.qmd`) and camdl-viewer (`cas.py`).

## The one rule

**Resolve runs by reading `run.json`, never by parsing path segments.**

Every leaf directory contains a `run.json` (`runid::RunRecord`) with the
authoritative fields:

- `run_id` — the 64-hex canonical address of the run.
- `kind` — `"sim" | "fit_stage" | "pfilter" | "survey" | "profile_point"`.
- `levels` — the factored identity, in order: each `{ name, label, hash,
  schema_version }`.
- `status`, `provenance.created_at`, `provenance.label`, `inputs`, `artifacts`.

Path segments mirror `levels` for human navigation, but the **label** in a
segment is provenance only: renaming it produces a new directory (a harmless
cache miss), never a wrong answer. Identity lives in the `hash8` suffix and,
authoritatively, in `run.json`. Do not infer kind, parameters, seed, or
lineage from the path — read them from `run.json`.

## Path shape

```
results/<kind_dir>/<seg>/<seg>/…/run.json
```

- `<kind_dir>`: `sims` | `fits` | `pfilters` | `surveys` | `profiles`.
- `<seg>` = `<label>-<hash8>`, one segment per level in `levels` order.
- **Collision suffix:** if two leaves would share a directory name, the
  later one's final segment gets a `~<disambiguator>` suffix
  (`<label>-<hash8>~<hash16>`). Enumerate sibling directories and read each
  `run.json` rather than reconstructing an expected name — a `~`-suffixed
  sibling is a normal leaf. Both colliding runs have distinct full `run_id`s.

### Levels per kind (the `levels` array; path segments mirror it)

| kind            | `kind_dir`  | levels (in order)                          |
| --------------- | ----------- | ------------------------------------------ |
| `sim`           | `sims`      | model · config · params · scenario · seed  |
| `fit_stage`     | `fits`      | fit · stage · seed                         |
| `pfilter`       | `pfilters`  | model · config · params · seed             |
| `survey`        | `surveys`   | model · config · box · seed                |
| `profile_point` | `profiles`  | profile · point · stage · seed · start     |

A **fit** is the `fits/<stem>-<hash8>/` segment: it has no `run.json` of its
own — it carries a `fit.meta.json` sidecar (fit-wide provenance: label,
model/data hashes, estimated/fixed, resolved priors) and one `fit_stage`
leaf per (cell × stage) underneath. Read the fit-level view by combining the
sidecar with its stage-leaf `run.json`s.

**Sub-artifacts (no `run.json`).** A trajectory leaf may declare an observation
ensemble as a child. It is **not** a `RunRecord`: it lives at
`obs/<obs_hash8>-<obs_seed>/` under the leaf and holds one `<stream>.tsv` per
observation stream plus an `obs.json` provenance file (`obs_hash`, `obs_seed`,
`process_seed`, `streams`). Reach it through the parent leaf's `children`, not
by walking for `run.json`. A `projection` kind is reserved (the lineage
projections are pure functions over a realized line list and write nothing to
the store today).

## What changed from the pre-refactor layout (the migration diff)

- **All existing `results/` are cache-misses.** Level hash *contents*
  changed (e.g. the model hash now covers the whole IR, including
  output/origin/time-unit). There is no migration: clear `results/` and
  re-run. Old paths/records are not readable by the new tools.
- **Fit stage segments gained an `NN-` ordinal prefix** (`01-scout-<h8>`,
  not `scout-<h8>`) so execution order sorts topologically.
- **`pfilter` moved to its own `pfilters/` kind** (was nested elsewhere).
- **`camdl if2` is removed.** A single-method IF2 run is now a one-stage
  fit: `fits/<stem>-<h8>/01-fit-<h8>/…`, run via `camdl fit run` with a
  `[stages.X] algorithm = "if2"` block.
- **No fit-wide `run.json`** at the `fits/<stem>-<h8>/` level — read the
  `fit.meta.json` sidecar + the stage leaves (see above).

## Derived index (`results/index.json`)

A rebuildable `index.json` at the store root caches `run_id → (path, kind,
label, status, created_at)` for fast lookup. It is a **cache, not truth** —
`run.json` is authoritative:

- An index miss falls back to a full tree walk (so an out-of-band-added leaf
  is still found), then repairs the index.
- An entry whose `run.json` is gone is dropped (never resolves to a dead
  path).
- Consumers may read `index.json` for fast enumeration, but must treat a
  missing/stale entry as "walk the tree", and must verify any entry against
  the live `run.json`. `camdl reindex` rebuilds it from the `run.json` files.
