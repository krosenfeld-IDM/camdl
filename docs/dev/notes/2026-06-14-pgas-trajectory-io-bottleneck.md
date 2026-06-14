# PGAS fit was I/O-bound, not compute-bound: unbuffered trajectory dump

Date: 2026-06-14\
Project: camdl\
Tags: profiling, pgas, inference, io, samply

Implemented: 3d905488 (`perf(fit): buffer PGAS trajectory-sample writes`)

## Context / question

Continuing the PGAS performance work (sibling to
[`2026-06-13-pgas-parallelism-and-scaling.md`](2026-06-13-pgas-parallelism-and-scaling.md),
the gh#209 CSMC-parallelism study). Question: where does a PGAS fit actually
spend its wall time, and how much faster can it go?

## Finding (headline)

**60% of a PGAS fit's wall time was a single unbuffered file write, unrelated to
inference.** The per-sweep latent-trajectory dump in `cli/fit/pgas.rs`
(`run_stage`) wrote each field of a ~1455-substep × 723-column TSV with its own
`write!` to a raw `std::fs::File` — ~1.05M `write()` syscalls per file, ~10.5M
across a 10-sweep fit. Wrapping the file in a `BufWriter` collapses that to a
few hundred syscalls per file.

Measured (polio 12-patch metapop: 145 compartments, 577 transitions, 1455
substeps; 80 particles, 10 sweeps, `--parallel 16`, M4 Max):

|        | wall      | sweep-loop breakdown                                    |
| ------ | --------- | ------------------------------------------------------- |
| before | 40.1s     | csmc 37% · gradient 1% · **trajectory-I/O 60% = 24.5s** |
| after  | **15.6s** | csmc 94% · gradient 2% · trajectory-I/O **0.15s**       |

**2.57× on the whole fit.** Output bytes are identical (verified: 10 files, 723
cols × 1456 rows, complete to t=2910). The fix is now legitimately csmc-bound,
as a PGAS fit should be.

This scales _up_ with model size: cost is
`substeps × (compartments +
transitions) × sweeps`. A 244-ward national model
(gh#207/#209) would be far worse, so the fix matters most at the scale we care
about.

## Three plausible-but-wrong candidates ruled out first

The bottleneck was hidden behind a wrong assumption — that the NUTS **gradient**
dominated (inferred from `rest = total − csmc ≈ 65%`, never measured directly).
Acting on that assumption, three levers were built/tested and all failed,
because they targeted a 1% slice:

| candidate                 | result    | why it's not the lever                     |
| ------------------------- | --------- | ------------------------------------------ |
| gradient binding cache    | 1.01×     | gradient cost isn't repeated binding evals |
| parallel-substep gradient | 1.15×     | gradient is ~1% of the loop, not 65%       |
| mimalloc (vs libmalloc)   | no change | not allocator-bound or contention-bound    |

A counting global allocator showed the gradient was **13%** of allocations (not
allocation-bound). Direct timing then showed the gradient was **1% of the loop,
called 20 times in 10 sweeps** — NUTS diverges immediately at the bench's
(infeasible, burn-in=0) start and barely touches it. The thing assumed to be the
bottleneck was 1%.

## How the real bottleneck was found (method, reusable)

1. **Counting global allocator** (env-gated `#[global_allocator]`, counter as a
   `pub static` in `sim` so `pgas.rs` can snapshot it around csmc vs the rest):
   gradient 13% of allocs, csmc 87%. Killed the allocation-bound hypothesis.
   _Gotcha:_ read the env flag once in `main()` into a plain `AtomicBool` —
   never `env::var_os` inside `alloc()`, which itself allocates → re-enters the
   allocator → deadlock.
2. **Direct timing** of `complete_data_loglik_grad`: 1% of loop, 20 calls.
3. **samply** with leaf + caller attribution: 60% of the main thread in the
   `write` syscall, under `run_pgas` → a per-sweep `write!`-to-`File`.
4. Traced to `cli/fit/pgas.rs:577`; confirmed file shape (723 × 1456) and file
   count (10) → ~10.5M syscalls.

### samply on macOS (worth knowing)

- `/usr/bin/sample` (the built-in) attaching to a separately-launched process on
  Apple Silicon + SIP **cannot unwind the stack without `sudo`** — every sample
  collapses to `_dyld_start`. Useless output that looks like a hang. Use
  **samply** instead: it execs the target as its own child, so it has sampling
  rights without sudo.
- `samply record --save-only` stores **un**symbolicated frames (raw addresses).
  `atos` against the binary needs the right load base and still mis-resolves
  through inlining (produced an internally-impossible stack here). The reliable
  path: add `--unstable-presymbolicate`, which emits a `.syms.json` sidecar
  whose `symbol_table` rva ranges match the profile's `frameTable.address`
  **directly** (no load-address guessing).

## The fix

`cli/fit/pgas.rs`, in `run_stage`'s trajectory-sample block:

```rust
if let Ok(f) = std::fs::File::create(&path) {
    let mut f = std::io::BufWriter::new(f);   // <- was: let mut f = ... (raw File)
    ...                                       // ~1455 × 723 write! calls
}                                             // BufWriter flushes on drop
```

PMMH's sibling writer already used `BufWriter` (`pmmh.rs:805`); pgas was the
lone offender.

## Downstream: after the I/O fix, recovery is mixing-limited (open)

With a _properly configured_ single-parameter recovery (estimate only `R0_c`,
start 4.0, truth 6.0, all else fixed at truth, burn_in=25), the sampler is
healthy — **78% acceptance** (the 0% acceptance seen on the profiling bench was
a misconfigured bench: it estimated `rho_afp` outside its truth value and fixed
several params off-truth). But `R0_c` does **not** recover — it crawls 4.000 →
4.002 over 50 sweeps.

A PF marginal-likelihood profile (X integrated out, 300 particles, all else at
truth) shows `R0_c` is **sharply identifiable** — peak at the truth, start 4.0
sitting **92.7 nats** below:

```
R0_c   3.0      4.0      5.0      6.0      7.0      8.0
ll    -2049.8  -1411.0  -1329.3  -1318.3  -1324.0  -1332.7   (peak at 6.0 = truth)
```

So the freeze is **purely a NUTS step-scale problem**. The profile curvature
implies a true marginal posterior sd ≈ **0.24**, but NUTS adapted its dense mass
matrix to sd ≈ **0.00015** — ~1600× too tight. It scales steps to the
_conditional_ p(θ | X, y) (artificially narrow: a single trajectory sample X
pins θ through the −3.76M transition density) rather than the _marginal_ p(θ |
y). Steps ~1600× too small → glacial mixing. Candidate fixes (inference math,
undecided): adapt the mass matrix on marginal θ-variance / inflate it; more
`csmc_sweeps_per_nuts`; or simply far more sweeps.

Note: a healthy fit (78% acceptance) builds real NUTS trees and calls the
gradient _many_ times per sweep — so once mixing is fixed, the gradient stops
being a 1% slice and the parked gradient levers may become worth revisiting.

## Repro

```bash
# breakdown (CAMDL_DEBUG_THREADS prints csmc/gradient/recompute/other split)
cd tests/recovery/cases/polio_metapop
CAMDL_DEBUG_THREADS=1 camdl fit run pgas_bench.toml --parallel 16

# R0_c marginal-likelihood profile (identifiability)
for r in 3 4 5 6 7 8; do
  sed "s/^R0_c .*/R0_c = $r.0/" truth.toml > /tmp/p.toml
  camdl pfilter model.camdl --params /tmp/p.toml --data data/afp.tsv --particles 300 --seed 7
done
```

## Next

- Decide the mixing-fix lever (mass-matrix scale is where the evidence points).
- Then re-profile: confirm csmc dominates and the gradient share rises in a
  healthy fit; revisit the parked gradient levers if so.
