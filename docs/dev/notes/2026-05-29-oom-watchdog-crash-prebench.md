# Dev-machine OOM watchdog panic while benchmarking the pre-Fix-B compile

Date: 2026-05-29 Project: camdl Tags: memory, oom, watchdog-panic, fix-b,
scaling-bench, perf/ir-bindings

## Context / question

A coding-agent session on `perf/ir-bindings` was finalizing the Fix-B
before/after scaling figure when the maintainer's machine (M4 Max, 48 GB, macOS
26.3.1 / Darwin 25.3.0) froze and hard-rebooted. Question: was this a
memory-exhaustion crash caused by the benchmark, or something else — and did it
leave the branch in a bad state?

This note records the investigation. The forward-looking remedy is a separate
RFC:
[`docs/dev/proposals/2026-05-29-compiler-memory-guardrail.md`](../proposals/2026-05-29-compiler-memory-guardrail.md).

## Finding (one line)

A **kernel watchdog panic driven by memory exhaustion**, triggered by
benchmarking the **pre-Fix-B "inlined" compile of a Kano-scale spatial model**
(peak RSS ≈ 15.6 GB) — i.e. the _old_ O(P²·A²) IR path, exercised on purpose for
the before/after figure. The branch code is intact, builds, and is fully tested;
nothing was corrupted.

## Evidence — verified

### 1. The machine watchdog-panicked at 18:16, and the cause was memory

```
$ sysctl -n kern.boottime
{ sec = 1780103788 ... } Fri May 29 18:16:28 2026     # rebooted 18:16; prior boot was Apr 22

$ tr ',' '\n' < /Library/Logs/DiagnosticReports/.contents.panic | grep -iE "panic_string|compressor"
"panic_string":"panic(cpu 6 ...): watchdog timeout: no checkins from watchdogd in 90 seconds ...
... Compressor Info: 73% of compressed pages limit (OK) and 100% of segments limit (BAD)
    with 48 swapfiles and OK swap space ...
** Stackshot Incomplete ** Bytes Filled 240, err 52
```

- **Proximate cause:** watchdog timeout — the system made no progress for 90 s,
  so the kernel force-panicked (the visible "freeze + reboot").
- **Root cause: memory exhaustion.** `100% of segments limit (BAD)` = the VM
  compressor's segment table was full; macOS had spun up **48 dynamic
  swapfiles**. The kernel does not do that absent severe memory reclaim
  pressure. A pure driver/CPU deadlock would not max the compressor or spawn 48
  swapfiles.
- **Honest caveat:** the panic's stackshot is **truncated**
  (`** Stackshot Incomplete **`), so the kernel log does _not_ itself name the
  offending process. Attribution to camdl below is from the agent transcript +
  timing, not from the panic naming it. (Inference, clearly marked.)

A separate, earlier jetsam memory-pressure event
(`JetsamEvent-2026-05-29-030828.ips`, 03:08) shows this machine was already
running near its memory ceiling overnight.

### 2. What ran at crash time — the trigger (from the session transcript)

The agent transcript (`~/.claude/projects/.../96c7fbf3-….jsonl`, last write
**18:13**, 3 min before the 18:16 panic) shows it building the Fix-B figure by
compiling/simulating a **pre-Fix-B (inlined) Kano LGA SEIRV model** (≈44 LGAs ×
21 ages). Verbatim from the transcript:

> "Kano-before is still compiling the **2.6 GB inlined IR** (then ~45 s
> simulate)."

and the before/after anchor it had just measured (committed to the working TSVs,
`realism / P=44 / A=21 / coupling=on / full`):

```
# scaling_before_b.tsv (pre-Fix-B: E+D, no B)
realism  44  21  on  full  3696  2772  3708232034  38.72  17.68  15628.6   # ir≈3708 MB, peak RSS 15.6 GB
# scaling_after.tsv (post-Fix-B: E+D+B)
realism  44  21  on  full  3696  2772  1069650672  12.12   2.57   3000.1   # ir≈1070 MB, peak RSS 3.0 GB
```

So the crash trigger was heavy **pre-B** compiles in the ~15 GB-RSS class run
back-to-back (the P=44 scaling point at 15.6 GB, then the real Kano LGA compile
at ~2.6 GB IR), on a 48 GB machine already holding ~10+ GB of resident apps
(Fusion, Preview, Chrome, Spotify). This is the _old_ code path, exercised
deliberately to produce the "before" baseline — not the production path.

### 3. Fix B is exactly the mitigation, and it largely works

At the Kano anchor (P=44, A=21), Fix-B shared-binding extraction gives:

| metric        | before (inlined) | after (hoisted) | factor |
| ------------- | ---------------- | --------------- | ------ |
| IR size       | 3708 MB          | 1070 MB         | 3.5×   |
| peak RSS      | 15.6 GB          | 3.0 GB          | 5.2×   |
| simulate wall | 17.7 s           | 2.6 s           | 6.9×   |

Honest scope: log-log slopes stay ≈2 before _and_ after (1.98→1.92) — Fix B is a
large **constant-factor** win, not asymptotic. The residual O(P²) lives in
`rate_grad` (the deferred B2). Figure:
`docs/dev/notes/assets/scaling/fix_b_before_after.png`.

## Branch state after the crash — verified

Working tree clean (all code committed; crash left no half-written tracked
files). Verification run post-reboot:

- `cd ocaml && dune build && dune runtest` → green.
- `cd rust && cargo test --workspace --no-fail-fast` → **1254 passed, 7 ignored,
  22 failed**. The proposal's primary gate `gate_trajectory_baseline` and all
  golden/sim/backend tests pass.
- Goldens valid: `make update-ocaml-golden` then
  `git status --porcelain
  ocaml/golden ir/golden` → **empty** (committed
  goldens byte-for-byte match HEAD's compiler).

### The 22 failures were a stale-install artifact, not a code bug

All 22 were `cli` acceptance tests failing with `error: camdlc version
mismatch`
(`rust/crates/cli/src/util.rs:150`). The check compares git hashes of `camdl`
and the `camdlc` it invokes. The fresh release `camdl` (HEAD, built 17:58) was
resolving the **stale globally-installed `camdlc`** on PATH
(`~/.local/bin/camdlc`, built **May 28 21:30**, older commit) — the agent
rebuilt `camdl` but never re-ran `make install`. Because the mismatch path calls
`std::process::exit(1)`, it aborts the whole test binary (hence all-or-nothing
counts, e.g. 10/10).

Proven benign two ways:

1. Pointed at the fresh in-tree `camdlc` with the bypass env →
   `CAMDLC=…/camdlc.exe CAMDL_SKIP_VERSION_CHECK=1 cargo test -p cli …` → **22
   passed, 0 failed**.
2. After `make install` (syncs `~/.local/bin/camdlc` to HEAD `55f0d58`), the
   same 22 run **plain** (no bypass) → **22 passed, 0 failed**.

Secondary observation: the 22 tests omit the `CAMDL_SKIP_VERSION_CHECK=1` guard
that their ~dozen passing siblings set — a test-hygiene gap that makes them
fragile to a stale install. Tracked separately from this crash.

## Next

- Memory guardrail so a too-big model cannot take down the host — see the RFC
  [`docs/dev/proposals/2026-05-29-compiler-memory-guardrail.md`](../proposals/2026-05-29-compiler-memory-guardrail.md).
- (Optional) harden the 22 version-check-sensitive tests with the bypass env or
  by pinning `CAMDLC` to the in-tree binary.
- (Housekeeping) `~/.local/bin` holds ~140 `camdlc-<hash>` binaries (~480 MB);
  the `install` target writes one per build and never prunes.
