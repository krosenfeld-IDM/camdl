---
status: draft
date: 2026-05-29
title: A memory guardrail so an oversized model cannot OOM the host
author: internal
scope: ocaml/lib/compiler/expander.ml, ocaml/lib/ir/serde.ml, scripts/bench_scaling.py, Makefile (bench-* targets)
motivates: docs/dev/notes/2026-05-29-oom-watchdog-crash-prebench.md
relates:
  - docs/dev/proposals/2026-05-29-shared-bindings-and-reduction.md (Fix B/D — the structural fix that shrinks the IR)
non-goals:
  - sparse spatial kernels / top-k neighbours (separate; changes the model)
  - replacing Fix B — this is a *safety net*, not the size fix
---

# A memory guardrail so an oversized model cannot OOM the host

## Problem (verified)

On 2026-05-29 a benchmark of the **pre-Fix-B inlined compile** of a Kano-scale
spatial model (≈44 LGAs × 21 ages) drove the dev machine into a
memory-exhaustion **kernel watchdog panic** (full reboot). Evidence and
timeline:
[`docs/dev/notes/2026-05-29-oom-watchdog-crash-prebench.md`](../notes/2026-05-29-oom-watchdog-crash-prebench.md).

Root mechanism: the expander materializes a **fully-inlined flat IR** — every
transition carries its own copy of the FOI rate tree — so IR size grows ≈
`n_transitions × (mean rate-expr node count)`. At P=44 that was 2772 transitions
and a **3.7 GB IR / 15.6 GB peak RSS** (committed `scaling_before_b.tsv`).
Nothing in the pipeline bounds this: the compiler will happily try to build an
IR larger than host RAM, and because macOS thrashes the VM compressor before
jetsam can act, the _whole machine_ dies rather than the offending process.

Fix B (shared bindings, landed on `perf/ir-bindings`) cuts this ~3.5×/~5× — but
(a) it is a constant-factor win, so a large-enough P still blows up, and (b) we
must keep _measuring_ the pre-B baseline, which is exactly the path that
crashes. We need a safety net independent of the structural fix.

This is two distinct audiences with two distinct remedies:

- **Users** (the health-ministry modeler) who accidentally write a model that
  expands huge — want a _clear compiler error before_ the blowup, not a frozen
  laptop.
- **Devs** benchmarking the regression baseline — want the heavy run to
  _OOM-kill the process_, not panic the kernel.

## Options

### A. Compiler preflight size estimate → hard error with hint (for users)

In `expander.ml`, after the flat transition list and rate trees exist but
**before** serialization, compute a cheap upper-bound estimate:

```
est_nodes ≈ Σ_transitions (node_count(rate) + node_count(rate_grad))
est_bytes ≈ est_nodes × bytes_per_node   (calibrate vs committed scaling TSVs)
```

If `est_bytes` exceeds a threshold (default e.g. 2 GB, env/flag-overridable),
emit a real diagnostic (error code + source-free model-level location) of the
form:

```
error[E6xx]: model expands to an estimated ~3.7 GB of IR (44 patches × 21 ages,
             2772 transitions). This will likely exhaust memory.
  = hint: spatial FOI inlining grows as patches²; compile with shared bindings
          (default on ≥0.6), reduce strata, or pass --allow-huge-ir to override.
```

Aligns with the project's "hard errors with hint text over warnings" and
"human-first DSL" principles. The estimate is deterministic and adds negligible
time (one pass over an AST already in hand).

**Implementer must verify (per CLAUDE.md "read the spec/code first"):** the
exact point in `expander.ml` where the flat transitions exist, and calibrate
`bytes_per_node` against `n_transitions`/`ir_bytes` in
`docs/dev/notes/assets/scaling/scaling_*.tsv` (≈1.3 MB IR per transition in the
inlined path at P=44 — sanity-check this, don't trust it blind).

### B. Resource cap on the benchmark harness (for devs)

Wrap the memory-heavy benchmark drivers so a runaway compile is OOM-killed
rather than taking the host down. Cheapest form: in `scripts/bench_scaling.py`
(and the pre-B baseline path), spawn each `camdlc`/`camdl` subprocess under a
virtual-memory cap, e.g. `ulimit -v <KB>` in the child, sized to leave headroom
(say cap at 24 GB on a 48 GB box). A killed child becomes a recorded "OOM at
P=N" data point instead of a panic. Pure harness change, no engine risk.

### C. Streaming / bounded IR serialization (rejected for now)

Avoid holding the full IR JSON in memory at once. Largely already addressed by
Fix E (untagged-buffering removal) and Fix D (parse cliff); the dominant cost
now is the _materialized AST_, not the JSON string. Out of scope — does not
address the expander-side blowup.

## Recommendation

Do **A + B**: the compiler preflight (A) protects every user and every agent
author with one deterministic check; the harness cap (B) makes it safe to keep
benchmarking the pre-B baseline that prompted this. C is deferred.

Land order, each its own commit, tests first (TDD per CLAUDE.md):

1. **B** first (lowest risk, immediately unblocks safe baseline benchmarking):
   add the `ulimit -v` wrapper + an "OOM at P" record path; manual check that a
   capped pre-B P=44 run is killed, not the machine.
2. **A**: red test = a synthetic high-P fixture that today would expand huge
   asserts the new `E6xx` fires with the size in the message; green = it does;
   then a `--allow-huge-ir` escape-hatch test asserting it compiles through.
   Calibrate the estimate against the committed scaling TSVs.

## Tradeoffs / risks

- **False positives.** A conservative threshold could block a legitimate large
  model on a big-RAM box. Mitigate with `--allow-huge-ir` (and/or scale the
  default to a fraction of detected host RAM). The estimate must be an _upper
  bound_ tuned to warn, not to be exact.
- **Threshold is a policy choice**, not a fact — document it and make it
  overridable; do not hard-code a magic number without the override.
- **B only protects the harness**, not ad-hoc `camdlc` invocations — that is
  what A is for. Neither replaces Fix B; both are nets under it.
