# Compiler-testing the documentation

Date: 2026-06-05
Project: camdl
Tags: doctest, documentation, dimcheck, cli, tooling, ci

## Context

The CAMDL documentation is read by two audiences that fail differently: humans,
who tolerate a stale example and move on, and AI coding agents, who copy a
documented snippet verbatim and then have to reconcile a compiler error the doc
promised wouldn't happen. A documented model that no longer compiles, or a `camdl
…` command whose flag was renamed, is not a cosmetic defect for the second
audience — it is a trap that costs an agent a debugging loop and erodes trust in
the docs as ground truth. As the surface area grows (the language spec alone
carries 115 fenced CAMDL examples; the onboarding and reference docs another 64),
hand-verification does not scale and silently rots.

The goal of this work: make "every documented example is checked against the real
compiler" a continuous, enforced invariant rather than a periodic promise — and,
in the process, surface the examples that *don't* compile, each of which is either
a compiler bug or a doc that lies to agents.

## What was built

### 1. A doctest gate — `camdlc doctest`

A subcommand that extracts ` ```camdl ` blocks from Markdown and compiles each
through the real front-end (`Compiler.collect_diagnostics` — the full
lex→parse→expand→validate→dimcheck→lint pipeline, returning structured
diagnostics without aborting). Intent is inferred from the compiler's own verdict
plus block shape rather than a tagging vocabulary, so the spec needs no mass
annotation:

- no error → **pass**; external-file dependence (`read()`/E200) → **skip:data**;
  parse-only fragment (E001, a legend or bare expression) → **skip:parse**; an
  errored block that is not a self-contained model → **skip:fragment**; a
  complete-model-shaped block that errors → **FAIL**.

`--gate` exits nonzero on any FAIL. The design choice that made this tractable:
*classify by the compiler's verdict, don't force everything to compile.* Roughly a
fifth of the spec's blocks are legitimately not models — unit-arithmetic legends,
grammar BNF, deliberate `# ERROR` illustrations, function-signature references —
and correctly stay skipped. The gate's job is to verify the things that *are*
models without false-failing the things that aren't.

### 2. Self-contained hidden preamble + data

Many valid examples are *fragments*: an `observations {}` or `init {}` block that
references compartments and parameters declared elsewhere in the section. To
compile-verify these without cluttering the rendered page, a block can borrow a
hidden preamble and inline data carried in the doc as invisible HTML comments:

```
<!-- camdl-doctest-preamble: sir
compartments { S, I, R }   parameters { gamma : rate }
-->
<!-- camdl-doctest-data: data/pop.tsv
patch<TAB>pop
north<TAB>50000
-->

```camdl preamble=sir
init { S = N0 - I0  I = I0 }      <!-- compiled as preamble ^ block; renders alone -->
```
```

The preamble is prepended before compiling; data chunks are materialised into a
temp directory the block's `read()` paths resolve against. Everything lives in the
doc, so there is no separate fixture file to drift out of sync. (An earlier
file-based `context=` variant was replaced by this in-doc form.)

One bug worth recording from building this: the comment terminator was first
matched anywhere on a line, but CAMDL transitions contain `-->` (`S --> I`), so a
preamble was truncated at its first transition. The fix — require the closing
`-->` on its own line — was caught by the classifier self-test, which is the kind
of thing a non-vacuous test earns you.

### 3. A CLI run-gate — `camdl __check-args`

The same drift problem afflicts the `camdl …` commands in the bash blocks. A
hidden parse-only mode (`camdl __check-args -- <argv>`) runs only clap argument
parsing against the real command tree — no file I/O, no execution — and exits 0 if
the surface parses (or a positional is merely missing, an *input* concern) and 2
on DRIFT (unknown subcommand/flag, unexpected positional, bad enum value). This is
parser-truth: the same typed parser the binary uses decides, so the gate can never
disagree with the binary about what surface is valid. `make test-cli-docs` runs it
over the command-bearing docs; a non-vacuous `--selftest` (synthetic drift it must
catch, valid-but-missing-input it must not flag) and a Rust exit-code test guard
the gate itself.

## Method

Three practices did the heavy lifting and are worth carrying forward:

- **Adversarial review before building.** A panel of independent reviewers
  pressure-tested the design proposal against the actual codebase. Their most
  valuable finding was not a design flaw but a *fact*: the worktree the proposal
  was drafted in was hundreds of commits behind `main`, so half the critique (and
  the proposal) targeted code that had since been rebuilt — the non-aborting
  `collect_diagnostics` surface the doctest now depends on already existed. The
  lesson generalises: verify the currency of the tree before designing against it,
  and an adversarial pass is good at catching "you are looking at stale ground."

- **Compiler fixes via red→green TDD.** The dimcheck fix (below) began with a
  test that reproduced the false positive and was confirmed *failing* against the
  current code, plus a regression test confirming the protection it relaxes still
  fires. A fix whose test never failed first proves nothing.

- **Gates carry their own negative test.** Each gate can demonstrably fail: the
  doctest self-test flips verdicts when a preamble/data is stripped; the CLI gate's
  `--selftest` catches synthetic drift. A gate that cannot fail is theatre.

## Results

Coverage now under continuous gating:

- **Language spec**: 28 → 54 compiler-verified blocks (high-priority feature
  examples — observations, interventions, events, balance, stratification,
  forcing, scenarios, ode, init, tables, multi-source/branching/Erlang
  transitions — wired via hidden preambles/data).
- **Doctest gate spans 6 docs** (spec, intro, user-features, dsl-cheatsheet,
  dates, run-spec): 162 blocks, 69 pass, 0 FAIL — each locked against future drift.
- **CLI gate**: 37 `camdl …` commands across workflow.md, inference.md,
  debugging.md, 0 drift.

### High-reward bugs surfaced (see `2026-06-05-doctest-drift-findings.md`)

- **Compiler bug — `projected` mis-dimensioned (fixed, `b141634`).** The
  `projected` likelihood keyword was hard-coded to dimension *population*, so a
  projection that is a proportion (`I / N`) used as a binomial probability
  false-fired E304. Root cause: a constant where an inference belonged — the
  `Projected` AST leaf was disconnected from the projection expression that
  actually determines its dimension. Fixed by threading the projection's inferred
  dimension to the leaf, which *preserves* the deliberate check that catches the
  missing-`/N` bug (a count used as a probability) while accepting valid
  proportions.
- **Doc bug — the definitional real-compartment example never compiled.** The
  canonical "declare a continuous-state compartment" snippet (`S, I, R` newline
  `W : real`, missing a comma) was E001 in both the language spec and the cholera
  worked example. The teaching example for the feature did not parse.
- **Doc bug — the onboarding age-SIR omitted its `parameters{}` block**, so the
  most-read intro example failed with undeclared `beta`.
- **CLI drift — inference.md / debugging.md** documented flags that do not exist
  (`profile --focal/--grid` → really `--sweep "R0=lin(…)"`; `pfilter
  --obs-model/--tol`; `simulate --trace`). All fixed and now gated.
- **Open: compiler bug — positional table index inside an intervention `at[]`**
  schedule (E263), repro pending. The §23 marquee spatial example also does not
  compile (gravity-kernel dimension, stratified-transfer E264); left skipped.

## Status and what remains

Gated and green today: 6 docs via `make test-docs`, 3 docs via `make
test-cli-docs`, both wired into a doc-triggered CI job (the main CI ignores
`docs/**`, so doc-only PRs need their own job). Incremental follow-ups, all
low-friction now that the mechanisms exist: the two doc-error renames the
wrapping pass surfaced (a parameter named `rate`, a reused `p_symp` at scalar and
indexed arity); the dimension-annotation grammar gap (no way to spell `1/(P·T)`,
which blocks the chemistry-style multi-source example); whether
`sum(c in compartments, …)` should be user-writable (the parser rejects it though
§10 presents it as such); the §23 marquee example and the E263 intervention-index
bug; and extending both gates to the remaining docs as they are brought to green.
The marginal cost of compiler-verifying one more documented example is now a few
lines of hidden preamble — cheap enough that "documented" and "verified" can
converge.
