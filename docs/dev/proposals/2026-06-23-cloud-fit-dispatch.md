# Cloud fit dispatch — a lightweight, content-addressed fan-out

Status: **Draft** — initial design sketch. Not scheduled; open questions below
are unresolved. Not a shippable spec.

Author: Vince Buffalo

Related: [`2026-06-09-mre-bundle.md`](2026-06-09-mre-bundle.md) (gh#212, the
bundler),
[`2026-06-22-predictive-ergonomics.md`](2026-06-22-predictive-ergonomics.md)
(`fit predict`, the future paired-output surface).

## Problem

Running real fits today means provisioning a VM by hand, copying a model, data,
and `fit.toml` onto it, running `camdl fit run`, and copying results back —
manually, per fit. A sweep over regions, seeds, or scenarios multiplies that
friction. We want to dispatch fits to ephemeral cloud compute that spins up on
demand and tears down to zero, with the operator running a single command and
collecting a result manifest. The target is **Azure first, cloud-agnostic
eventually** — the storage and execution substrate must not pin us to one
provider's batch API.

The encouraging part: this can be much lighter than a general
distributed-compute harness, because of what a camdl fit _is_ and what camdl's
run-identity already guarantees.

## The shape of the problem: open-loop fan-out, not a closed optimizer loop

A model-calibration harness (e.g. ABM calibration) is typically a **closed
loop**: a central coordinator holds optimizer state, proposes parameters, waits
for simulations, scores them, and re-proposes. Round-trip latency and stateful
coordination dominate, and nothing deduplicates because every round's parameters
are novel.

A camdl fit is **open-loop**. The entire inference loop — IF2's cooling
schedule, PGAS's sweeps, the particle filter — runs _inside one worker process_.
The dispatcher never sees a particle or a parameter proposal; it hands a worker
a self-contained job and waits for a result. Distributing fits is therefore a
**fan-out / fan-in over opaque, long-running jobs** — the simplest distributed
pattern there is. No optimizer state in the middle, no low-latency message bus.

And because a fit is **deterministic** given its seed (per-particle RNGs seeded
from `seed + index`, a compute-count bail with no wall-clock gate, serial
weight/resample), one property collapses most of the infrastructure:

> **At-least-once delivery is sufficient; exactly-once is unnecessary.** A
> duplicated job recomputes the byte-identical artifact under the same
> content-addressed key — a wasteful no-op, never a wrong answer. The queue can
> be dumb, workers can be spot/preemptible, and a reclaimed worker just gets
> redelivered.

## What camdl already provides

The hard part — naming work so it can be deduplicated, resumed, and stored
without collision — is already done by the run-identity (`runid`) crate and the
`mre` bundler. Verified against the code:

- **The run-id closure is honest about data.** A fit's `run_id` hashes data file
  _contents_, not paths (`fit/cas.rs:248` reads the bytes; `runid/inputs.rs`
  `DataDigest`: "Hashed by content, never by path"). A worker that ships the
  bytes gets the correct identity.
- **The binary version is in the key**, so a heterogeneous worker fleet cannot
  collide in a shared store. The engine git-hash and IR schema version are
  folded into every leaf (`ModelDigest { ir, ir_version, engine }`,
  `resolve.rs:97`; engine = `CARGO_PKG_VERSION + "+" + CAMDL_GIT_HASH`). Two
  different builds running the same `fit.toml` produce _different_ `run_id`s and
  land in different leaves — no manual namespacing, no silent cross-version
  contamination.
- **The output leaf path _is_ the run-id.** Results land at
  `results/fits/{stem}-{h8}/{NN-stage}-{h8}/{seed}-{h8}/`
  (`runid/layout.rs:122`), content-addressed on disk. The object-store layout is
  the on-disk layout: "collect results" is a prefix sync, "is this done?" is a
  key-exists check.
- **Per-stage / per-seed factoring with a deps-DAG.** Scout and posterior are
  separate leaves; the posterior's hash folds in the scout's identity. The
  identity model already supports finer-grained jobs when we want them.
- **The bundler exists.** `camdl mre fit <fit.toml>` packages the full input
  closure — model + the `read()` table closure (via `camdlc --emit-deps`) +
  `fit.toml` + data + fixed files — into a self-contained `.tar.gz`, enforcing a
  contained relative layout, with `--no-data` for structure-only. Its manifest
  already records per-file `sha256`, `camdl_version`, and the exact reproduce
  command (`mre.rs:44-63`). Built for bug reports, but the artifact _is_ the
  portable input closure a worker needs.

The one missing piece: there is **no way to ask camdl for a fit's `run_id`
without running it.** `simulate` has `--dry-run`; `fit` does not. This is the
linchpin for cheap deduplication and for building the result manifest up front
(see "Required camdl changes").

## Architecture

Four layers. The object store is the coordination substrate, not just the
archive.

```
           ┌─────────────────────────────────────────────────────┐
dispatcher │ plan: sweep.toml + .camdl + data                     │  (laptop / CI)
 (thin CLI)│   → `camdl mre fit` → content-addressed bundle        │
           │   → `camdl fit plan` → predicted run_id per (job,seed)│
           │   → dedup against store → enqueue only the missing    │
           └───────────────┬─────────────────────────────────────┘
                           │ {bundle_digest, seed}  (tiny payload)
       OCI registry (ORAS) ▼               work queue (at-least-once)
       ┌───────────────────────────┐   ┌──────────────────────────┐
       │ worker image              │   │  visibility-timeout       │
       │ bundles/<digest>          │   │  redelivery on death      │
       └───────────────────────────┘   └─────────┬────────────────┘
       blob store                                │ claim
       ┌───────────────────────────┐             ▼
       │ results/fits/<leaf>/  ◄────┼──── ┌──────────────────────────────┐
       │ (prefix-listable)         │     │ worker (spot): {camdl, camdlc} │ scale-to-zero
       └───────────────────────────┘     │   ~30-line loop, stateless     │
                                         └──────────────────────────────┘
```

### The Job type

The unit of remote work is one `fit.toml` invocation (coarse granularity — see
"Granularity"):

```rust
struct Job {
    bundle_digest: Digest,          // OCI digest of the mre bundle {ir-source, data, fit.toml, read closure}
    seed: u64,
    predicted_run_id: ContentHash,  // from `camdl fit plan` — the output leaf, known before running
    resources: ResourceHint,        // vcpus / mem, derived from chains × particles
}
```

### The worker — the entire "lightweight core"

Stateless, ~30 lines, because camdl does the work:

```
loop:
  msg = queue.claim()                                  # at-least-once
  if none and idle > T: exit                           # scale-to-zero
  if blob.exists(results/.../predicted_run_id/run.json):
      queue.ack(msg); continue                         # CAS skip — already computed
  bundle = oras.pull(msg.bundle_digest) -> /work       # ir-source + data + fit.toml
  camdl fit run /work/fit.toml --seed msg.seed --output-dir /work/out
  blob.put(/work/out/results/**)                       # content-addressed → idempotent overwrite
  queue.ack(msg)
```

### The dispatcher is declarative

Its job is "ensure these `run_id`s exist," not "run these jobs." Re-invoking it
after a partial failure resumes for free — already-present leaves are skipped.
The dispatcher holds almost no state because the store _is_ the state.

```
jobs   = expand(sweep.toml)                       # one Job per (fit.toml, seed)
plan   = camdl fit plan ...                       # predicted run_id per job
missing = [j for j in jobs if not blob.exists(j.predicted_run_id)]
for j in missing: oras.push(mre fit ...)          # idempotent: identical bundle = no-op
enqueue(missing)
wait until every job's predicted_run_id exists in blob
emit manifest: name → run_id → blob path
# compute tears down to zero on its own (scale-to-zero on empty queue)
```

### Keep the cloud out of the camdl binary

`camdl` stays a pure local fit/sim engine. `camdl mre fit` emits a local
artifact (tarball + manifest + a content-addressed bundle digest); the
_dispatcher_ — which already holds cloud credentials — does the `oras push`, the
blob writes, and the queue operations. The seam between them is the bundle + the
registry + the CLI. This keeps the scientific core cloud-free and portable, and
isolates every provider-specific concern to the dispatcher.

## Storage

Two systems, each to its strength.

- **Inputs (bundles) → OCI registry via ORAS.** Decided. Bundles are immutable,
  write-once, content-addressed — a perfect fit for an OCI registry, which is
  digest-addressed natively and which _every_ cloud offers (Azure ACR, AWS ECR,
  GCP GAR, GHCR) speaking the same distribution spec. ORAS is one push/pull
  interface across all of them, so migrating clouds re-points a URL rather than
  rewriting the storage layer — this is the cloud-agnostic substrate. The worker
  image and the bundles live in the same registry, so there is no separate
  bucket for inputs. The bundle's OCI digest serves as the `bundle_digest`.
- **Results → blob store (Azure Blob first).** Decided. Results are a tree we
  want to browse, prefix-list, and sync (`camdl list/show/cat`, "completed vs
  pending"). Registries are tag+digest, not prefix-enumerable, so they are a
  poor home for results. A blob store gives cheap prefix listing and maps
  directly onto camdl's on-disk `results/fits/...` layout.

The registry's weak listing does not hurt the dispatch loop: the dispatcher
drives from _known_ predicted `run_id`s (existence-check, not enumerate).
Listing only matters for ad-hoc browsing of remote results, which the blob store
covers.

## The cloud-agnostic seam: two adapter traits

Everything above the queue and compute is already cloud-neutral (ORAS bundles,
blob results, run-identity, dedup, the worker container). The provider-specific
surface reduces to two small traits:

```rust
trait Queue {
    fn enqueue(&self, job: &Job) -> Result<()>;
    fn claim(&self, timeout: Duration) -> Result<Option<ClaimedJob>>;  // at-least-once + visibility timeout
    fn ack(&self, claim: ClaimedJob) -> Result<()>;
}

trait Compute {
    fn submit(&self, n: usize, image: &ImageRef, res: ResourceHint) -> Result<()>;  // scale workers up
    // scale-to-zero is intrinsic (KEDA on queue depth) — no explicit teardown call
}
```

Azure implementations land first; AWS/GCP are additional impls, not a rewrite.

## Azure-first concrete stack

```
ACR (OCI registry)          worker image  +  mre bundles (ORAS push)
Azure Blob                  results/ (prefix-listable; camdl list works)
Azure Storage Queue / SB    at-least-once work queue; payload {bundle_digest, seed}
ACA Jobs + KEDA             scale-to-zero container workers; scale on queue depth
worker = OCI container {camdl, camdlc} pinned to one git hash
dispatcher = thin CLI: mre fit → oras push → enqueue → wait-on-run_ids → manifest
```

**Azure Container Apps Jobs + KEDA** is preferred over Azure Batch for compute:
it is container-native, scales to zero, and is k8s-shaped, so the eventual
cloud-agnostic move is "run the same OCI worker as a k8s Job" rather than
"rewrite against a different batch API." Azure Batch would tie us to an
Azure-specific surface, against the stated direction.

## Required camdl changes

These are the in-repo deltas; everything else lives in the dispatcher.

1. **`camdl fit plan <fit.toml> [--seed N ...]`** — resolve config → IR →
   predicted `run_id` + leaf path per (stage, seed), running _no_ inference. The
   identity logic already exists in `fit/cas.rs::resolve_fit_stage`; this is a
   CLI surface over it that stops before execution. It still needs `camdlc` +
   the data files present (to compute the IR and data digests), which the
   dispatcher node has. This is the linchpin for dedup and for the up-front
   manifest.
2. **Generalize `mre` into the canonical input-closure producer.** It already
   packs the closure; it needs a single **content-addressed bundle digest** (a
   rollup hash over the canonicalized manifest, whose per-file `sha256`s already
   exist) to serve as the dedup key and ORAS tag. Bug-repro becomes one consumer
   of the same artifact. Keep ORAS _out_ of `mre` — the dispatcher pushes.
3. **Keep `.camdl` source in the bundle (not compiled IR); bake `camdlc` into
   the worker image** (4 MB). The worker compiles on arrival; the
   engine-git-hash in the `run_id` means a `camdlc`/`camdl` mismatch yields a
   _different_ `run_id` (caught, never silently wrong). Source-portability of
   the bundle is worth keeping. The worker image pins matched `camdl` + `camdlc`
   at one git hash, satisfying the version guard without
   `CAMDL_SKIP_VERSION_CHECK`.

## Decisions (locked for this draft)

- **Results on blob, inputs on ORAS/registry.** (See "Storage".)
- **Coarse granularity: one bundle = one `fit.toml`, all stages on one worker.**
  This sidesteps the one bundler gap: `mre` does _not_ yet bundle upstream
  artifact seeds (`init = from_mle / from_posterior / ...`) — it hard-errors
  with guidance (`mre.rs:13-14`), which is exactly the scout→posterior
  dependency. Coarse jobs never hit it. Fine-grained stage-jobs (scout on a
  cheap box, posterior on a big one — which the per-stage `run_id` factoring
  already supports) wait until `mre` learns to bundle an upstream artifact.

## Preemption

A 300-sweep PGAS can run for hours; a spot reclaim mid-run is _correct_ to
re-run (determinism guarantees the same artifact) but wastes the elapsed time.
`--resume` exists for PGAS/PMMH but writes to a _new_ leaf (reading the base
read-only), so it is "continue from a checkpoint" rather than in-place.

- **v1:** re-run on preemption (simple, correct). Splitting a fit into separate
  scout/posterior stage-jobs (once the bundler supports upstream artifacts)
  means a posterior reclaim does not also re-burn the scout.
- **v2:** checkpoint-to-store-and-resume, if the wasted compute proves material.

## Future: inline paired predictive outputs

A fit usually wants a companion predictive artifact — posterior-predictive
checks, a forecast over a horizon — and in a cloud context we especially want
**one job to produce the complete deliverable** rather than a second dispatch
round. The future surface: a `fit.toml` declares its predictive outputs (a
`[predict]` block — horizon, draws, free-forward vs conditioned), and the worker
realizes them inline after the fit, writing predictive artifacts into the _same_
result leaf. This integrates with the `fit predict` verb and the
`FitResult / Horizon / ParamTreatment` types from
[`2026-06-22-predictive-ergonomics.md`](2026-06-22-predictive-ergonomics.md):
rather than a separate `camdl fit predict` invocation, the predictive step
becomes a declared, content-addressed extension of the fit job. Out of scope for
v1; recorded here so the bundle/leaf design leaves room for it.

## Open questions

(Draft-stage; each must be resolved or converted to a tracked follow-up before
this graduates to a scheduled spec.)

1. **Dispatcher language / home.** A small Rust crate in the monorepo (outside
   the inference path), or a separate TS/Python tool? Keeping it out of the
   `camdl` binary is decided; where it lives is not.
2. **Sweep → bundle deduplication.** When a sweep varies only the seed, every
   job shares one bundle — the dispatcher should push it once and enqueue N
   `{bundle_digest, seed}` messages. When a sweep varies data or fixed params,
   bundles differ. Confirm the bundle digest is computed before the push so
   identical bundles collapse.
3. **Credentials & isolation.** How the dispatcher and workers authenticate to
   ACR/Blob/Queue (managed identity vs SP), and whether multi-user runs share a
   store namespace or partition by principal.
4. **Cost controls.** Spot-only vs fallback to on-demand; a per-sweep ceiling on
   concurrent workers; an idle-timeout for scale-to-zero.
5. **Result retrieval UX.** Does the operator pull results back locally, or does
   `camdl list/show/cat` learn to read a remote blob store directly (a
   `--store
   az://...` backend)?
6. **`fit plan` and engine-version coupling.** `fit plan` on the dispatcher must
   produce the same `run_id` the worker will — i.e. the dispatcher's `camdl`
   must match the worker image's git hash. Pin both from one release artifact;
   spell out how.
