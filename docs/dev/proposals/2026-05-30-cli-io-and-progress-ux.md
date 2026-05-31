# CLI UX rev 3: CAS-by-default I/O, a nested progress model, and finishing the resolver migration

**Date:** 2026-05-30
**Status:** Draft — design review
**Class:** doc-vs-code (output layout drift) + code-vs-code (flag-semantics
inconsistency across sibling subcommands) + new-feature (progress model).
**Depends on / sequences after:**
[`2026-05-25-cli-init-and-params-ux.md`](2026-05-25-cli-init-and-params-ux.md)
(the `--fixed`/`--init`/`ParameterResolver` rev-2; steps 4–5 shipped, 6–12
deferred). This proposal does **not** restart that work — it lands it, and
makes the surface it leaves behind consistent with the I/O and progress
changes here.
**Touches the run-spec:** yes — [`docs/camdl-run-spec.md`](../../camdl-run-spec.md)
is updated in lockstep (§2 layout, §3 types, §4 single-run, §5 batch).
**Downstream consumer:** [`camdl-viewer`](../../../../camdl-viewer) — a
Streamlit run-browser whose `cas.py` reads `run.json`. The CAS schema here is
its contract.

---

## TL;DR

Three changes, one coherent surface:

1. **CAS-by-default I/O.** Every run registers in the content-addressable
   store and announces itself (`✓ wrote run <hash> · camdl cat <hash>`).
   `--stdout` is the explicit opt-out for piping. This kills the
   160 MB-to-terminal flood (the bug that opened this thread), unifies the
   "where did my output go?" story across `simulate`/`pfilter`/`if2`/`fit`,
   and makes the viewer trivial (it already reads the CAS).

2. **A nested progress model.** Progress is a *tree* whose shape differs per
   command (`seeds → timesteps`, `chains → iterations → filter sweep`).
   Replace the flat, `n_runs > 1`-gated rendering with one `ProgressReporter`
   that loops report into; rendering is a pure projection of the tree. Single
   long runs and the silent ~20 s compile step finally show feedback. Slim
   the mode enum from `auto/pretty/plain/none` to `auto/none`.

3. **Finish the resolver migration + fix the two orthogonal flag overloads.**
   Land rev-2's `ParameterResolver` across `if2`/`profile`/`survey`/`fit run`
   (removing `--params` there), and additionally fix the overloads rev-2 does
   *not* touch: `--obs` (output-path on `simulate` vs input-block-name on
   `pfilter`/`if2`) and `survey --data` (inline params, not a data file).

The connective tissue: `run.json`'s `status` field (`running` →
`completed{wall_time}`, already in `run_meta.rs`) is the seam that unifies
all three — a run writes its record at *start* with `status=running`, so a
live run is discoverable in `camdl list` and the viewer *while it runs*; the
progress tree's outer node and the CAS record are the same object.

---

## What is already true (verified, not assumed)

This proposal is deliberately small because most of the machinery exists. All
of the following was confirmed by reading source or running the binary
(`camdl camdl 0.1.0+7974ea5`); items I could not run-confirm are tagged.

**Output today is stdout-by-default, CAS opt-in.**
- `rust/crates/cli/src/args/mod.rs` — `--output` help reads "default: stdout"
  at lines 497 (simulate), 1237 (pfilter), 1349 (if2), 1546 (profile), 1862,
  1979, 2046. `--cas` is an opt-in flag.
- *Observed:* `simulate kano.camdl --backend chain_binomial --scenario
  baseline` with no `-o` writes a **160 MB / 16,457-column** trajectory to
  stdout after a **~45 s** silent run (`wc -c` = 168,125,850; byte-verified),
  preceded by a **~21 s / 8.4 GB-RSS** silent `camdlc` compile producing a
  **1.81 GB** IR (`ir_bytes=1814677164`).

**The unified run record already shipped** —
`rust/crates/cli/src/run_meta.rs` (1504 lines):
```rust
pub struct Run {
    pub hash: String,
    pub version: String,
    pub created_at: String,
    pub argv: Vec<String>,
    pub status: RunStatus,            // running | completed { wall_time_seconds }
    pub label: Option<String>,
    pub kind: RunKind,                // #[serde(tag="kind")] → "kind":"simulate" etc.
}
pub enum RunKind { Simulate(SimulateMeta), Fit(FitMeta), FitStage(FitStageMeta), Batch(..) }
// SimulateMeta { model, model_hash, scenario, sim_hash, scen_hash, seed,
//                backend, dt, sweep_point, from_fit_hash, parameters_provenance }
// FitMeta { model, fit_toml_path/hash, data_hashes, estimated, fixed,
//           stages_declared, ic_free, resolved_priors, parameters_provenance }
```
So the `Run`/`RunKind`/`RunStatus` design from
[`2026-04-19-unified-output-tree.md`](2026-04-19-unified-output-tree.md) is
**landed**, including `parameters_provenance` (the resolver's output) and the
`status` lifecycle. *To confirm before implementation:* one real `run.json`
on disk matches this exactly (my `--cas` probe failed on an unrelated missing
`N0` default; the schema above is from source, not from an artifact).

**The resolver is half-migrated** (from
[`2026-05-25-cli-ux-impl-questions.md`](../notes/2026-05-25-cli-ux-impl-questions.md)):
`params_resolver.rs` (660 lines, 16 tests) shipped; `eval` + `pfilter`
migrated; `if2`/`profile`/`survey`/`fit run` deferred (they touch inference
math + the book renders in lockstep). *Observed:* `if2 … --params p.toml` →
exit 1, "`--params` is no longer accepted on `camdl if2`."

**The viewer is real and CAS-shaped** —
`camdl-viewer/camdl_viewer/cas.py` walks
`{root}/sims/{sim8}/{slug}-{scen8}/seed_{n}/{traj.tsv,run.json}`, parses
`run.json` (handling both an *old-flat* and the *new-tagged* `RunKind`
format), groups sims into scenarios, and overlays ensembles with PI ribbons.
README launches it with `--runs /path/to/output/sims`.

---

## Problem 1 — Output layout: three names for one tree (doc-vs-code)

`results/` vs `output/` vs `runs/` is genuinely ambiguous across the codebase
and its docs:

| source | sim root it names |
|---|---|
| `docs/camdl-run-spec.md` §2.4 | `results/simulate/…` + `manifest.json` |
| `docs/camdl-run-spec.md` §4.4 (same doc) | `results/sims/…` |
| `2026-04-19-unified-output-tree.md` (proposed rename, never fully landed) | `output/sims/…` |
| **camdl code** (`results/sims` literals; grep-verified) | **`results/sims/…`** |
| camdl-viewer README / `--runs` | `output/sims/…` |

The run-spec **contradicts itself** (`simulate` in §2.4, `sims` in §4.4), and
the viewer demo expects `output/` while camdl writes `results/`.

**Decision (D1 — resolved): `results/`.** The shipping code already defaults
to `./results` (`args/mod.rs:529`, "Root directory for --cas output
[default: ./results]"). The two dissenters are (a) run-spec §2.4's
`results/simulate/` typo-variant and (b) the camdl-viewer scaffold, which
passes `--output-dir output` to *override* the default — and that scaffold is
one commit deep, agent-generated, and simply got it wrong. We do not rename
shipping code to match a one-commit scaffold; we fix the doc and the scaffold.

Actions: run-spec §2.4 → `results/sims/` everywhere (drop `results/simulate/`
and reconcile with §4.4); the camdl-viewer Makefile/`batch.toml` drop the
`--output-dir output` override (or set it to `results`) when we PR the viewer.
One name — `results/` — enforced by the single `default_output_dir()`.

*To verify:* whether `camdl batch run` actually emits the `manifest.json` the
viewer's Make target depends on, or whether that target is itself part of the
scaffold's drift.

---

## Problem 2 — CAS-by-default

### Behavior

Every run registers in the CAS and prints a one-line discoverability banner
to **stderr** (so stdout stays clean for the rare `--stdout` pipe):

```
$ camdl simulate kano.camdl --backend chain_binomial --scenario baseline
compiling kano_lga_seirv.camdl … done · 4,620 compartments · 1.8 GB IR · 21s
simulate · chain_binomial  ████████████  t 4388/4388  done · 45s
✓ run a1d91886  ·  160 MB traj  ·  camdl cat a1d91886 | less
```

- `--stdout` — opt out; write the primary artifact to stdout, suppress the
  CAS write and the banner. The Unix-filter path (`… --stdout | head`) and
  agent piping survive explicitly.
- `--output PATH` — keep: write to a named file (and still register in CAS?
  see D3).
- Ensembles register as **one multi-seed run** with a `seed` column in
  `camdl cat` — removes the "`--cas` supports single runs only" wall (audit
  B5) and the delimiter-less concatenated-stdout problem (audit B6).
- `fit run` already writes a hashed dir; it gains the same banner so its
  output is discoverable without a second `camdl fit where` call (audit B4).

### Small-result echo (Decision D2)

`pfilter` prints a 143-byte loglik; `eval` a small TSV. Forcing `camdl cat`
for one number is friction. Proposed rule: **everything registers in CAS;
results below a size threshold (or scalar summaries) also echo to stdout.**
So "CAS everywhere" = *every run is recorded and browsable*; *tiny results
stay glanceable*. (Alternative: strict CAS-only for uniformity — rejected as
hostile to the `pfilter` quick-check loop.)

### `--output` + CAS interaction (Decision D3)

When `--output foo.tsv` is given, do we *also* register in CAS, or treat the
explicit path as "user owns placement, skip CAS"? Recommendation: **register
in CAS and additionally copy/symlink to `foo.tsv`** — the run stays
discoverable; the named file is a convenience. (Alternative: `--output`
suppresses CAS, like `--stdout` — simpler but loses the record.)

---

## Problem 3 — The progress model

### Why the current design is wrong, not just incomplete

Progress is **nested**, and the nesting differs per command:

| command | level 1 | level 2 | level 3 | live scalars |
|---|---|---|---|---|
| simulate (1) | — | timesteps 0..T | — | ETA |
| simulate (ensemble) | seeds/draws | timesteps | — | ETA |
| pfilter | — | obs steps | (particles) | loglik, ESS |
| if2 | chains | iterations | filter sweep | loglik |
| fit / pgas | chains | warmup·sample | filter sweep | loglik, accept |
| profile | grid points | chains | iterations | loglik |
| batch | sweep points | seeds | timesteps | — |

The current renderer is flat and **gated on `n_runs > 1`** — which is exactly
why a single 45 s simulate shows nothing (the original bug) and why the 21 s
compile is invisible. You can't patch a flat model into a tree; you model the
tree.

### Design: one reporter, rendering is a projection

A `ProgressReporter` in the middle layer. Backends and inference loops call
`reporter.node(level, label).set(current, total).scalar("ll", v)`; they never
know whether bars or log-lines come out. **That is the seam** — `if2.rs`,
`chain_binomial.rs`, `pgas.rs` depend on the reporter trait, not on
indicatif. Rendering reads the tree:

- **TTY** → nested/stacked bars: outer aggregate + up to ~8 active-worker
  lines (Decision A, confirmed: capped active lines make a stuck chain
  visible).
- **non-TTY (agents/CI)** → throttled structured lines carrying *both* levels
  on one line, greppable.
- **single run** → show the inner timestep bar (deletes the `n>1` gate).
- **compile** → spinner immediately (Decision B, confirmed: the 21 s silent
  compile is the worst offender).

### Approved rendering (the mockups you signed off)

```
# single long sim (TTY) — currently shows NOTHING
compiling kano_lga_seirv.camdl … done · 4,620 compartments · 21s
simulate · chain_binomial  t 2150/4388  ████████░░░░  49%  ETA 11s

# ensemble, 4 parallel (TTY)
simulate  seeds ███░░░░░░░ 5/20                       elapsed 0:42
  seed 06  t 3800/4388 ██████████░ 87%
  seed 07  t 1200/4388 ███░░░░░░░░ 27%
  seed 08  t  900/4388 ██░░░░░░░░░ 21%

# fit, 4 chains (TTY)
fit pgas  warmup 2/4 · sampling                       elapsed 4:21
  chain 1  iter 1450/2000   ll -1234.5   acc 0.31
  chain 2  iter 1380/2000   ll -1236.1   acc 0.29

# non-TTY / agent (throttled, structured, BOTH levels)
[simulate] seed=7/20 t=1200/4388 27% eta=31s
[fit/pgas] chain=2/4 phase=sample iter=1380/2000 ll=-1236.1 acc=0.29 t=261s
```

### Mode slimming (Decision D4)

Today: `auto/pretty/plain/none` (default `auto`;
`types.rs:146-157`). `auto` already does the only split that matters —
`progress.rs:41-44`: TTY → bars, non-TTY → structured lines. Once rendering
is a projection of one tree, `pretty` (force-bars-off-TTY) and `plain`
(force-lines) are redundant. **Proposed: `auto` + `none`**, with a
`CAMDL_PROGRESS=plain` env escape hatch if anyone needs to force lines under
a TTY. Smaller surface; "the grammar fits in a head."

### Decisions C, E (confirmed by your sign-off on the mockups)

- **C** — fit shows chains×iters by default; the inner filter sweep only at
  `--verbosity debug` (the mockup shows chain/iter/ll, not the sweep).
- **E** — live scalars: `loglik` (fits/pfilter), `ETA` (sims),
  `accept`/`ESS` where cheap.

---

## Problem 4 — Flag-semantics consistency

Two layers. Layer 4a is rev-2's job (land it). Layer 4b is the orthogonal
overloads rev-2 does not cover (fix them here, same pass).

### 4a — finish the `ParameterResolver` migration (rev-2 steps 6–12)

Per [`2026-05-25-cli-init-and-params-ux.md`](2026-05-25-cli-init-and-params-ux.md):
collapse value-setting to `--fixed` (universal) and chain-start to `--init`
(inference-only), one resolver, everywhere. `eval`+`pfilter` done; migrate
`if2`/`profile`/`survey`/`fit run`; remove `--params` on inference
subcommands. **Sequencing constraint:** these touch inference math
(`if2.rs`, `pgas.rs`, `pmmh.rs`) — one subcommand per commit, green test run
between each, pattern-matching `eval.rs`'s migrated shape (per the
impl-questions handoff). The book chapter examples update in lockstep (M-1
hard break).

"Consistent with where we're going" (your phrase): the resolver already
writes `parameters_provenance` into `run.json` — which is *exactly* the CAS
record this proposal makes the default and the viewer consumes. So finishing
4a and landing CAS-by-default are mutually reinforcing, not competing.

### 4b — the overloads rev-2 does not touch (verified)

- **`--obs` means opposite things** (audit B1, observed): output *path* on
  `simulate` (`--obs cases.tsv` writes synthetic obs), input block-*name* on
  `pfilter`/`if2` (`--obs weekly_cases` selects a block). An agent that
  learned `simulate --obs file.tsv` mis-drives `pfilter`. **Proposed:**
  rename the *selector* to `--obs-block NAME` on pfilter/if2; keep `--obs
  PATH` as output everywhere. (Or the reverse — but output-`--obs` is the
  more established surface.)
- **`survey --data` = inline fixed params, not data** (audit B3, help-read):
  collides with `pfilter --data PATH` (the data TSV). **Proposed:** survey
  uses `--fixed NAME=VALUE` (the now-universal value-setter from 4a) and frees
  `--data` to mean the data file, matching every sibling.

These are pure CLI-layer renames (no inference math), so they can land early
and independently of the risky 4a commits.

---

## The CAS layout + `run.json` are the consumer contract — the run-spec defines that seam

The CAS is not just where output lands; it is the **public API between camdl
and everything that reads camdl's results** — `camdl list/show/cat`,
camdl-viewer, and any future tool or notebook. That API has exactly two
parts: the **directory layout**
(`results/sims/{sim8}/{slug}-{scen8}/seed_{n}/`) and the **`run.json`
schema** (`Run`/`RunKind`/`RunStatus`). Both are middle-layer seams: a
consumer that depends only on them is insulated from how simulation,
inference, or rendering work internally.

`docs/camdl-run-spec.md` is where that seam is *specified*. Today it
half-documents the layout (and contradicts itself, §2.4 vs §4.4) and predates
the `run_meta.rs` schema. **Part of this proposal is to make the run-spec the
authoritative contract document for the CAS API** — layout + `run.json`
schema, with the field table generated from / checked against `run_meta.rs`,
and the `status` lifecycle and `parameters_provenance` documented as
load-bearing for consumers. camdl-viewer's `cas.py` then cites run-spec §N as
its source, not a reverse-engineered guess. That is the same "consumer
contract" discipline applied to a data format instead of a function
signature: define the seam once, in the spec, and let both sides depend on
the spec rather than on each other.

To keep the viewer a static reader over `run.json` — no server, no DB —
*because* the CAS is the API:

1. **One schema, no dual-format tax.** The viewer's `cas.py` currently
   carries `normalize_run_json` to handle *old-flat* + *new-tagged*. At alpha
   (backcompat is a non-goal) we regenerate all example/golden output to the
   tagged `Run`/`RunKind` schema and drop the old-flat branch. *Coordination,
   not unilateral deletion* — the viewer is a separate repo; we land the
   regen, then PR the viewer to delete the dead branch.
2. **`status` is the live-run signal.** Write `run.json` with
   `status="running"` at start; flip to `{"completed":{wall_time_seconds}}`
   at end. The viewer and `camdl list` then show in-progress runs — the
   progress tree and the CAS record are one object.
3. **`parameters_provenance` (from 4a) lands in every record** so the viewer
   can show "this value came from `--fixed`, that from a scenario preset"
   without re-deriving precedence.

*To verify before implementation:* point the viewer at freshly-regenerated
CAS output and confirm Tab 1 (`run.json` display) + Tab 2 (ensemble) render
without the old-flat fallback.

---

## Open decisions (need your call)

- **D1 — output root:** ✅ resolved → `results/` (shipping-code default; the
  `output/` viewer scaffold is corrected, not followed).
- **D2 — small-result echo:** CAS-register-and-also-echo tiny scalars vs
  strict CAS-only. *Lean: echo small results.*
- **D3 — `--output` + CAS:** register in CAS *and* write the named file, vs
  `--output` suppresses CAS like `--stdout`. *Lean: register + write.*
- **D4 — progress modes:** slim to `auto`/`none` (+env) vs keep four. *Lean:
  slim.*
- (A, B, C, E already confirmed via the mockup sign-off.)

---

## Implementation sequencing

Ordered to put low-risk, high-visibility wins first and quarantine the
inference-math commits:

1. **Run-spec reconciliation** (doc-only): make it the authoritative CAS-API
   contract — one output root (`results/`), fix the §2.4/§4.4 contradiction,
   document the `run.json` schema against `run_meta.rs`, the `status`
   lifecycle, and CAS-as-default + `--stdout`. *(No code; this is the seam
   definition the viewer cites.)*
2. **`--obs-block` / `survey --data` renames** (CLI-layer only, no inference
   math). Tests + golden CLI snapshots.
3. **CAS-by-default + `--stdout` + banner** for `simulate` (then `pfilter`,
   `eval`), incl. ensemble-as-one-run + `seed` column. `RunStatus=running`
   written at start.
4. **`ProgressReporter` tree + renderers**, single-run + compile coverage,
   mode slim. Wire `simulate`/`pfilter` first (no inference-math risk in the
   reporter itself).
5. **Finish rev-2 resolver migration** — `if2`/`profile`/`survey`/`fit run`,
   one per commit, green between each, book examples in lockstep. CAS-default
   + progress for these as they migrate.
6. **Viewer coordination** — regen example/golden output to tagged schema;
   PR `camdl-viewer` to drop `normalize_run_json`'s old-flat branch.

Each step is independently shippable and testable. Steps 1–4 carry no
inference-math risk; step 5 is the conservatively-scoped one.

## Verified current behavior (real runs, 2026-05-30, `camdl 0.1.0+8cdedd0`)

Captured against the viewer's bundled `he_measles.camdl` + `params.toml`
(golden models have no param defaults so can't `--cas`). Artifacts in
`/tmp/casdemo`, `/tmp/cap.log`, `/tmp/cap2.log`.

**Confirmed (schema is real — resolves the earlier caveat):** a real
`run.json` matches `run_meta.rs` exactly — top-level `hash, version,
created_at, argv, status, kind`; `status: {"completed":{"wall_time_seconds":
0.0076}}`; nested `kind: {"kind":"simulate", model, model_hash, scenario,
sim_hash, scen_hash, seed, backend, dt}`. Layout: `results/sims/
he_measles-<sim8>/baseline-<scen8>/seed_1/{run.json,traj.tsv}`. Default root
is `./results` (confirms **D1 = `results/`**).

**Bugs/frictions found by running (new, beyond the audit):**

- **`--cas` prints `cached:` on a FIRST write** — verbatim stderr:
  `cached: he_measles-162c0116/baseline-e9235d61/seed_1`. It was just
  written, not a cache hit. The banner wording is simply wrong and must
  distinguish *wrote* from *cache hit*. (This is the banner the proposal
  replaces with `✓ run <hash>`.)
- **`--cas` is additive, not a redirect** — with `--cas` the trajectory is
  *still* dumped to stdout (126,132 bytes) **and** written to CAS. So today
  `--cas` doesn't solve the terminal-flood at all. CAS-by-default + the
  trajectory going to CAS-only (stdout reserved for `--stdout`) is the actual
  fix.
- **🔴 CRITICAL: `model_hash` is `SHA256("")` → `--cas` can serve the WRONG
  model's trajectory** (gh#135). `SimulateMeta.model_hash` is the empty-string
  hash (`e3b0c442…855`) for every model (`.camdl` or IR). Since
  `sim_hash = hash(model_hash, params, backend, dt)` (`hashing.rs:38-40`), the
  cache key ignores model structure. **Verified collision:** two models
  differing only in `R0` (15 vs 30), same params/seed/backend/dt, same `--cas`
  dir → identical `sim_hash 162c0116`, one dir; the second run was a **cache
  hit serving the first model's trajectory** (`sha256(v2-in-shared)==sha256(v1)`
  TRUE, `==sha256(v2-alone)` FALSE; `run.json.argv` still records v1). This is
  a priority-zero silent-wrong-answer bug. **Hard sequencing constraint: this
  proposal must NOT flip the output default to CAS until gh#135 is fixed** —
  doing so would expose every user to silent cache collisions on the common
  "edit model, re-run with same params" loop. Step 3 (CAS-default) is blocked
  on gh#135; everything else (progress, flag fixes, run-spec) can proceed.
- **`camdl list` works — but the root argument is a sharp edge** (correcting
  an earlier misread of mine). `list /tmp/casdemo` *does* find the run and
  prints a clean table (`CREATED HASH LABEL MODEL SCENARIO SEED … PATH`). The
  trap: it wants the dir *containing* `sims/`, so `list /tmp/casdemo/sims`
  returns "(no cached runs)" — pointing one level too deep silently shows
  nothing. CAS-by-default should make `list` default to `./results` with no
  argument so this edge rarely bites.
- **`list --root` is rejected** (`unexpected argument '--root'`); `list`
  takes a positional `[ROOT]` while `show --root` takes a flag. Minor
  flag inconsistency to harmonize.
- **`show` emits raw ANSI to stdout even when piped** (audit B10 confirmed,
  not softened): 32 ESC bytes survive `| cat` — `show` does not TTY-detect
  stdout. The `human` default is not pipe-safe; `--format json` is the
  scripting escape hatch. Same unconditional-color issue affects `fit`'s
  stderr markers. Fold into the progress/render pass: gate color on
  `stdout.is_terminal()`.
- **`--cas` rejects multi-run** (verbatim): `--cas supports single runs only.
  For sweeps … use 'camdl batch run FILE'`. Confirms audit B5.
- **Audit B6 was WRONG (correcting my earlier claim):** multi-seed stdout
  *is* delimited — by a leading **`replicate`** column (`replicate t S E I R
  flow_…`). But it's `replicate` (1,2,3), **not the seed value**, so a row
  can't be mapped back to its seed. Friction, not a blocker.
- **The viewer's own `batch.toml` is REJECTED by current camdl** — a scenario
  that names a model preset AND carries inline `params = {…}` errors:
  "A scenario reference is either a model preset OR an ad-hoc patch, never
  both." The viewer's `strong_seasonality`/`vaccination` scenarios do exactly
  this → **`make` in camdl-viewer is currently broken**, which is why no
  `manifest.json` exists to confirm. (Viewer scaffold bug; fix when we PR it.)

## Current progress output (verbatim, captured 2026-05-30) — the "before"

Captured against the book's `guide/getting-started/sir.camdl` + synthetic
obs. These are what the gallery below replaces.

**pfilter** — stdout is a **bare number** (no label); stderr is 2 setup
lines; **nothing during filtering**:
```
# stdout                          # stderr
-59.4996                          pfilter: 12 observations × 1 streams, 1000 particles, dt=1, seed=1
                                  pfilter: bound streams: weekly_cases(neg_binomial)
```

**fit run — the most informative surface today (real capture, exit 0).** This
is the actual `--progress plain` stderr, verbatim and abridged:
```
fit: /tmp/fit_sir.toml (1 stage)
  model:    …/sir.camdl
  estimate: beta, gamma, rho
  fixed:    N0, I0, k
  output:   results/fits/fit_sir-8c096b3d        ← prints the landing dir (B4 fix, already here)
── stage: scout (method=if2) ──
running 2 chains × 400 particles × 6 iterations, cooling=0.7, dt=1
transforms:
  beta   log    [0.05, 1]   log(0.2214) = -1.51  rw_sd=0.0335 (auto)
  ⚠ 3/3 parameters using auto rw_sd. Check traces and set explicit values.
cooling: cf50=0.70 over 6 iterations × 12 observations
[… INFO camdl::fit::runner] fit chain 1 iter 1/6 ll=-81.7
[… INFO camdl::fit::runner] fit chain 1 done iter 6/6 final_ll=-52.5
evaluating loglik (every 10 iterations, all 2 chains)...
best chain: 2 (loglik=-50.59 ± 0.02)
Â:
  beta   Â=2.002 ✗
  gamma  Â=8.122 ✗
  rho    Â=1.415 ~
dt-convergence at θ̂: PASS
```
What this gets *right* and the others lack: a header naming model/estimate/
fixed, **the landing dir printed inline** (the audit-B4 fix — already present
on `fit`, absent on `simulate`/`pfilter`/`if2`), rw_sd transform context, a
diagnostics block (Â, dt-convergence), and per-iter loglik lines.

What it gets *wrong* (so this is NOT a finished north star — correcting an
earlier draft of this section that claimed it shipped progress bars; it does
not):
- **No progress bars** — it's raw `log::info!` lines (`[timestamp INFO
  camdl::fit::runner] …`), module path and all. Fine for a log, noisy for a
  human watching a run.
- **Only `iter 1/N` and `done N/N` print per chain — the middle is silent.**
  (Verified by polling stderr-line growth over wall time: setup at ~30ms,
  `iter 1` at ~50ms, then the bulk of iterations + the loglik re-scoring
  block flush near the end.) On a minutes-long fit that silent middle is
  the gap. NB *correcting an earlier claim of mine*: the per-iteration
  `log::info!` lines do **not** vanish under default `auto` on a non-TTY —
  `auto` falls back to the same plain log lines as `--progress plain`
  (subagent-verified, 878 bytes, identical structure). The problem is the
  *silent middle* + raw formatting, not suppression.
- **Raw timestamps + `camdl::fit::runner` module path leak** into user-facing
  output.
- **No CAS-style `✓ run <hash>` banner** — it prints the dir in the header but
  has no completion line a consumer can grep for done-ness, and the dir is a
  path, not the short hash `camdl show`/the viewer key on.

So the gap is *consistency and polish*: `fit run` has the richest content but
renders it as debug logs; `simulate`/`pfilter`/`if2` have almost nothing. The
`ProgressReporter` (below) factors a shared seam so all of them emit the same
structured tree — `fit`'s content, rendered as bars on a TTY and clean
structured lines off it, every command, no `log::info!` leakage.

**Standalone `camdl if2` — invocable, but via a confusing overload**
(corrected; gh#137). It *does* run from scratch (verified, exit 0). In
explicit `--rw-sd "beta=…,gamma=…"` mode, `--fixed-file start.toml` supplies
the **starting values for the estimated params**, while `--fixed` pins the
rest. The overload: a flag family named `--fixed` is the only way to seed a
param you are *estimating*. With `--rw-sd auto` and no `--fixed-file` you
still get `error: parameter 'beta' has no value`. Real captured stderr
(`--progress plain`, abridged):
```
if2: 12 observations, 2 chains × 300 particles × 4 iterations, cooling=0.95, dt=1, seed=1
if2: regime=manual, estimating 2 parameters, 4 fixed, threads=16
[… INFO camdl::if2] if2 chain 1 iter 1/4 ll=-52.7
[… INFO camdl::if2] if2 chain 1 done iter 4/4 final_ll=-52.6
evaluating loglik (every 10 iterations, all 2 chains)...
  chain 1: ll=-50.5      chain 2: ll=-51.2
Â (across 2 chains, last 2 iterations):
  beta   Â=3.35 ✗   gamma  Â=1.85 ✗
Best chain: 1 (loglik=-50.49)
MLE estimates (best chain): beta = 0.594652  gamma = 0.204608  loglik = -50.49
```
Same shape as `fit run`: raw `[… INFO camdl::if2]` lines, only `iter 1/N` +
`done N/N` per chain (silent middle), and stdout is a per-iteration TSV trace
(`chain iteration if2_perturbed_loglik beta gamma`). So if2's "before" is the
same problem as fit's: rich content, debug-log rendering, silent middle. Fix
= land `--init` on `if2` (rev-2/gh#83) so estimated-param starts have a
clearly-named home instead of `--fixed-file`.

**fit.toml has a steep first-write cliff (audit B14, now quantified):** five
sequential schema errors to a valid config — `camdl=` not `model=`,
`algorithm=` not `method=`, `[data.observations]` map, then per-*stage*
`cooling` **and** `backend` both required (config-level `backend` does not
satisfy the stage). Each is a clean line/caret error, but it's five
round-trips. Separate proposal, flagged here as evidence.

**First-run traps in the book's own model:** `guide/getting-started/
sir.camdl` has no param defaults and ships no `params.toml`, so a newcomer's
first `simulate` fails "parameter 'beta' has no value"; and `N0`/`I0` are
`count` (no bounds) so they can't be auto-estimated. Getting-started
frictions (separate fix), noted as evidence.

## Progress mockup gallery — the validation oracle (the "after")

This gallery is the **acceptance oracle**: after implementation, the agent
runs each command and diffs real stderr against these blocks. Each shows
TTY (bars) and the non-TTY/agent (structured-line) rendering. Numbers are
illustrative; structure is normative.

All commands converge on one look: a one-line context header, a bar (TTY) or
structured line (non-TTY) per active work unit, and a `✓ <kind> <hash> ·
<next-command>` banner. (This is `fit run`'s *content* — header, per-unit
progress, landing id — rendered consistently, not its current `log::info!`
formatting.)

### simulate — single run (the original bug: currently shows NOTHING)
```
# TTY
compiling kano_lga_seirv.camdl … done · 4,620 compartments · 1.8 GB IR · 21s
simulate · chain_binomial   t 2150/4388  ████████░░░░  49%   ETA 11s
✓ run a1d91886 · 160 MB traj · camdl cat a1d91886 | less

# non-TTY (agent/CI) — throttled (~1 line/sec), greppable
[compile] kano_lga_seirv.camdl compartments=4620 … done t=21s
[simulate] backend=chain_binomial t=2150/4388 49% eta=11s
[done] run=a1d91886 bytes=167772160 cat="camdl cat a1d91886"
```

### simulate — ensemble (seeds, 4 parallel)
```
# TTY — outer aggregate + up to ~8 active workers
simulate  seeds ███░░░░░░░ 5/20                         elapsed 0:42
  seed 06  t 3800/4388 ██████████░ 87%
  seed 07  t 1200/4388 ███░░░░░░░░ 27%
  seed 08  t  900/4388 ██░░░░░░░░░ 21%
✓ run 7c4d20fe · 20 seeds · camdl cat 7c4d20fe

# non-TTY
[simulate] seeds=5/20 active=[s6:87% s7:27% s8:21%] elapsed=42s
[done] run=7c4d20fe seeds=20 cat="camdl cat 7c4d20fe"
```

### pfilter — loglik at fixed θ (tiny scalar; see D2 below)
```
# TTY   (today: bare "-59.4996" + 2 setup lines, no progress bar)
pfilter · 1000 particles   obs 48/52  ███████████░  ll -1234.5  ESS 612
log-likelihood: -1234.53                          # ← labeled; today it's bare
✓ run 3f9a2c10 · camdl show 3f9a2c10

# non-TTY
[pfilter] obs=48/52 ll=-1234.5 ess=612
log-likelihood: -1234.53
[done] run=3f9a2c10
```

### if2 — iterated filtering (2 chains)
```
# TTY
if2  chains ██████░░ ·                                   elapsed 1:18
  chain 1  iter 42/100  ll -1240.1
  chain 2  iter 39/100  ll -1242.8
✓ run b81e… · camdl fit summary b81e…

# non-TTY
[if2] chain=1/2 iter=42/100 ll=-1240.1
[if2] chain=2/2 iter=39/100 ll=-1242.8
[done] run=b81e… summary="camdl fit summary b81e…"
```

### fit run / pgas — keep the rich content, render it cleanly
`fit run` already has the best *content* (header, transforms, Â, dt-check); the
redesign keeps all of it and replaces the `log::info!` iteration spew with the
bar (TTY) / structured line (non-TTY) renderer, drops the timestamp+module
prefix, and adds the `✓ … → <hash>` banner. The header block stays.
```
# TTY  (header + transforms + diagnostics unchanged from today; only the
#        per-iteration log::info! lines become a bar, + a banner at the end)
fit: results/fits/sir-3818f1c8 · scout(if2) → refine(pgas)
── stage: refine (pgas) ──  4 chains · warmup 1000 + sample 1000
  chain 2/4  sample 1380/2000  ll -1236.1  acc 0.29        elapsed 4:21
Â: beta 1.01 ✓  gamma 1.00 ✓  rho 1.02 ✓
✓ fit sir-3818f1c8 · camdl fit summary sir-3818f1c8

# non-TTY  (replaces the raw "[… INFO camdl::fit::runner] …" lines)
[fit/pgas] chain=2/4 phase=sample iter=1380/2000 ll=-1236.1 acc=0.29 t=261s
[done] fit=sir-3818f1c8 summary="camdl fit summary sir-3818f1c8"
```

### profile (grid × chains)
```
# TTY
profile tau  grid ████░░░░░ 12/30                        elapsed 6:02
  cell tau=-18.2  chain 3/4  iter 820/1000  ll -1255.0
✓ profile c4f0… · camdl cat c4f0…
```

### completion / error footers (uniform across commands)
```
✓ run <hash> · <summary> · <next-command>        # success
✗ run <hash> failed at obs 31/52: PFDegenerate   # error: record kept, status=failed
```

## D2 — the "tiny scalar" issue, concretely

Most runs produce big artifacts (a 160 MB trajectory) that *belong* in CAS,
not on a terminal. But a few commands produce a **single number or a handful
of rows** — and for those, "go run `camdl cat <hash>`" to read one float is
hostile to the tight `pfilter` loop (vary θ, re-check loglik) an analyst runs
dozens of times.

Concretely, `pfilter` today prints exactly this to stdout (verbatim, ~9
bytes + newline):
```
-59.3958
```
Under strict CAS-only, that becomes: run → `✓ run 3f9a2c10` → `camdl cat
3f9a2c10` → `-59.3958`. Three steps for one number.

**Proposed (D2): everything registers in CAS; results at/under a small
threshold ALSO echo to stdout.** So `pfilter` stays a one-liner *and* becomes
reproducible:
```
$ camdl pfilter sir.camdl --fixed beta=0.4 --data cases.tsv --particles 1000
log-likelihood: -59.3958                          # ← stdout, glanceable
✓ run 3f9a2c10 · camdl show 3f9a2c10              # ← stderr, the record
```
The threshold applies to *result size*, not command identity: a `pfilter
--replicates 50` (50 logliks) or an `eval` over 4,000 timesteps crosses it
and goes CAS-only with the banner; a single loglik echoes. Commands affected:
`pfilter` (scalar), `eval` (small TSV), `fit summary`/`compare` (already
small text reports — unchanged). `--stdout` still forces full output for any
of them.

**How is the scalar stored in the CAS? (your question) — as a typed field in
`run.json`, not a separate file or a stringly blob.** `run.json` *is* the
JSON record; the scalar becomes a typed field in the kind-specific metadata,
following the precedent already in the codebase:
`FitStageMeta.best_loglik: Option<f64>` (`run_meta.rs:307`) — a fit stage
already stores its loglik as a typed `f64` in its `run.json`. So:

- **Today** `pfilter` does **not** register in CAS at all — it only does
  `println!("{:.4}", result.log_likelihood)` (`pfilter.rs:504`); there is no
  `RunKind::Pfilter` (variants are `Simulate, Fit, FitStage, Profile, Survey,
  Batch`).
- **Proposed:** add `RunKind::Pfilter(PfilterMeta { loglik: f64, loglik_se:
  Option<f64>, n_particles, ess: Option<f64>, n_obs, … })` so a pfilter run
  registers like any other. The stdout echo (`log-likelihood: -59.3958`) is a
  *rendering of that field*, not a second source of truth. `camdl show <hash>`
  reads the same `loglik` field back; the viewer can table pfilter runs.

This keeps the contract uniform: every run kind → one `run.json` with typed,
kind-specific fields; scalars are fields, trajectories/traces are sibling
files (`traj.tsv`, `trace.tsv`). No special "scalar storage" mechanism — the
existing `best_loglik` pattern generalizes.

## D4 — mode slimming, concretely

Today: `--progress auto|pretty|plain|none` (default `auto`). The reason there
are four is that rendering was decided ad-hoc per call site, so each needed
its own override. Once rendering is a *projection of one tree*, the only
distinction that carries information is **bars (interactive) vs structured
lines (machine)** — and `auto` already picks that correctly from
`stderr.is_terminal()` (`progress.rs:41-44`). `pretty` (= force bars even when
not a TTY) and `plain` (= force lines even on a TTY) are escape hatches for
cases that barely exist.

**Proposed (D4): `--progress auto|none` + `CAMDL_PROGRESS` env override.**

```
--progress auto   (default)   TTY → nested bars;  non-TTY → structured lines
--progress none               nothing on stderr (CI that only wants the banner)
CAMDL_PROGRESS=plain          force structured lines even under a TTY
                              (the rare `tee`/`script(1)`/screen-recording case)
CAMDL_PROGRESS=bars           force bars even when piped (almost never needed)
```

What each currently-named mode maps to:
| today | rev-3 |
|---|---|
| `auto` | `auto` (unchanged default) |
| `plain` | `CAMDL_PROGRESS=plain` (env, not a flag) |
| `pretty` | `CAMDL_PROGRESS=bars` (env, niche) |
| `none` | `--progress none` (kept) |

Net: the *flag* surface is two values (the only choice a user makes at the
prompt is "show me progress or don't"); the force-a-specific-renderer cases
move to an env var that an automation harness sets once. Smaller grammar, no
lost capability. The `--progress plain` auto-bumps-verbosity behavior
(`progress.rs`) folds into the structured-line renderer, which always emits
at the right level.

## Out of scope

- Changing inference algorithms or numerics (this is I/O + UX only).
- The `fit.toml` schema ergonomics (audit B14: `camdl=`/`algorithm=`/
  `[data.observations]`) — real, but a separate proposal.
- A live web viewer / server (the static CAS reader is the design).


## Addendum (2026-05-30): filed issues, sequencing blocker, fit-output policy

### Filed upstream issues (verified reproductions, this investigation)
| # | Title | Severity | Relation to this proposal |
|---|---|---|---|
| [#135](https://github.com/vsbuffalo/camdl/issues/135) | `model_hash = SHA256("")` → `--cas` serves wrong model's trajectory | 🔴 critical (silent wrong answer) | **BLOCKS** step 3 (CAS-default). Must land first. |
| [#136](https://github.com/vsbuffalo/camdl/issues/136) | `--cas` prints `cached:` on a first write | low | Subsumed by step 3's `✓ run <hash>` banner; one-line fix if sooner. |
| [#137](https://github.com/vsbuffalo/camdl/issues/137) | standalone `camdl if2` un-invokable from scratch (no `--init`) | medium | Resolved by step 5 (land `--init` on `if2`, rev-2 + gh#83). |
| [#131](https://github.com/vsbuffalo/camdl/issues/131) *(pre-existing)* | `--progress` not working for `camdl simulate` | — | The original bug that opened this thread; resolved by steps 3-4. |
| [#83](https://github.com/vsbuffalo/camdl/issues/83) *(pre-existing)* | `init`: `from_prior`/`from_posterior` chain-start modes | — | Part of the rev-2 `--init` family that step 5 lands. |

### Sequencing blocker (hard)
**Step 3 (flip output default to CAS) is blocked on #135.** Making CAS the
default while `sim_hash` ignores model content would expose every user to
silent cache collisions on the routine "edit model → re-run, same params"
loop. Order: fix #135 (+ regression test that two structurally different
models get different `sim_hash`) → then step 3. Steps 1 (run-spec), 2 (flag
renames), 4 (progress for simulate/pfilter), and the #136/#137 fixes carry no
dependency on #135 and can proceed in parallel.

### Fit-output policy (your question: "print all of that message still?")
**Yes — keep 100% of the content; change only the rendering.** The captured
`fit run` output (header naming model/estimate/fixed/output-dir, rw_sd
transform table, the `⚠ auto rw_sd` warning, cooling schedule, per-iteration
loglik, the `Â` block, `dt-convergence PASS`) all stays and stays at default
verbosity. The redesign changes only *form*, not *content*:
- the per-iteration lines stop being raw `[<ts> INFO camdl::fit::runner] …`
  log records and become the bar (TTY) / structured line (non-TTY) the other
  commands share — and appear without needing `--progress plain`;
- the timestamp + `camdl::fit::runner` module-path prefix is dropped from
  user-facing output (still available at `--verbosity debug`);
- a `✓ fit <hash> · camdl fit summary <hash>` completion banner is added,
  keyed on the short hash the viewer/`show` use.

Nothing informative is removed. `fit run` already has the richest content of
any command; the job is to give the *other* commands that richness and stop
`fit` from rendering it as debug logs.
