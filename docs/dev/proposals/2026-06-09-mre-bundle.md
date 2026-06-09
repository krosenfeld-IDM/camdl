---
date: 2026-06-09
status: proposal
area: cli / compiler (camdlc) / bundle format
related:
  - 2026-06-02-cas-run-identity (runid crate; ContentHash identity)
  - gh#211 (spun off: warn on absolute read() paths — portability lint)
issues: gh#212 (tracking)
---

# `camdl mre`: one-command minimal-reproducible-example bundles

## Problem

camdl is alpha with a growing alpha-user base, and the maintainer is the sole
developer. Bug reports arrive as a model, some data, and a fit/simulate
invocation that produces a wrong number or a crash. Reproducing them today means
manually reconstructing the reporter's exact closure: the `.camdl`, every
covariate/contact/coupling table the model reads, the observed data (and
holdout), the fixed-params file, the fit.toml, and the `dt`/backend/seed — then
discovering, three iterations in, that a contact matrix the model `read()`s at
compile time was never sent.

The open-issue profile is exactly the population an MRE serves. Two halves:

- **Silent mis-fit / wrong-inference bugs** — gh#186 (params in a
  `time_function` frozen at compile), gh#187 (PGAS ignores scheduled
  interventions), gh#191 (ODE-coupled reservoir frozen at init), gh#197/gh#200
  (NUTS density/gradient inconsistency), gh#180 (dropped chain-rule term),
  gh#129 (survey ranks by likelihood not posterior). The symptom _is_ a wrong
  number; it is invisible without the reporter's exact model × data × config ×
  seed.
- **Engine / CLI edge cases** — gh#198 (double-fired intervention at colliding
  dt-steps), gh#199 (negative `add` bypasses the guard), gh#208 (rate clamp),
  gh#169 (filename-too-long on CAS commit), gh#207 (RAM blowup at P=244), gh#202
  (lineage + sparse-fold uncompilable). Structural; reproducible from the model
  shape alone.

The marginal cost of building this is low because the input closure is already
**enumerable from the resolved job** for everything except one piece — the
model's compile-time `read()` closure — and that one piece has a clean compiler
seam. The asymmetry (high value, low marginal cost) is the ROI case.

## What an MRE must capture (the input closure)

A fit's external inputs live in **three** places, not one:

| Source                                          | Where it is named                                       | Captured by          |
| ----------------------------------------------- | ------------------------------------------------------- | -------------------- |
| `model.camdl`                                   | `fit.toml [model] camdl` (`config_v2.rs:136`)           | direct               |
| observed data, holdout                          | `fit.toml [data]` (`config_v2.rs:191`)                  | direct               |
| fixed params                                    | `fit.toml [fixed] from_file` (`config_v2.rs:598`)       | direct               |
| synthetic ground truth                          | `fit.toml [synthetic] true_params` (`config_v2.rs:307`) | direct               |
| survey landscape (init)                         | `fit.toml … survey_path` (`config_v2.rs:854`)           | direct (a CAS dir)   |
| **compile-time tables / covariates / forcings** | **inside `model.camdl` via `read(...)`**                | **compiler depfile** |
| external tables (`external(...)`)               | CLI `--table NAME=FILE` (sim/pfilter/profile only)      | direct (CLI)         |

The first five are read straight off the resolved `FitConfigV2`. The CLI
`--table` case is read off the parsed args. The sixth row is the one that does
not appear anywhere in the fit surface — and it is usually the bulk of the
"covariate/table data."

### When do covariate tables get in? The compiler step.

This is worth stating plainly because it drives the whole design. There are two
distinct table mechanisms (`ir/table.rs:13-23`):

- **`TableSource::Inline`** — the model writes
  `pop = read("pop.tsv",
  column="patch")` or a contact matrix
  `read("contact.tsv")`. `camdlc` opens and reads the file _at compile time_
  (`expander.ml:305` `read_csv_rows` → `open_in abs_path` at line 328, resolved
  relative to the model dir) and **bakes the values into the IR** as inline
  literals. The file is a compile-time dependency of `model.camdl`; it never
  appears in fit.toml or on the fit CLI.
- **`TableSource::External`** — the model writes `external("contact")` and the
  values are injected _at runtime_ via `--table contact=matrix.tsv`.

Now the key fact, verified: **`camdl fit run` exposes neither `--table` nor a
`[tables]` block.** `FitRunArgs` is config-only — its fields are
`config`/`stage`/`seed`/`sweep`/… (`args/mod.rs:715`), with no
`InferenceModelOverrides` and so no `--table`; and `FitConfigV2` has no tables
field (`config_v2.rs:26`). `--table` exists only on the flag-driven commands
(`ModelOverrides`/`InferenceModelOverrides`, `args/mod.rs:240,271` → simulate,
pfilter, profile). **Consequence: a model used in a fit must carry all its
tables inline, i.e. via compile-time `read()`.** So for fits, _all_
covariate/table data enters at the compiler step, and an MRE bundler cannot find
those files by inspecting fit.toml or the CLI — it must ask the compiler what it
read.

That is the architectural reason the compiler change below is load-bearing, not
a nicety.

## Part 1 — Compiler: `camdlc` emits its read-closure

### Does this change the IR? No.

The read-closure is **compile-time provenance**, not model semantics. The Rust
runtime never consults it — by the time the IR exists, every inline table's
values are already baked in. Two reasons it must stay _out_ of the IR:

1. **Identity.** The IR is content-hashed for run identity
   (`runid::inputs::ModelDigest.ir = model.content_hash()`,
   `inputs.rs:153-171`). A `reads` field would fold **absolute, machine-specific
   paths** into the identity hash — a correctness bug for the CAS (two machines,
   same model, different `run_id`).
2. **Cost.** An IR field is a schema change: `ir/schema.json` + `ir/VERSION` +
   both language types + every golden, regenerated — for data the simulator
   never reads.

So the depfile is a **sidecar**, emitted alongside the IR, never inside it.

### Design decision: record-at-the-chokepoint vs re-derive-from-decls

Two designs were researched (each grounded in the code; both run _one_ real
compile — neither is a naive source regex):

- **Design A — instrument the chokepoint.** Every external-data read in the
  compiler funnels through a _single_ function, `read_csv_rows`
  (`expander.ml:305`, whose only `open_in` is line 328): the inline-table loader
  (`load_table_data`, 386→586), the `DRead` dimension-column loader (709→742),
  and the file-backed `interpolated()` forcing loader (3684→3715) all route
  through it. There is no other data read in the expander (the only other
  `open_in` in `ocaml/lib/compiler` is `inspect.ml`/`doctest.ml` reading
  _source_). So A adds one `mutable reads` field to the expander context (beside
  the existing mutable `diags`, `expander.ml:27`), one push at `expander.ml:306`
  recording the _resolved_ `abs_path`, and a `--emit-deps` flag that writes the
  set. It records **what the compile actually opened**.
- **Design B — re-derive after the compile.** `camdlc inspect` already runs the
  full front-end (`Compiler.compile_detail_result` → `Expander.expand_detail`,
  `inspect.ml:1263`) and already extracts `read(...)`/`external(...)` paths from
  the AST (`table_source_label`, `inspect.ml:1083`). So B adds a read-only
  `deps` view that walks `ctx.table_decls`/`ctx.dim_decls`/`ctx.func_decls` off
  the already-returned `ctx` — **touching neither `read_csv_rows` nor
  `Compiler.compile`'s return**, reusing `extract_path_arg`/`resolve_data_path`
  to avoid resolution drift.

B's case is real and worth stating: it edits **none** of the surface CLAUDE.md
marks high-risk (no field on the hot expander context, no push in the read
primitive, no change to the compile boundary), and it rides machinery that ships
today.

**Verdict: Design A.** Three things decide it.

1. **Exact-by-construction beats re-derivation — this is settled industry
   practice.** The prior-art survey is unambiguous: gcc/clang (`-MMD`), rustc
   (`--emit=dep-info`), and the Ninja/Make depfile protocol all emit the dep set
   **as a byproduct of the real compile**, precisely because anything that
   re-computes it can drift. gcc's history is the cautionary tale — the separate
   `-M` preprocess pass could disagree with the real compile, and the fix was to
   move capture _inside_ the compile (`-MMD`). OCaml's own `ocamldep` is the
   standing example of a separate static scanner that is documented-imprecise in
   both directions. For an MRE bundler the failure is asymmetric and silent: a
   missed file packs cleanly and fails to reproduce on the maintainer's machine
   — the exact pathology the tool exists to kill. A records reality; B
   re-derives a model of it.
2. **The reads are conditional on expansion, which favors recording.** A
   file-backed indexed `interpolated()` forcing is read per-stratum-level
   (`expander.ml:3790-3804`) and a `DRead` dimension _derives its levels from a
   file_ (`expander.ml:783`) — what gets opened is downstream of dimension
   resolution. The _set_ of distinct files is recoverable from the decls (so B
   is not wrong today), but "record what was opened" is the honest invariant;
   "walk the three decl lists and re-resolve" must be kept in lockstep with the
   reader set and the skip rules (E222 no-dimension `read()`, missing-file
   skips) the compile already applied.
3. **B's reuse advantage is smaller than it looks.** `table_source_label` covers
   only `table_decls`; B must still hand-write path extraction for `dim_decls`
   (`DRead`) and `func_decls` (forcings) — the two readers where the drift risk
   actually lives. A captures all three uniformly at the one chokepoint they
   already share, and any _future_ reader that routes through `read_csv_rows` is
   captured for free.

The risk A pays is genuinely small: the expander context is _already_ a
mutable-accumulator record (it accumulates `diags`, `comp_decls`, `param_decls`,
… as it expands); A adds one more field of the same kind and one `::` at the
line where the path is resolved. It touches no inference-math file, no IR, no
schema, no golden, and is **observably inert without `--emit-deps`** (IR on
stdout byte-identical). Both designs still lean on the round-trip `run_id` test
(Testing plan) as the completeness oracle — a dropped file shifts the IR digest
and the `run_id` diverges — but A needs that backstop _less_, which is the
point.

### The shape (Design A)

The return-shape sub-decision resolves cleanly. `Compiler.compile`
(`compiler.ml:399`) returns `(Ir.model, string) result` and has exactly **one
production caller** (`camdlc.ml:187`; ~60 others are tests). There are **no
`.mli` files** in `ocaml/lib/compiler/`. So:

- Add an accessor `Expander.reads ctx`, mirroring the existing
  `Expander.transition_loc ctx` / `compartment_loc` accessors
  (`expander.ml:619-637`, re-exposed at `compiler.ml:185-188`).
- Add a sibling entry
  `Compiler.compile_with_reads : … -> (Ir.model * (string *
  string) list, string) result`,
  implemented as `compile` + `Expander.reads`. **`compile` stays
  byte-identical** — the ~60 test callers and the IR output are untouched; only
  `camdlc.ml:187` switches to the new entry when `--emit-deps` is set.

camdlc gets one new flag on the compile subcommand (`camdlc.ml:154`, alongside
`-o`/`--set`/`--json-errors`):

```
--emit-deps FILE    after a successful compile, write the read-closure to FILE (JSON)
```

Two refinements from the prior-art survey: write the depfile **atomically
(temp + rename) and only on a successful compile** (a killed/failed build must
never leave a half-written or stale depfile), and record the **resolved,
canonicalized** path (the bundler copies it; the as-written form is kept
alongside for human readability). Dedup preserving insertion order. We keep JSON
rather than the Make-rule `.d` format because the sole consumer is `camdl mre`,
which wants structured `(as_written, resolved)` pairs; a Make-format emitter is
a trivial later add if a build tool ever needs to consume it (e.g. the IR cache
invalidating on a `read()`-file edit — gh#141 — a real future use that wants
exactly this resolved list).

Depfile shape (JSON sidecar, machine-read by `camdl mre`):

```json
{
  "schema": 1,
  "model": "models/he2010.camdl",
  "reads": [
    {
      "as_written": "data/contact.tsv",
      "resolved": "/abs/.../data/contact.tsv"
    },
    { "as_written": "data/pop.tsv", "resolved": "/abs/.../data/pop.tsv" }
  ]
}
```

`camdl mre` already compiles the model once (it needs the IR for the obs-stream
names, for `--verify`, and to stamp the origin `run_id`). It passes
`--emit-deps` on that **same** invocation — one compile yields both the IR and
the depfile. No second compile; `run_camdlc` (`util.rs:312`) grows one optional
arg.

A note on scope creep avoided: depfile emission is **off by default** (only
under `--emit-deps`), JSON not Make-format, and not wired into the IR cache in
v1. One flag, one JSON file, one consumer.

## Part 2 — The `camdl mre` command

### Shape decision: custom command, per-source subcommands, reusing the arg structs

The tempting "put `mre` in front of any subcommand" (transparent argv
passthrough) is **deceptively** simpler. Packaging requires _resolving_ the job
— loading and validating the fit.toml, resolving the model, enumerating data
paths — which is the front half of the real dispatch. A generic wrapper still
needs per-command input-collection logic, because the three commands collect
inputs from structurally different places: `fit run` is **config-driven** (paths
in fit.toml), while `simulate`/`pfilter` are **flag-driven** (tables/params/data
on the CLI). A uniform front door hides that difference; it does not remove it.

So v1 is a dedicated command with explicit per-source subcommands that **reuse
the existing typed arg structs and resolvers** — clap validation and config
validation come for free, not reimplemented:

```
camdl mre fit      <fit.toml>           [-b BUNDLE] [--no-data] [--verify]
camdl mre simulate <model.camdl> [sim flags...] [-b BUNDLE] [--no-data]
# camdl mre profile ...   — later
```

- `mre fit` takes `FitRunArgs.config` (the same path `fit run` takes), runs the
  existing `FitConfigV2` load + `validate()` + `DataSpec::validate()`, and
  collects inputs from the resolved config + the model depfile.
- `mre simulate` `#[command(flatten)]`s the real `SimulateArgs`, so every
  simulate flag parses and validates **identically** to `camdl simulate`; inputs
  come from the model depfile + `--table` + `--params`/`--param-vec` + `--draws`
  (when a path, not the literal `uniform`/`prior`) + `--fit`. (Forward sim has
  no observed data: `--data` lives on pfilter/profile, not simulate.)

**clap-composition footgun (must heed).** `SimulateArgs.output` is
`#[arg(short, long)]` (`args/mod.rs:507`) — it already owns **`-o`/`--output`**.
So `mre`'s bundle-output flag **must not be `-o`**; use `-b`/`--bundle` (above).
A second `-o` in the flattened command graph is a clap _parser-construction
panic_ (`debug_assert`, "short option names must be unique") that crashes the
binary on _any_ invocation — it passes `cargo build` and unrelated tests, then
detonates at startup. `FitRunArgs` owns no short flag, so `mre fit` is clean
either way, but the asymmetry is a trap. v1 adds a
`Cli::command().debug_assert()` smoke test so a future flag clash fails loudly
in CI, not in a user's terminal. (Note the residual UX wrinkle: under
`mre simulate`, the flattened `-o` still means simulate's trajectory-mirror
path; the bundle is `-b`. The README the bundle ships documents the exact
reproduce command, so this never reaches the maintainer.)

This gives the "feels like the real subcommand" ergonomics (the arg struct _is_
the real one) without argv-double-parsing, and the front door is honest:
`mre
fit` vs `mre simulate` because they genuinely differ. A passthrough sugar
(`camdl mre <rest>` dispatching to the right collector on the leading token) is
a clean **later** addition once the per-source collectors exist — explicitly out
of scope for v1, per "simple first go."

Yes to reusing existing arg-checking throughout: the bundler validates the job
is _runnable_ before packaging, so a malformed fit.toml fails at pack time with
the real diagnostic, not when the maintainer opens the bundle.

### The collector seam

One per-command function over the already-resolved job:

```rust
/// What the bundle copies. Roles drive the on-disk layout and the consent banner.
enum InputRole { Model, ReadClosure, Data, Holdout, FixedParams, TrueParams,
                 Table, ParamVec, InitSource }

struct InputRef { role: InputRole, src: PathBuf, /* bundle-relative dest */ dest: String }

fn collect_inputs_fit(cfg: &FitConfigV2, args: &FitRunArgs, deps: &DepFile) -> Vec<InputRef>;
fn collect_inputs_sim(args: &SimulateArgs, deps: &DepFile) -> Vec<InputRef>;
```

The enumeration must be **exhaustive** — a missed file is a silently
non-reproducing bundle, the one failure the tool exists to prevent. The full set
per command (each verified against the arg/config structs):

- **`fit`** — `[model].camdl` + its depfile `ReadClosure`; `[data].file` **and**
  every value of the `[data.observations]` / `[data.holdout]` per-stream maps
  (both are `IndexMap<String,String>`, `config_v2.rs:209,220` — not a single
  file); `[fixed].from_file`; `[synthetic].true_params`; **every stage's init
  source**, which is per-stage and file-bearing in four shapes (`fit/init.rs`):
  `FromParams{path}`, `FromMle{File|FitDir}`, `FromPosterior{DrawsTsv|FitDir}`,
  and `StartsFrom::Directory`; plus the `FitRunArgs` companion flags that
  override them (`--params`, `--mle`, `--posterior`, `--survey-path`).
  `*FitDir`/`Directory`/`survey_path` point at upstream CAS dirs (recursion
  question below).
- **`simulate`** — `[model].camdl` + depfile; `--table`; `--params`
  (`Vec<PathBuf>`) and `--param-vec` (`PREFIX=FILE`, `args/mod.rs:453`);
  `--draws` _when it is a path_ (the literals `uniform`/`prior` are keywords,
  not files — `main.rs:887`); and `--fit` (the fit.toml consumed under
  `--draws prior`, `main.rs:620`). **No `--data`** — that flag is on
  pfilter/profile (`Vec<DataSpec>`), not simulate.

`survey_path` and the `*FitDir`/`Directory` init sources are per-stage upstream
CAS dirs. v1 **warns and skips** them rather than recursively bundling a CAS
tree — but the warning must be honest about a real asymmetry: for **Bayesian
stages (PGAS/PMMH)** the chain start does not change the stationary posterior
(`config_v2.rs:893`), so a skipped survey/posterior seed is harmless; for **MLE
stages (IF2/NLopt)** the start selects the basin θ̂ lands in, so dropping
`init = survey_top_k` (→ `lhs`) can move θ̂ and the _numerical_ symptom may not
reproduce. So: skip-with-loud-warning for Bayesian stages, and for MLE stages
either bundle the landscape (`landscape.tsv` + `run.json`) or warn that the MLE
may shift. Open question below.

## Part 3 — Bundle format

### A `.tar.gz` tarball

The bundle is a single `<slug>.mre.tar.gz` — the universal interchange format
for something a reporter emails or attaches to a GitHub issue. None of `tar` /
`flate2` / `zip` is a workspace dependency today, so we add two: **`tar`** (the
de-facto Rust tar crate) + **`flate2`** (gzip; both are alexcrichton crates,
among the most-downloaded on crates.io, pure-Rust-capable, tiny transitive
trees). `camdl mre run` reverses it (untar to a temp dir). If Windows-side
sharing ever matters more than Unix convention, `zip` is the drop-in
alternative; `.tar.gz` is the right default for a scientific Unix audience.

Internally the bundle is the directory tree below; the tarball is just its
serialization, so it stays equally inspectable (`tar tzf` / extract):

```
he2010-bug.mre/
  manifest.toml      # versions, entry command, consent stamp, origin run_id, symptom
  fit.toml           # rewritten to bundle-relative paths (or argv for simulate)
  model.camdl
  inputs/            # data, holdout, fixed-params, true_params, read()-closure, --table files
  README.md          # auto-generated: reported symptom + exact reproduce command
  expected/          # OPTIONAL: the buggy output the reporter saw (obs.tsv / fit summary / stderr)
```

**Path rewriting.** The reporter's `fit.toml` points at files by _their_ paths
(`data = "../shared/cases.tsv"`, or an absolute `/home/reporter/…`). Those won't
exist on the maintainer's machine, so the copy of `fit.toml` _inside_ the bundle
must point at the bundled copies. The clean rule: **preserve the model-relative
layout** when copying (a `read("data/contact.tsv")` lands at
`inputs/data/contact.tsv`), so paths that were already relative-and-contained
resolve unchanged and need **no** rewriting; only absolute paths or ones that
escape the model dir with `../` get rewritten to their `inputs/…` location.
camdlc already resolves `read()` paths relative to the model dir
(`resolve_data_path`, `expander.ml:284`) and `camdl` resolves fit.toml paths
relative to the fit.toml (`resolve_config_path`, `util.rs:361`), so reproducing
that layout is what makes the bundle relocatable. The round-trip `run_id` test
(Testing plan) is the guard that the rewrite was faithful.

An absolute `read()` path is a portability smell in its own right (it makes the
_model_ non-reproducible, independent of MRE). The upstream fix — an
expander-time warning at `resolve_data_path`, where the `Filename.is_relative`
branch already lives — is filed as gh#211; `camdl mre` additionally surfaces the
smell at pack time when it has to rewrite such a path. (It must be an expander
warning, not an `ir/lint.ml` lint: the `read()` path is absent from the IR.)

The manifest is the load-bearing artifact:

```rust
struct MreManifest {
    schema_version: u32,
    /// camdl engine + camdlc hash that PACKED the bundle.
    packed_by: ToolVersions,
    /// engine + camdlc hash that HIT the bug (from the failing run's run.json
    /// when packing from a CAS run; else == packed_by). Lets the maintainer
    /// detect "you're not even on the same binary" before debugging.
    origin: ToolVersions,
    /// Fit { toml } | Simulate { argv } — the exact command to reproduce.
    entry: EntryCommand,
    inputs: Vec<BundledInput>,    // dest path, role, byte len, sha256
    data_consent: DataConsent,
    /// CAS identity of the failing run, when known.
    origin_run_id: Option<String>,
    /// Free-text the reporter pasted ("posterior for alpha == prior").
    symptom: Option<String>,
}

enum DataConsent {
    Included { files: Vec<DataFileInventory> },  // name, rows, sha256
    Excluded,                                      // structure-only (schema, no values)
    Synthetic { seed: u64 },                       // future
}
```

Stamping `origin` + `origin_run_id` is free leverage given the existing CAS
identity layer and the recurring camdlc-version-guard pain: the bundle records
the exact `(compiler_hash, engine_version)` that produced the bug.

## Part 4 — Data consent (explicit and loud)

The reporter is sharing real observed data; that must be impossible to do
silently. As proposed (default subject to Open question 5),
`camdl mre fit
fit.toml` **includes** data (a no-data bundle usually cannot
reproduce an inference bug) and:

- prints a prominent banner — _"⚠ This bundle contains observed data: cases.tsv
  (142 rows), holdout.tsv (20 rows). Share only with the maintainer."_
- records a per-file inventory (name, rows, sha256) under
  `DataConsent::Included`.

`--no-data` produces a **structure-only** bundle: model + config + data _schema_
(column names, row count, time range, dtypes) but no values — for structural
bugs where the data is sensitive. Many engine-class issues (gh#198, gh#199,
gh#208, gh#202, gh#207) reproduce from structure alone.

**Decision to confirm — opt-out vs opt-in (default matters for real PHI-adjacent
data).** The default above is _opt-out_: a bare `camdl mre fit` includes the
data and warns. The counter-case is strong and worth weighing: the banner fires
_after_ the bytes are staged and hashed; an agent or CI loop driving `camdl mre`
suppresses exactly the warning channel consent rides on (the same reason this
proposal prefers hard errors elsewhere); and a `.mre.tar.gz` is built to be
emailed and issue-attached, so the failure mode of opt-out is a one-drag PHI
leak while the failure mode of opt-in is one re-run with a flag — asymmetric
harm favouring the safe default. The opt-in alternative: a bare `camdl mre`
emits the **structure-only** bundle and prints "add `--include-data` to include
observed values"; `--include-data` _is_ the deliberate consent token (honored
without a prompt off-TTY, since the flag is the act), with an optional one-shot
TTY confirm. The banner + inventory stay either way — as confirmation of a
chosen inclusion rather than as the consent mechanism. This contradicts the
original "include by default + flag" framing, so it is **flagged for the
maintainer to decide** (Open question 5), not silently flipped.

## Part 5 — Verification (`--verify`), phased

Packing assembles the bundle's files in a _staging_ dir (the working tree that
gets tarred). The subtlety: `--verify` must **not** run the fit from the staging
dir — that dir sits next to the reporter's originals, so a forgotten file could
be silently picked up from its original location and verification would pass
vacuously. Instead `camdl mre fit fit.toml --verify` finishes the tarball, then
**untars it into a fresh temp dir with no path back to the originals and runs
the fit there**, exactly as the maintainer will. It records, in the manifest,
the resulting `run_id` plus a one-line observable (final loglik / θ̂, or the
error text for a crash). The payoff is large — it eliminates "works on my
machine" bundles by proving self-containment _and_ symptom-reproduction before
the bundle is sent — but it is the most scope (a pack-time run + the
untar-and-run harness). Open question below on whether it lands in v1 or v1.1.

**Output-root isolation (a real hole).** Untarring to a temp dir is not enough
on its own. A fit's CAS output root resolves CLI > `[output_dir]` >
`CAMDL_OUTPUT_DIR`

> `./results` (`run_paths.rs`), so a bundled `fit.toml` carrying an **absolute**
> `output_dir` would write results _outside_ the sandbox — escaping the
> isolation and polluting the runner's store. The pack step must therefore
> neutralize `[output_dir]` in the bundled `fit.toml` (drop it, or make it
> bundle-relative), and the verify/`run` harness sets
> `CAMDL_OUTPUT_DIR=<tempdir>/results` so output is contained regardless. This
> is the same class as path-rewriting (Part 3): an absolute path in the config
> defeats relocation.

The maintainer-side counterpart is `camdl mre run <bundle.tar.gz>`: untar to a
temp dir, run the recorded `entry`, surface the observable. It _is_ the back
half of `--verify`, so the two land together.

## Part 6 — Future: synthetic shape-matched data

A future `--synthetic` populates `DataConsent::Synthetic`, replacing observed
values with random data of the same shape (same times × strata, same column
schema, plausible magnitudes). The **honest boundary**, forced by the identity
model: data is content-hashed (`DataDigest(ContentHash)`, `inputs.rs:84-88`), so
a synthetic swap **changes the `run_id`**. A synthetic bundle therefore
reproduces the _code path and shape_, **not** the _numerical symptom_. That
makes it useful for crashes / RAM / structural bugs and **useless** for
silent-wrong- posterior bugs (gh#186, gh#197) where the specific values drive
the symptom. The synthetic README must say so; we should not let a synthetic
bundle masquerade as a numerical repro. Explicitly out of scope for the first
cut.

## Scope summary

**v1 (this proposal):**

1. `camdlc --emit-deps FILE` — one chokepoint accumulator + one flag + JSON
   sidecar. No IR change.
2. `camdl mre fit <fit.toml>` and `camdl mre simulate <model> ...` — custom
   command, per-source subcommands reusing `FitRunArgs`/`SimulateArgs` +
   existing config validation.
3. Bundle `.tar.gz` (`tar` + `flate2`) + `MreManifest` + auto-generated README.
4. Data consent: loud banner + inventory + `--no-data` structure-only. Default
   (opt-out include vs opt-in `--include-data`) is Open question 5.

**Deferred:** `--verify` + `camdl mre run` (if not v1); `mre profile`;
passthrough sugar (`camdl mre <subcommand> …`); recursive `survey_path`
bundling; `--synthetic`; `.zip` output.

## Lift estimate

Roughly a **2–4 day** lift for v1 (bundle, no `--verify`/`run`), then **+1–2
days** for `--verify` + `mre run`. By component:

- **camdlc `--emit-deps` — small (~half a day).** The expander context is
  already threaded out of the compile boundary (`compiler.ml:114` binds
  `Ok (model, ctx, summary)` from `Expander.expand_detail`, and exposes ctx
  accessors like `Expander.transition_loc ctx`). So this is: one `reads` field
  on the ctx, one push at the `read_csv_rows` chokepoint (`expander.ml:306`),
  one accessor, and the `--emit-deps` flag in `camdlc.ml` writing a Yojson
  sidecar. Return-shape resolved (Part 1): a `compile_with_reads` sibling leaves
  `compile` byte-identical, so the ~60 test callers and the IR output are
  untouched — only `camdlc.ml:187` switches over. No `.mli` churn (none exist in
  `ocaml/lib/compiler/`). No new machinery.
- **`camdl mre` command + collectors — medium (~1–1.5 days).** New `Mre`
  subcommand enum; `mre simulate` `#[command(flatten)]`s `SimulateArgs` (bundle
  flag is `-b`, not `-o` — see the clap footgun above), `mre fit` takes the
  `FitRunArgs.config` path; `collect_inputs_fit`/`_sim` reuse the existing
  config load + path resolvers (`util.rs:361` `resolve_config_path`). The fiddly
  part is the **exhaustive** enumeration (Part 2): the fit side spans `[data]`'s
  per-stream maps, `[fixed]`/`[synthetic]`, and **every stage's** init source
  (`FromParams`/`FromMle`/`FromPosterior`/`Directory`) plus the `FitRunArgs`
  companion flags; the simulate side spans `--table`/`--params`/`--param-vec`/
  `--draws`(path)/`--fit`. A missed field is a non-reproducing bundle, so the
  round-trip test (Testing plan) gates it.
- **Bundle writer + manifest — medium (~1 day).** Copy the closure into the
  staging dir preserving the model-relative layout, rewrite only the
  absolute/`../`-escaping fit.toml paths (the one genuinely careful bit), write
  `manifest.toml` (serde), generate the README, `tar` + `flate2` it. Two small
  new deps.
- **Consent + `--no-data` schema — small (~half a day).** Row counts + sha256
  for the inventory; column/time-range extraction for the structure-only form.
- **`--verify` + `mre run` — medium (~1–2 days), deferred.** Stage → run the
  _unpacked bundle_ as a black box (shell out to this same binary's `fit run` /
  `simulate`) in a temp dir → capture the observable → record in the manifest.
  The care is in running the materialized bundle (not the in-memory staging) so
  the isolation is real.

The one remaining risk is contained: fit.toml path-rewriting (mitigated by the
round-trip `run_id` test, which fails loudly if a path is mis-rewritten or a
file is missed). The OCaml compile-return-shape question is resolved (Part 1).

## Open questions

1. **`--verify` in v1 or v1.1?** It is the single highest-value add (kills
   non-reproducing bundles) but the most scope (a pack-time run). Lean: ship the
   bundle in v1, add `--verify` immediately after once the format is stable.
2. **From-CAS-run entry?** A `camdl mre <run-dir>` that recovers input paths
   from the failing run's `run.json` provenance (`RunProvenance.argv`,
   `inputs.rs:218`) and repackages. Convenient, but fragile (input files must
   still exist at their original paths). Lean: config-first for v1, run-dir form
   later.
3. **Upstream CAS-dir seeds (`survey_path`, `*FitDir`, `Directory`).**
   Warn-and-skip (require a self-contained fit) for v1, or bundle the upstream
   CAS dir? Lean: warn-and-skip — but with the MLE/Bayesian asymmetry stated
   (Part 2): harmless for PGAS/PMMH, can move θ̂ for IF2/NLopt.
4. **Depfile and the IR cache.** When the model compile is cache-hit (`util.rs`
   ir_cache), there is no fresh compile to attach `--emit-deps` to. v1 can force
   a compile under `mre` (correctness over speed; MRE packing is not hot), or
   cache the depfile next to the cached IR. Lean: force-compile in v1; cache
   later if it bites.
5. **Consent default — opt-out or opt-in?** Keep the original default-include
   (`--no-data` to exclude), or flip to default structure-only with
   `--include-data` as the explicit consent token (Part 4)? The opt-in case is
   safety-driven (a `.mre.tar.gz` is one drag from a public tracker; the banner
   can't un-write the bytes; agents suppress warnings). The opt-out case is
   ergonomic (most inference bugs need the data; one fewer flag). Maintainer's
   call — this is the one decision that touches how a user's real surveillance
   data leaves their machine.

## Testing plan

- **camdlc `--emit-deps`**: a fixture model that `read()`s two files → assert
  the depfile lists exactly those two (relative + resolved), and that a model
  with no reads emits `"reads": []`. Negative control: a model whose read path
  is missing still errors via E200 (the depfile is only written on a
  _successful_ compile).
- **collector**: golden test — `mre fit` over a fixture fit.toml with data +
  holdout + fixed-file + a `read()`-ing model → assert the `InputRef` set (by
  role) is exactly the closure; assert `--no-data` drops the Data/Holdout roles
  and emits the schema instead.
- **round-trip**: pack a known-good fit → `camdl mre run` (or manual unpack) →
  assert it produces the _same_ `run_id` as the original. This is the
  load-bearing test: it proves the **data-included** closure is
  identity-faithful (a missing table changes the IR digest → the `run_id`
  diverges). It does **not** exercise the `--no-data` path — cover that
  separately by asserting the structure-only bundle carries the schema and no
  values.
- **clap surface**: a `Cli::command().debug_assert()` smoke test, so a future
  `mre`-level flag that clashes with a flattened struct's short/long name (the
  `-o` class of bug) fails in CI at parser construction, not in a user's
  terminal.
- **consent**: assert the banner fires and the manifest records the inventory
  whenever data is included; assert `DataConsent::Excluded` carries no byte
  contents.

Integration (cross-language, the `tests/test_ocaml_to_rust.sh` harness — the
real camdlc + the real `camdl`, since `--emit-deps` and the round-trip both span
the OCaml→Rust boundary):

- **`mre run` round-trip (the load-bearing one)**: pack a fixture fit whose
  model `read()`s a contact table → `camdl mre run` the bundle from a **scratch
  dir with no path back to the originals** → assert it (a) succeeds and (b)
  yields the **same `run_id`** as the original in-tree fit. Self-containment
  _and_ closure completeness in one assertion: a dropped table or a
  mis-rewritten path either fails the run outright or shifts the IR digest, and
  the `run_id` diverges. The negative control is the diagnostic: delete one
  `inputs/` file from the packed bundle, re-run, assert a hard failure (never a
  silent fall-through to the original copy).
- **`--verify` records a faithful observable**: pack with `--verify` → assert
  the manifest's recorded observable (final loglik / θ̂, or error text) matches a
  direct in-tree run of the same fit, and that the recorded `run_id` equals the
  in-tree one. A crash fixture (e.g. a model that trips a known engine guard)
  asserts the error text is captured rather than swallowed.
- **`--verify` runs the materialized bundle, not the staging**: a guard test
  that removes a staged file _before_ the verify step still fails verification —
  proving `--verify` exercises the unpacked artifact, not the in-memory closure
  (the vacuous-pass trap from Part 5).
