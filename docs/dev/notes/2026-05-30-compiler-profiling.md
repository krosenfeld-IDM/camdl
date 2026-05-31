# Compiler (camdlc) profiling: where the time goes on large models

Date: 2026-05-30
Project: camdl
Tags: perf, compiler, camdlc, serialization, scaling, profiling

## Context / question

The FOI-scaling work (`2026-05-29-foi-scaling-bench.md`) cut IR size and sped
up the Rust runtime. For large spatial models the *compiler* wall-clock is now
a visible cost: the real Kano LGA SEIRV model (44 LGAs × 21 ages, 4 620
compartments) takes **~21 s and ~8.4 GB RSS** to compile. This note establishes
a compiler-only timing baseline and localizes the hot path before optimizing
(measure-first, per the optimization-methodology rule).

The prior hypothesis — carried in the task brief — was that the 5 078-line
`expander.ml` is the compile hot path. **The profile refutes that.** The hot
path is JSON serialization; the expander is ~1 %.

## State of existing compiler profiling (before this note)

Verified by reading the Makefile and `scripts/`:

- `scripts/bench_scaling.py` + `make bench-scaling` — times the **full**
  `camdl compile` → `simulate` pipeline through the Rust CLI and records
  `compile_s` as one coupled number alongside `sim_s`. Tuned to OOM-dangerous
  scales (the P=44,A=21,full point was the ~15.6 GB pre-Fix-B OOM hazard).
- `make bench-micro` / `scaling.rs` (criterion) — per-step `eval_propensities`
  / `step_one` / model-load; **runtime only**.
- `make flamegraph-real` / `flamegraph-bench` / `profile-pmmh` — inferno
  flamegraphs over `simulate` / PMMH; **runtime only**.

Gap: nothing isolated `camdlc` itself, and nothing split the compile into its
passes. This note adds both.

## What was added

1. **`scripts/bench_compile.py` + `make bench-compile`** — times `camdlc.exe`
   *directly* (no Rust runtime) over a synthetic ladder (reusing
   `gen_scaling_models.gen_camdl`) plus the real Kano model; records median/min
   wall + peak RSS + emitted IR size. `--passes` additionally captures a
   per-pass breakdown.
2. **`ocaml/lib/ir/passtime.ml`** — env-gated (`CAMDL_TIME_PASSES`) per-pass
   timing wired into `compiler.ml` (parse / expand / validate / dimcheck /
   autodiff) and `camdlc.ml` (serialize). Zero-cost and silent unless the env
   var is set; writes only to stderr, so the emitted IR is byte-for-byte
   unchanged (the compile-side analogue of `CAMDL_TRACE_STEPS`).
3. **`scripts/plot_compile.py`** — scaling curves + per-pass breakdown figure,
   overlay-capable for before/after comparisons.

## Baseline (vanilla switch: OCaml 5.2.0, flambda OFF, `dune build` dev profile)

`docs/dev/notes/assets/compile/compile_baseline.tsv`, reps=3, dim-check ON
(the cost a user actually pays). Figure: `compile_curves.png`.

| model | n_comp | n_tr | IR (MB) | wall (s) | peak RSS (GB) |
|---|---|---|---|---|---|
| P8_A21_on_full   | 672  | 504  | 38.8   | 0.45  | 0.21 |
| P16_A21_on_full  | 1344 | 1008 | 146.7  | 1.66  | 0.79 |
| P32_A21_on_full  | 2688 | 2016 | 570.2  | 6.25  | 3.06 |
| P44_A21_on_full  | 3696 | 2772 | 1069.7 | 11.91 | 4.78 |
| **kano_lga_seirv** (real) | 4620 | — | 1814.6 | **20.44** | **8.41** |

Two clean relationships hold across the whole ladder (the real model lands on
the same lines as the synthetic ladder — the toy generator is a faithful
compile-cost proxy):

- **Compile wall time ∝ emitted IR bytes**, ~11 ms/MB (P32 11.0, P44 11.1,
  Kano 11.3). The O(P²) in the curve is the IR-size O(P²) from the flat spatial
  coupling, *not* a super-linear algorithm.
- **Peak RSS ∝ emitted IR bytes**, ~4.6–5.2× IR.

## The profile: serialization is 97 %

`CAMDL_TIME_PASSES=1` breakdown (`compile_baseline_passes.tsv`, figure
`compile_passes.png`). Processor seconds, share of total:

| pass | P32 | P44 | Kano |
|---|---|---|---|
| parse | 0.000 (0%) | 0.000 (0%) | 0.000 (0%) |
| expand | 0.026 (0.4%) | 0.058 (0.5%) | 0.214 (1.1%) |
| validate | 0.002 (0%) | 0.003 (0%) | 0.010 (0.1%) |
| dimcheck | 0.078 (1.3%) | 0.176 (1.5%) | 0.159 (0.8%) |
| autodiff | 0.062 (1.0%) | 0.126 (1.1%) | 0.205 (1.0%) |
| **serialize** | **6.004 (97.3%)** | **11.556 (97.0%)** | **19.364 (97.1%)** |
| TOTAL | 6.172 | 11.919 | 19.952 |

Reproduce (verified, not inferred):

```
$ CAMDL_TIME_PASSES=1 ocaml/_build/default/bin/camdlc.exe \
      .../kano_lga_seirv.camdl -o /tmp/kano.ir.json 2>&1 >/dev/null | tail -8
  parse            0.000 s    0.0%
  expand           0.214 s    1.1%
  ...
  serialize       19.364 s   97.1%
  TOTAL           19.952 s  100.0%
```

The expander — the assumed hot path — is **1.1 %**. Everything is
`Serde.model_to_string`, which is:

```
$ rg -n 'pretty_to_string|envelope_to_json' ocaml/lib/ir/serde.ml
1105:  Yojson.Safe.pretty_to_string (envelope_to_json m)
```

i.e. IR → full `Yojson.Safe.t` AST (`envelope_to_json`) → pretty-printer →
1.8 GB string. Three full materializations of an 1.8 GB document; the
pretty-printer (column-fitting layout engine) is the dominant term, and the
boxed intermediate AST explains the ~5× RSS/IR ratio.

## Implications — ranked levers (to be verified with the harness)

1. **Serialization is the only thing worth optimizing.** Any expander
   micro-optimization is capped at ~1 % — ignore it.
2. **flambda + release build** (byte-identical): codegen speedup of the
   Yojson/pretty traversal. The switch is flambda OFF and the dune files carry
   no `ocamlopt_flags`/release profile. Cheapest byte-safe win; measured in the
   companion section once the flambda switch build lands. *Prior: modest — the
   layout engine work remains; flambda speeds its codegen but does not remove
   it.*
3. **Compact serialization** (`Yojson.Safe.to_string`/`to_channel`, or a direct
   IR→Buffer writer): bypasses the pretty-print layout engine entirely — the
   thing that *is* the 97 %. Almost certainly the dominant lever (potentially
   several×). **But it changes the emitted IR bytes** (whitespace only;
   semantically identical JSON the Rust side parses regardless). That trips the
   "golden byte-identical" gate, so it is a **format decision for the
   maintainer**, not a silent change. Recommend measuring the ceiling and
   deciding explicitly (alpha + "clean breaks preferred" argue for it; golden
   readability of *small* models argues against a blanket switch). A
   byte-identical replica of Yojson 2.x's adaptive pretty layout is fragile and
   not recommended.
4. **Stream pretty-print to the output channel** (`pretty_to_channel`,
   byte-identical): avoids materializing the final 1.8 GB string. A memory win
   (~the string), small time win. Free and safe; worth taking regardless.

The IR size itself (O(P²) from flat spatial coupling) is the FOI study's
domain (sparse coupling) — out of scope here, but note that halving IR bytes
halves both compile time and RSS one-for-one.
