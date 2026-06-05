# Proposal: compact IR JSON serialization (4.6× faster compile, 5× smaller IR)

Date: 2026-05-30 Project: camdl Status: **ACCEPTED — Option 2 implemented**
(single canonical compact format; `--pretty` kept as a view). Maintainer chose
Option 2 (newline-delimited compact) to keep golden diffability. Tags: perf,
compiler, serialization, ir, golden

Profiling note:
[`docs/dev/notes/2026-05-30-compiler-profiling.md`](../notes/2026-05-30-compiler-profiling.md)

## Decision & outcome (2026-05-30)

Chose **Option 2**: one canonical compact format everywhere (what ships = what
CI byte-tests), with one element per line for the model's top-level arrays so
golden diffs stay reviewable; `camdlc --pretty` / `camdlc inspect` give the
indented view. Measured on implementation (`bench_compile`, reps=3 min):

|                   | before (streaming pretty) | after (canonical compact)          |
| ----------------- | ------------------------- | ---------------------------------- |
| Kano compile wall | 20.4 s                    | **4.42 s** (4.6×)                  |
| Kano IR on disk   | 1814.6 MB                 | **360.9 MB** (5.0×)                |
| Kano peak RSS     | 3.02 GB                   | 3.51 GB (~flat, AST-bound)         |
| sir_basic golden  | 179 lines                 | 18 lines (still 1 transition/line) |

Correction to the table below: Option 2 turned out **equal** to Option 1 on
speed/size/memory (all AST-bound), not faster/lower-memory — its only edge over
Option 1 is the diffable goldens. Divergence is pinned by `canonical_equiv_test`
(`test_ir_roundtrip.ml`): for every golden,
`from_string(compact) ==
from_string(pretty)` and compact round-trips to the
model. The writer iterates the canonical `envelope_to_json` AST (no hardcoded
field list), so a future schema field cannot be silently dropped.

## Problem

Compiling large models is dominated — **97 %** — by IR JSON serialization
(`Serde.model_to_string` → `Yojson.Safe.pretty_to_string`). The real Kano LGA
SEIRV model compiles in ~20 s / 8.4 GB RSS, of which ~19 s is the pretty
printer. Measured (`bench_compile.py`, reps=3 min, OCaml 5.2.0):

|                   | baseline (pretty) | compact (`to_string`) | factor   |
| ----------------- | ----------------- | --------------------- | -------- |
| Kano compile wall | 20.44 s           | **4.47 s**            | **4.6×** |
| Kano peak RSS     | 8.41 GB           | **3.60 GB**           | **2.3×** |
| Kano IR on disk   | 1814.6 MB         | **360.8 MB**          | **5.0×** |
| P44 compile wall  | 11.91 s           | 2.64 s                | 4.5×     |
| P44 IR on disk    | 1069.7 MB         | 201.1 MB              | 5.3×     |

The pretty-printed IR is **~80 % whitespace** (indentation of deep `Expr`
trees). That whitespace is pure waste _end to end_: the Rust runtime is
parse-bound on IR bytes (FOI study, `scaling.rs::bench_load`, ~230 MB/s), so a
5× smaller IR also makes the runtime's `ir::from_str` ~5× faster. Compact JSON
therefore wins the whole pipeline — compile time, compiler RAM, on-disk IR, and
runtime parse — for one change.

Flambda was tested and does **not** help (the cost is allocation + memory
bandwidth, not codegen — see the profiling note). The only lever is the
serialization format.

## Why this needs a decision (not a silent change)

Compact output changes the bytes of every committed golden `.ir.json`.
`make update-golden` would rewrite them all from multi-line to a single line.
That is semantically identical JSON (the Rust side parses it regardless), but it
trades away the **line-by-line git-diffability of the golden files** — today a
one-field change to a small model is a small golden diff a reviewer can eyeball;
under whitespace-free compact it becomes "the whole line changed". The golden
review workflow is the thing at stake.

## Options

1. **Compact `Yojson.Safe.to_string`** (1-line change in `serde.ml`).
   - Full win: 4.6× / 2.3× / 5×. Minimal code.
   - Goldens become one line each. Loses golden diffability.

2. **Compact `Yojson.Safe.to_channel`** (streamed, ~5-line change).
   - Same speed; lower memory than (1) (no intermediate giant string — only the
     Yojson AST). Goldens still one line each.

3. **Custom streaming writer — compact _with_ newlines at the array-element
   level** (write IR → `Buffer`/channel directly, ~one object per
   compartment/transition/binding per line, compact within the line).
   - Best of both: ~(1)'s speed, lower memory than (1)/(2) (skips the Yojson AST
     entirely — RSS floor drops below 3.6 GB), **and** keeps golden diffability
     (a changed transition is a one-line diff). New format, new golden bytes;
     ~150 lines mirroring the existing `*_to_json` functions as buffer-writers.
     Recommended if golden diffability is worth the code.

4. **Keep pretty, stream to channel** (`pretty_to_channel`, byte-identical) —
   _already landed on this branch_ as a safe, no-decision memory win (saves the
   1.8 GB intermediate string). Does **not** address the 97 % time (the layout
   engine still runs). Memory only. (See measured number in the profiling note.)

5. **Do nothing beyond (4)** — keep pretty for golden readability; accept the
   ~20 s / 8.4 GB Kano compile. Optionally add a flambda switch +
   `-O3 -inline
   1000` for a byte-identical ~8–13 % (requires adopting a
   flambda toolchain).

## Recommendation

**Option 3** if golden diffability is to be preserved (the model author / CI
review workflow values it); **Option 1** if it is acceptable to treat the IR as
a pure machine artifact and lean on `camdlc inspect` for human reading. Both
deliver the ~4.6× compile / ~5× IR-size win; (3) additionally lowers the RSS
floor and keeps diffs reviewable. Given the project ethos ("alpha; clean breaks
with regenerated goldens preferred"), either is defensible — the open question
is purely golden-file ergonomics, which is the maintainer's call.

Whatever is chosen changes the IR-on-disk format, so it lands as: update the
serializer, `make update-golden` (regenerate all goldens + `ir/golden` Rust
fixtures), confirm the Rust side still parses (it will — serde_json is
whitespace-agnostic), one atomic commit. The IR schema version need not bump
(structure is unchanged; only whitespace).

## Out of scope

IR _size_ reduction beyond whitespace (the O(P²) coupling blowup) is the FOI
study's sparse-coupling domain. Compact serialization is orthogonal and stacks
with it.
