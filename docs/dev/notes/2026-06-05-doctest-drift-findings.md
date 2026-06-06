# Doctest drift findings — what compiler-testing the docs surfaced

Date: 2026-06-05
Project: camdl
Tags: doctest, dimcheck, cli, drift

Wiring the `camdlc doctest` gate through the language spec, the onboarding docs,
and the CLI command examples surfaced the following. Each is tagged with its
verification status (✓ verified against code with a pasted repro here; ⟳
reported by a conversion pass, not yet independently reproduced).

## Compiler bugs (valid CAMDL the compiler wrongly rejects)

### B1 ✓ FIXED (`b141634`) — `projected` keyword hard-coded to dimension P → false E304
`Projected` is unconditionally dimensioned `population` in the dim-checker
(`ocaml/lib/ir/dimcheck.ml:282` and `:584`: `| Projected -> Known population`).
So a projection that is a *proportion* used as a probability false-fires E304.

Repro (`camdlc check`):
```
observations {
  pos : {
    projected  = I / N                              # a proportion (dimensionless)
    likelihood = binomial(n = 100, p = projected)   # E304: p must be dimensionless
  }
}
```
`error[E304]: Binomial p must be dimensionless … got P (population count)`. The
explicit form `binomial(n = 100, p = I / N)` passes. The dim-checker should take
the projection expression's own dimension, not assume P.

Blocks left skipped by this: spec §12 prevalence-as-proportion (`slide_positivity`).

### B2 ⟳ — positional table index + bound binder mis-resolves inside intervention `at[]` (E263)
`sia[p in patch] : transfer(...) at [sia_day[p, 0], sia_day[p, 1]]` errors
`'?' is not a level of dimension 'round'`. Per the conversion pass, the same
`sia_day[p, 0]` (binder + positional index) resolves fine in a transition rate
but not inside an intervention `at[]` schedule. The spec's own prose asserts it
"resolves to the correct row at compile time," so the compiler is wrong. Needs
an independent minimal repro before fixing. Block left skipped: spec §13.3.

## Doc errors

### D1 ✓ fixed — real-compartment example never compiled (E001)
The definitional example for a continuous-state compartment (spec §3, and §22.5
cholera) wrote `S, I, R` newline `W : real` with no comma after `R`. Compartments
are comma-separated; newline is not a separator. Fixed (`S, I, R,`).

### D2 ✓ fixed — scenario `set`/`scale` must be newline-separated (E001)
Spec §17.2 used `set = { beta = …, gamma = … }` comma-separated on one line; the
parser wants one entry per line (its own hint says so). Fixed to newline form.

### D3 ✓ fixed — intro age-SIR omitted `parameters{}` (E100)
`intro.md`'s flagship "Add age structure to the SIR" example declared no
`parameters{}`, so `beta`/`gamma` were undeclared. The most agent-facing doc
taught a non-compiling model. Added the params block.

### D4 ⟳ — inline `set(COMP, value = EXPR)` action has no grammar production (E001)
Spec §13/§13.1 document `set(I[child, p1], value = …) at […]`, but `parser.mly`
has productions only for `transfer(...)`, `add(...)`, and the block action form.
Either the spec is wrong (use the block form) or `set(...)` should be added to
the grammar. Decide direction. Block left skipped: spec §13 intro.

## CLI drift (commands/flags the docs show that don't exist — agents will fail)

**Resolution:** all fixed (`0218b3f`) and now gated. A permanent CLI run-gate
(`f5501ef`, `e96ec5a`) adds `camdl __check-args` (parse-only, exit 2 on a bad
command/flag) and `make test-cli-docs` over workflow.md + inference.md +
debugging.md (37 commands, 0 drift), with a non-vacuous `--selftest` and a Rust
exit-code test.

Verified against `rust/crates/cli/src/args/mod.rs` (clap-typed; surface errors
exit 2, distinct from input errors exit 1).

- ✓ `docs/inference.md:283` — `camdl pfilter … --obs-model discretized_normal --tol 1e-18`.
  `--obs-model` and `--tol` do **not** exist on `PfilterArgs`. (The obs model is
  declared in the model, not a CLI flag.)
- ✓ `docs/inference.md:539` and `:555` — `camdl profile … --focal R0 --grid "…"`
  and `--focal alpha,gamma --grid-alpha … --grid-gamma …`. `--focal`/`--grid*`
  do **not** exist; the real API is `--sweep "R0=lin(0.5,5,20)"` (repeat `--sweep`
  for 2D). (`ProfileArgs.sweep`, args/mod.rs:1443.)
- ✓ `docs/debugging.md:82` — `camdl simulate … --trace`. `trace` is a
  `PfilterArgs` field (args/mod.rs:1253), not `SimulateArgs` (437).

`docs/workflow.md` itself is **drift-free** — all 14 `camdl` commands parse.

A prototype CLI run-gate exists at `scripts/check_cli_docs.sh` (extracts `camdl …`
from bash fences, classifies DRIFT vs EXPECTED by exit code + stderr). The robust
permanent form is a hidden `camdl <sub> --check-args` parse-only mode (exit 0/2,
no I/O) so DRIFT = exit 2 with no stderr heuristics — ~30 lines.

## Wrapping-pass findings (transition features §9–§10)

Surfaced wrapping bare-transition feature snippets in `transitions { }` and
compiling them. §12 prevalence-as-proportion converted cleanly (confirms B1's
fix end-to-end); the following were left skipped:

### G1 ⟳ — dimension-annotation grammar can't spell `1/(P·T)` (§9.1.1)
The chemistry-style multi-source line `react : A + B --> C @ k * A * B` needs `k`
declared with dimension `P⁻¹·T⁻¹` so `k·A·B` is `P·T⁻¹`. The `[dim]` annotation
grammar (§4.1.1) admits `[1] [P] [T] [1/T] [T^-1] [P/T] [P*T^-1]` but none of
`1/(P*T)`, `P^-1*T^-1`, `1/P/T` parse (E001). Without `k` annotatable, the line is
E300. Verdict: extend the annotation grammar to compound inverse-population
dimensions, or drop the chemistry line. (The other multi-source examples —
`bite`, `infect_v` — compile.)

### G2 ⟳ — `sum(c in compartments, …)` rejected though §10 shows it as user syntax
§10.1/§10.2 use `sum(c in compartments, c[b,t])` as the stratum-population
denominator and §10.3 says it is "generated automatically," but the parser only
accepts `sum(b in age, …)` over declared dimensions — the `compartments` binder is
E001. Verdict: COMPILER-BUG if it is meant to be user-writable (§10.1 presents it
as the hand-written primitive), else the spec should not show it as user syntax.

### D5 ⟳ — `p_symp` reused at scalar and indexed arity (§9.1.2)
The branching example's malaria variant uses `p_symp[a]` while the headline uses
scalar `p_symp`; one name cannot be both in a model. Rename the indexed one
(`p_symp_age[a]`). Content edit.

### D6 ⟳ — reserved word `rate` used as a parameter name (§9.3)
The compound-guard `transfer` example declares `parameters { rate : … }`; `rate`
is a reserved type keyword (E001). Rename (`tau`). Content edit. (`fertility[a]`
also needs an indexed declaration.)

## Other

- ⟳ Spec §11 says the `ode {}` block is "parsed but currently discarded by the
  expander," yet `W : real` + `ode {}` compiles cleanly with no diagnostic — the
  prose may be stale relative to the compiler. Verify.
- Spec §23 "Full Example (spatial age-structured SEIR)" does not compile even with
  consistent data: gravity-kernel `let mig = theta * pop[j] / distance²` is
  dimension P not T⁻¹ (E300 on every `migrate`), a `transfer` over stratified
  compartments hits E264, and `init { S[child, p1] }` references a patch level not
  in the data. The marquee example needs a dedicated fix pass; left skipped.

## needs-wrap (high-value features blocked from doctesting by presentation)
~10 spec blocks present transitions as bare top-level lines (invalid without a
`transitions { }` wrapper), so they can't be preamble-compiled: §9.1.1 multi-source
(`bite : S_h + I_v --> I_h + I_v`), §9.1.2 branching (`X --> { A : p, B : 1-p }`),
§9.4 Erlang/consecutive sub-staging, §9.5 compartment iteration, §10.1 coupling
primitive. Wrapping each shown snippet in a `transitions { … }` block (a small,
faithful doc edit) would make these features compiler-verified.
