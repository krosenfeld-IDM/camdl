# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with
code in this repository.

## Implementation standard

This software is used to inform major public health decisions. Errors
in inference, simulation, or data handling are not just bugs — they can
mislead policy. Every implementation must be:

- **Correct before clean**: verify logic against the mathematical
  derivation or spec before refactoring for style.
- **Tested at every step**: run `cargo test` before and after each
  change; do not batch multiple semantic changes into one commit without
  an intermediate green test run.
- **Reviewed against the proposal**: when implementing from a proposal
  in `docs/dev/proposals/`, follow it exactly unless a concrete reason
  to deviate is documented inline. Do not improvise design changes
  mid-implementation.
- **Conservatively scoped**: if a change touches inference math
  (`pgas.rs`, `pgas_grad.rs`, `obs_loglik.rs`, `obs_model.rs`,
  `if2.rs`, `particle_filter.rs`), treat it as high-risk regardless of
  how mechanical it looks. Read the full function before editing any
  part of it.

## Working on this codebase

AI is leverage; the standards belong to the maintainer. You are the
careful counterpart, not the arbiter of scientific judgment.

- **The compiler and tests are ground truth.** When unsure what a
  construct means, check the compiler, don't guess. A wrong guess
  must surface as a compile error or failing test — never as a
  silent change that looks plausible.
- **Verify against code, not docs — and paste the verification
  inline.** Doc text describes intent that may have drifted from the
  implementation. Before writing an incident report, a fix section,
  or any normative claim about how the system behaves *today*, run
  the command that verifies it (grep the file, read the function,
  run the test) and *paste the command and its output into the
  artifact alongside the claim*. Not "expander.ml uses Julian
  `365.25/12`" but "`rg 365 ocaml/lib/compiler/expander.ml` → no
  matches in the expander; OCaml does not use 365.25." The pattern
  self-corrects: you can't write a load-bearing claim without first
  running the command, and the command either confirms or refutes.
  If the output is too long, paste the command alone with a
  one-line summary of what it confirmed.
- **Mark inference vs verified.** "The spec says X" and "the code
  does X" are different claims. If you've only read the doc, write
  "the spec says X (not yet confirmed against the implementation)"
  — one clause surfaces the gap. The failure mode the previous rule
  prevents is the silent promotion of "the doc implies" to "the
  code does."
- **Fix bugs via TDD: red → green → refactor.** When fixing a
  reported bug, write a test that *asserts the correct behaviour*
  first, run it and confirm it FAILS against the current code, then
  apply the fix and confirm the test now PASSES. The failure is the
  diagnostic — a test that doesn't fail on the buggy code isn't
  actually exercising the bug, and a "fix" that passes a never-failing
  test isn't proof of anything. After green: re-run the existing
  suite to confirm no regressions. This applies even when the fix
  looks obvious — "I'll write the test after" routinely produces
  tests that pass for the wrong reason (assert the symptom, not the
  cause; assert a related fact that was already true; or get the
  baseline wrong and silently pass). Concretely: paste the
  red-then-green test output in the commit message as the proof
  the fix landed where intended.
- **Incident reports require a reproduction.** A concrete input →
  wrong output, with the command that produced it. "Would be off by
  ~0.4 days" is a hypothesis, not an incident. If you can't produce
  a reproduction, the artifact is a *question* filed under
  `docs/dev/notes/`, not a `docs/dev/incidents/` entry. The
  reproduction bar is what keeps phantoms out of the incident archive.
- **Classify discrepancies before proposing fixes.** Three classes,
  three different fixes:
  - *doc-vs-doc* — edit a doc.
  - *doc-vs-code* — verify which side is right, then sync the loser.
  - *code-vs-code* — fix the code and add a test pinning the
    agreement.
  State the class explicitly at the top of any incident or proposal
  that depends on the answer. Misclassifying inflates a typo into an
  engineering project (or, the other direction, hides a real bug
  behind a doc edit).
- **Ship the fix; don't document the broken interim.** When a bug
  fix is straightforward and the fixed state is the right state,
  apply the fix and update the user-facing doc to describe the
  *fixed* reality. Long descriptions of the broken interim state
  belong in incident reports, not in spec/cheatsheet/user-features.
  Doc-around-the-bug is noise that delays shipping and confuses the
  next reader.

### Self-check tells that you're describing rather than verifying

When you catch any of these in your own draft, stop and run the
verification before continuing:

- Hedged tense (*would*, *could*, *might*) where *is* belongs to
  describe current behaviour.
- A detection story that doesn't name the file you read or the
  command you ran to confirm the finding.
- Corroborating detail — specific line numbers, conversion tables,
  three-decimal constants — too complete for a claim that was
  trivially checkable.
- Process-moralising disproportionate to what was actually verified
  (three "lessons learned" about a bug whose existence was never
  demonstrated).
- Self-narrated diligence as a load-bearing claim — "a careful read
  would have caught this" is itself an unverified claim about your
  own conduct.
- **Never lower the bar to make something pass.** No `--no-verify`,
  no weakening an assertion, no skipping a gate, no widening a
  tolerance to get green. If something fails, find the cause.
- **Surface uncertainty.** If a change touches inference math or
  numerics and you are not certain it is correct, say so explicitly
  and propose the test that would settle it. "Plausible" is not
  "verified" — this software informs public-health decisions.
- The maintainer welcomes scrutiny over speed: a found bug or a
  flagged dubious design is more valuable than a fast green diff.

### Required reading before structural proposals

Before drafting a `docs/dev/proposals/` document or making
non-trivial changes to load-bearing surfaces, read the normative docs
for that area first. Working from a mental model of the language
rather than from the spec has, in practice, produced proposals that
reinvent existing surface badly — once is bad luck, twice is a
pattern, and the pattern is fixed by reading first, not by trying
harder to remember.

Per area:

- **DSL changes** (lexer, parser, expander, dimcheck, new unit
  literals, new functions in DSL constant positions): read
  [`docs/camdl-language-spec.md`](docs/camdl-language-spec.md)
  end-to-end (especially §2 on units and dimensions, §4 on parameter
  kinds, §6 on tables, §7 on forcings),
  [`docs/user-features.md`](docs/user-features.md) for example
  patterns, and [`docs/dsl-cheatsheet.md`](docs/dsl-cheatsheet.md)
  for a fast orientation. For the actual grammar:
  `ocaml/lib/compiler/lexer.mll` (unit literals + tokens),
  `ocaml/lib/compiler/parser.mly` (the rule for whatever you're
  changing), `ocaml/lib/compiler/dimcheck.ml` (dimensional behaviour).
- **IR / schema changes**: read `ir/schema.json` (the OCaml↔Rust
  contract) and `ir/VERSION`. The atomic update procedure is at
  "Changing the IR schema" below. Cross-language constants follow the
  pattern of `rust/crates/ir/src/caltime.rs::rata_die` — single
  source of truth, mirror only with an equivalence test.
- **Calendar / time / date changes**:
  [`docs/dates.md`](docs/dates.md) is the policy document;
  `docs/camdl-language-spec.md` §2.1 has the unit table;
  `rust/crates/ir/src/caltime.rs` is the conversion code;
  `docs/dev/proposals/2026-05-22-calendar-time.md` and
  `docs/dev/proposals/2026-05-22-typed-time-and-dsl-ergonomics.md` are
  in-flight design.
- **Inference math**: the proposal that introduced the feature
  (under `docs/dev/proposals/`), the relevant module in
  `rust/crates/sim/src/inference/`, and any related incident reports
  in `docs/dev/incidents/`.

When a proposal is the *first* thing you'd read about a topic, that
proposal needs to either be self-contained (cites all the existing
surface relevant to its claims) or explicitly state what background
the reader is assumed to bring. The "read the spec first" rule is for
the author, not just the reviewer.

## docs/dev layout and where work gets tracked

- `docs/dev/notes/` — dated design sketches, investigation logs.
- `docs/dev/incidents/` — serious bugs/outages: cause, fix, what it
  changes.
- `docs/dev/reviews/` — audits and PR write-ups. Audit-fix commits
  cite these via an `Audit ref:` footer.
- `docs/dev/proposals/` — RFCs for non-trivial changes. Implementation
  commits cite via a `Proposal:` footer; follow the proposal exactly
  unless a deviation is documented inline.
- Stable normative docs live at `docs/dev/` root (e.g.
  `commit-style.md`, `testing.md`, `warning-catalog.md`).

Now that camdl is alpha:
- **Small, well-scoped work → a GitHub issue** (`gh issue create`),
  referenced as `gh#NN` in the commit subject. No proposal needed.
- **Bigger lifts** (schema/IR changes, new inference methods, anything
  cross-cutting) → a `docs/dev/proposals/` doc first, then implement
  against it.
- Commit/PR conventions: `docs/dev/commit-style.md`. Contributor
  onboarding: `CONTRIBUTING.md`.

## Project Overview

`camdl` is a monorepo for stochastic compartmental epidemic modelling.
It has two independent subsystems connected by a shared JSON IR (Intermediate
Representation):

- **OCaml frontend** (`ocaml/`): DSL → stratification expansion → IR
  serialization
- **Rust backend** (`rust/`): IR deserialization → simulation →
  trajectory/observation output

The IR schema (`ir/schema.json`) is the contract between them. Changes to the
schema must be reflected in both language implementations atomically.

## Build Commands

```bash
make build           # build both OCaml and Rust
make build-ocaml     # cd ocaml && dune build
make build-rust      # cd rust && cargo build --release
```

## Test Commands

```bash
make test            # all levels: unit + golden + integration
make test-unit       # fast, per-language unit tests only
make test-golden     # golden IR deserialization + simulation determinism

# OCaml only
cd ocaml && dune runtest

# Rust only
cd rust && cargo test

# Single Rust test file
cd rust && cargo test --test golden_simulate
cd rust && cargo test --test expr_eval

# Integration (cross-language, slow — CI only)
bash tests/test_ocaml_to_rust.sh
```

## Golden File Management

Golden files in `ir/golden/` are the integration test surface — committed IR
JSON that both sides must parse and agree on.

```bash
make update-golden    # recompile DSL fixtures → ir/golden/*.ir.json
make update-expected  # re-simulate golden models → ir/expected/*.tsv
```

When adding a new model: write DSL in `tests/fixtures/`, run `update-golden`,
review the JSON, run `update-expected`, review the TSV, commit all three
together.

## Quick Simulation

```bash
make sim MODEL=ir/golden/sir_basic.ir.json
# or directly:
rust/target/release/camdl simulate <model.ir.json> --traj /tmp/traj.tsv --obs /tmp/obs.tsv
```

## Debugging a diverging simulation

When a simulation's dynamics don't match a reference implementation (pomp,
Stan, a paper's published trajectory), the first tool is the per-substep
tracer built into the chain-binomial backend:

```bash
CAMDL_TRACE_STEPS=1 camdl simulate model.camdl --params p.toml \
    --backend chain_binomial --dt 1 --seed 1 --obs-only /tmp/obs.tsv \
    2> /tmp/trace.tsv 1>/dev/null
```

The trace dumps one TSV row per substep to **stderr** with columns:
`t`, all compartment counts, all `flow_<name>` (counts per substep), all
`rate_<name>` (total per-source rates evaluated this substep), and
`total_pop`. Redirect stderr to a file — stdout carries the normal TSV
simulation output, so keep them separate.

Workflow: pick a few diagnostic times (t=1, after seasonal onset, at
peak, post-epidemic trough) and compare the rate/flow columns against
hand-computed values from the reference implementation's rate
expressions. A mismatch at t=1 localizes to init or rate construction; a
mismatch that grows over time localizes to dynamics (noise, forcing
interaction, event ordering).

Other logging channels worth knowing about:
- `log::debug!` in `pgas.rs`, `particle_filter.rs`, `if2.rs`: inference
  diagnostics (-inf logliks, skipped observations, density mismatches).
  Enable with `RUST_LOG=camdl_sim=debug` or similar.
- `CAMDL_TRACE_STEPS=1` also activates in `intervention.rs` — logs
  intervention firings alongside the substep trace.

Before inventing new logging, check the existing paths above. They
already cover most per-step/per-iteration diagnostics.

## Architecture

### The IR as contract

The IR is a **fully-expanded** declarative model — no stratification shorthand
survives serialization. The OCaml compiler performs stratification expansion;
what reaches Rust is a flat list of compartments, transitions (with
stoichiometry + rate expression), observation models, parameters, and initial
conditions.

The expression language (`expr`) is a pure, total, first-order AST over
`Const | Param | Pop | PopSum | Time | BinOp | UnOp | Cond | TimeFunc | TableLookup`.
No recursion, no binding — propensities evaluate in bounded time. `Cond` guards
against division-by-zero in Gillespie. `TableLookup` keeps stratified models
compact (contact matrices, age-specific rates).

### Rust crate dependency order

```
cli → io → observe → sim → ir
```

- `ir`: pure types + serde, no simulation logic
- `sim`: simulation backends (Gillespie, tau-leap, ODE, chain-binomial) +
  propensity evaluator; defines the `Model` trait
- `observe`: projection + likelihood sampling/scoring; depends on `sim` for
  `Trajectory`
- `io`: TSV read/write glue
- `cli`: arg parsing + orchestration

### OCaml library order

```
expand → dsl → ir
```

- `ir`: OCaml types mirroring the schema + Yojson serialization/deserialization
- `dsl`: embedded DSL builder combinators; produces pre-expansion IR
- `expand`: base model × stratification spec → flat expanded IR (the core
  compiler logic)

### RNG and paired-seed coupling

The runtime uses a plain ChaCha8 `StatefulRng`. Paired scenarios with
the same seed produce identical trajectories only while the RNG is
consumed in the same order on both sides: pre-intervention
trajectories are byte-identical for `enable`/`disable` scenarios,
and correlated-but-not-identical for `set`/`scale` scenarios that
modify propensities from t=0. Any structural change that reorders
draws also breaks the coupling — this is paired-seed CRN, NOT
event-keyed RNG.

### Implementation phases

| Phase | Status      | Scope                                                    |
| ----- | ----------- | -------------------------------------------------------- |
| v0.1  | Complete    | Forward simulation + synthetic data generation           |
| v0.2  | Complete    | Inference: IF2 (MLE), PGAS+NUTS (Bayesian), particle filter, priors, real data input |
| v0.3  | In progress | Hierarchical priors, reporting pipelines, spatial coupling |

Public **alpha** as of 2026-05 (blog announcement): usable for real
fits, public surface documented, breaking changes still expected.

### Inference algorithms

The inference stack lives in `rust/crates/sim/src/inference/`:

- `if2.rs` — Iterated filtering for maximum likelihood estimation
- `pgas.rs` — Particle Gibbs with Ancestor Sampling (production Bayesian method)
- `pgas_grad.rs` — Gradient evaluation for PGAS (uses compiler-emitted `rate_grad`)
- `nuts.rs` — No-U-Turn Sampler for gradient-based parameter proposals within PGAS
- `pmmh.rs` — Particle Marginal Metropolis-Hastings (experimental, gated)
- `particle_filter.rs` — Bootstrap particle filter
- `dmeasure.rs` — Observation likelihood compilation
- `obs_loglik.rs` — Distribution log-PMFs + analytical gradients (incl. digamma)

The OCaml compiler (`ocaml/lib/ir/autodiff.ml`) performs source-to-source
symbolic differentiation of rate expressions, emitting `rate_grad` fields
in the IR. The Rust backend evaluates these derivative expressions via
`eval_expr` — no runtime autodiff, no finite differences.

### DSL features for inference

- `events {}` — Scheduled discrete state modifications (cohort entry,
  importation). Sister construct to `interventions {}` but fires every
  substep. Uses `add()`, `transfer()`, `set()` actions.
- `balance {}` — Population conservation constraint. Applied last in each
  substep after transitions and events.
- `ivp: true` — Parameter type for initial value parameters (s0, e0).
  PGAS draws stochastic initial states via Binomial(N, param).

### Backend capabilities

Model features constrain which backends can run them. The `Capabilities`
bitflags in `rust/crates/sim/src/lib.rs` enforce this at dispatch time:

- `OVERDISPERSION`: transitions using `overdispersed(rate, σ²)` require tau-leap
  or chain-binomial (NegBinomial draws). Gillespie and ODE reject these models
  with a hard error.
- `REAL_COMPARTMENTS`: real-valued compartments with ODE equations.

The `CompiledModel::required_capabilities()` scans the IR; each backend's
`Simulate::capabilities()` declares what it supports. Mismatch → error before
simulation starts.

### Scheduled interventions and simulation backends

Interventions are deterministic state modifications (not stochastic events).
Each backend handles them differently and the interaction is non-trivial — see
§2.3.1 of `compartmental-ir-spec.md` for the
Gillespie/tau-leap/ODE/discrete-time specifics. The key constraint: after a
Gillespie intervention, propensities must be fully recomputed from the modified
state; do not resume with remaining exponential time.

### Changing the IR schema

1. Update `ir/schema.json` + bump `ir/VERSION`
2. Update OCaml types in `ocaml/lib/ir/` (ir.ml, serialize.ml, deserialize.ml)
3. Update Rust types in `rust/crates/ir/src/`
4. `make test-unit` — fix type errors
5. `make update-golden && make update-expected` — regenerate all golden files
6. Commit schema + both language changes + updated golden files in one atomic
   commit

## Design Principles

### No loose semantics

Never silently accept invalid input. If a construct looks like it means
something, it must either mean exactly that or produce a clear error. Examples:
`_args` patterns that discard function arguments, optional fields that default
to "works but wrong." If the compiler accepts it, the behavior must be fully
specified and intentional.

### Error messages are a feature, not polish

Error quality is a first-class design goal. A bad error message is a bug —
it means the compiler detected a problem but failed to help the user fix it.

Every diagnostic should:
- Show what went wrong (the mismatch, the constraint violation)
- Show where (source location, transition name, parameter name)
- Show why (the expected vs actual value, with domain-specific names)
- Suggest a fix when possible (hint text, corrected code)

When two possible error codes could fire for the same root cause, prefer the
one that points closest to the actual mistake. E.g., a parameter used
inconsistently across transitions should produce E303 ("conflicting
dimensions in transition A vs B") not E302 ("dimension mismatch in
addition") — even though E302 is technically correct, E303 gives the user
the cross-transition context they need.

Never use `failwith` or `assert false` for user-facing errors. These produce
stack traces instead of diagnostics. Use the Diagnostics module with error
codes, source locations, and hint text.

### Design the DSL for humans first; agents follow

A meaningful fraction of `.camdl` files now come from coding agents, and
that share will grow. The temptation is to optimize the surface for
agents directly — explicit verbosity, machine-friendly tags, lots of
"obvious" guardrails. Resist it. The DSL's value to agents comes from
the *same* property that makes it value to humans: that a sharp
non-software-engineer epidemiologist (a health-ministry modeler in an
under-resourced setting, the recurring target user) can read a model
and have a chance of being right about what it does. Agents do well on
this DSL because it is human-readable, not in spite of it. When a
syntax choice is in tension between "what an agent would tolerate" and
"what a model author would understand at a glance," the model author's
gut is the tiebreaker — that is the choice that serves both audiences,
because it is the one that doesn't ask either of them to carry hidden
calendar arithmetic, ambiguous units, or implicit conventions in their
head. Concretely: prefer explicitly named functions over polymorphic
operators where the semantics differ (`add_calendar_months(d, 1)`
beats `d + 1.month` when the operation is non-affine), prefer hard
errors with hint text over warnings (warnings are noise an agent will
suppress and a non-specialist will skim), and keep the surface small
enough that the entire grammar fits in a head.

### Backwards compatibility is a non-goal

camdl is alpha: the public surface is documented but breaking changes
are still expected. Do not add backwards-compatibility shims, `alias`
attributes, fallback deserialization paths, or deprecated field names. When a
field is renamed, rename it everywhere atomically. When a format changes, update
all golden files. Clean design beats legacy support — at alpha a clean
break with updated golden files is preferred over a compatibility shim.

### Delete dead code on sight

Same principle, enforcement mechanism. Unused functions, unused modules,
"v1" paths kept around after a "v2" rewrite, prototype code kept around
"in case we need it" — all delete-on-sight. There is no consumer to
placate, no migration to stage, no contract to honour. Code that comes
back can come back from `git log -S '<symbol>'`.

- **`#[allow(dead_code)]` is a smell, not a fix.** At a definition site
  it tells a future reader "I know this is dead but didn't delete it."
  At a module level (`#![allow(dead_code)]` or
  `#[allow(dead_code)] mod foo;`) it hides *which specific items* are
  dead, blocking the compiler from reporting individual rot. Either
  prove the item is reachable from a live entry point, or delete it.
- **"v1" alongside "v2" is dead code.** When a rewrite lands, the old
  path is deleted in the same commit. Carrying both is the
  number-one source of context tax.
- **Comments saying "kept in case X" are dead code with extra steps.**
  If X happens, `git log` recovers the file in seconds. Carrying it in
  the working tree forever costs every reader.
- **Ruthlessness is collegial.** Smaller surface = humans review
  faster, agents edit faster and read less context. The reader you're
  helping most is the one six months from now (often you, often an
  agent acting on your behalf) who has to load this code into a head.

When you encounter dead code while doing other work, delete it in a
separate commit before the substantive change — review is easier when
each commit is one thing.
