# Proposal: Unified Diagnostic Surface

**Status:** draft for discussion. **Scope:** extend the existing
`rust/crates/sim/src/inference/diagnostic.rs` typed diagnostic surface from
inference-only to project-wide. All camdl subcommands (`simulate`, `fit`,
`survey`, `pfilter`, `profile`, `eval`, etc.) route their pre-flight checks,
config-load validation, backend selection notices, parameter-override info, and
runtime warnings through one structured
`Diagnostic { kind, severity, message,
stage, timestamp }` channel. The
collector fans output to CLI stderr in real-time, to `run_meta.json` for
programmatic consumption, and to the per-fit `fit_summary.json` where
applicable. **Primary application:** the survival-conditioning corner-divergence
guard (proposal 2026-05-23 §3.4) and the gh#71 stuck-chain warnings both want to
emit structured, severity-tagged diagnostics. Doing them through ad-hoc
`eprintln!("error: …")` works but means re-implementing severity rendering, JSON
serialization, and CLI prominence per subsystem. We have the infrastructure
already; we just haven't used it project-wide.

> **Provenance.** This RFC was forced by two adjacent in-flight pieces of work
> (survival-conditioning §3.4 guard and gh#71 stuck-chain proposal §5.2) both
> proposing slightly different versions of the same "structured,
> severity-tagged, CLI-rendered diagnostic" pattern. Rather than ship each
> separately and re-converge later, it makes sense to consolidate now.
> Verification target: read `rust/crates/sim/src/inference/diagnostic.rs`
> end-to-end before reviewing — the type system this RFC extends is concrete and
> already-tested. Citation discipline (per CLAUDE.md): the existing
> `DiagnosticKind` has ~20 variants already; this RFC adds categories, not a
> parallel system.

---

## Summary

Camdl already has a well-designed structured-diagnostic primitive at
`rust/crates/sim/src/inference/diagnostic.rs`: a tagged `DiagnosticKind` enum
with `Severity::{Info, Warning, Error}`, serde-derived JSON, machine-readable
variant names, and ~20 typed variants for inference-time concerns (`RhatHigh`,
`LowESS`, `ParamNearBound`, etc.). It is consumed by camdl-book, CI, and
vignette regression tests today.

But it is **inference-only**. Pre-flight checks, config-load validation, backend
selection notices, parameter-override info, and runtime warnings all use ad-hoc
`eprintln!("error: …")`, `eprintln!
("[info] …")`, and `log::info!`/`warn!`
patterns scattered across the CLI. Three concrete consequences:

1. **The same diagnostic shape gets re-implemented per subsystem.** The survival
   corner-divergence guard (`min_takeoff_probability`) and the gh#71 stuck-chain
   proposal both want "structured error message + severity + remedy text +
   machine-readable code"; each re-implements the pattern from scratch.
2. **Programmatic consumption is patchy.** Inference diagnostics appear in
   `fit_summary.json`; backend-auto-match info appears only on stderr;
   config-load errors print and exit. Tooling (camdl-book agents, CI, downstream
   notebooks) has to scrape stderr or guess.
3. **CLI rendering is inconsistent.** The existing convention has `error:` and
   `[info]` prefixes but no central control over ordering, colour, or
   prominence. The gh#71 incident's "acceptance-rate warning dismissed as
   slightly hot" failure mode is partly about this: warnings disappear into a
   noise floor that has no severity hierarchy.

This RFC proposes:

- **Extend `DiagnosticKind`** with new variant categories for pre-flight checks
  (`ConfigLoadFailure`, `IncompatibleStageCombo`,
  `MinTakeoffProbabilityViolated`, etc.), backend selection
  (`BackendAutoMatched`, `ParameterOverride`), and project-wide runtime events
  (`SimulationStarted`, `StageBoundary`).
- **Lift `DiagnosticCollector`** from inference-only to a top-level
  `camdl::Diagnostics` registry that any subcommand can push to.
- **Fan output to three destinations**: CLI stderr (real-time, severity-coded
  rendering), `run_meta.json` (post-run, programmatic), and `fit_summary.json`
  (fit-specific, programmatic). Each subcommand controls which destinations it
  writes to.
- **Reorder CLI rendering by severity**: Error-severity diagnostics print at the
  top of output with a banner, Warning after tables, Info dimmed in-line. The
  gh#71 §5.2 CLI re-ordering becomes a special case of this general rule.

We do **not** propose breaking the existing `inference::diagnostic` surface —
adding categories to `DiagnosticKind` is backwards-compatible by construction
(downstream deserializers either know the new variants or treat them as unknown
tag values). Migration is opportunistic: each subsystem moves its ad-hoc
warnings to the new surface when it's touched for other reasons. The
corner-divergence guard and the gh#71 stuck-chain warnings are the first two
real users; everything else gets retrofitted over time.

**Cost-benefit framing:** the RFC adds ~30 new `DiagnosticKind` variants
(mechanical), a top-level collector struct (~150 LoC), and a CLI rendering pass
(~200 LoC). Saves re-implementing the same pattern in the survival-corner-guard
and gh#71-stuck-chain work, and gives camdl one place to enforce "be loud about
real problems" discipline rather than scattering it across subcommands.

---

## 1. The current fragmentation

### 1.1 What we have

Three parallel diagnostic surfaces in camdl today:

**(a) Structured inference diagnostics.**
`rust/crates/sim/src/inference/diagnostic.rs` defines:

```rust
pub enum Severity { Info, Warning, Error }

pub struct Diagnostic {
    pub kind: DiagnosticKind,
    pub severity: Severity,
    pub message: String,
    pub stage: String,
    pub timestamp: String,
}

#[serde(tag = "type", rename_all = "snake_case")]
pub enum DiagnosticKind {
    RhatHigh { param, rhat, threshold },
    LowESS { obs_time, ess, n_particles, ess_fraction },
    AcceptanceRateUnhealthy { rate, param },
    ParamNearBound { param, value, bound, bound_type },
    // ... ~20 variants total
}

pub struct DiagnosticCollector { /* push, drain, serialize */ }
```

Used by PMMH, PGAS, IF2, profile, NUTS. Serialized into `pmmh_summary.json`,
`pgas_summary.json`, `if2_summary.json`. Consumed programmatically by camdl-book
regression tests, vignette CI, and the fit-report renderers.

**(b) Ad-hoc CLI eprintln patterns.** Scattered across `crates/cli/src/`:

```rust
eprintln!("[info] backend auto-matched to {} (dt={}) from fit \
    provenance in {}. Pass --backend explicitly to override; ...",
    backend, dt, path.display());

eprintln!("error: --rho must be in [0, 1). Got: {}", r);
std::process::exit(1);

eprintln!("[2026-05-24T04:10:01Z INFO  camdl::util] \
    --params /var/.../mle_gill.toml: mu=0.05 overrides previous \
    value 0.1");
```

No common structure, no severity hierarchy, no JSON output, no ordering control.

**(c) `log::info!` / `log::warn!` macros.** Used in `crates/sim/src/inference/`
for per-iteration diagnostics during inference (PF skipped observations, density
mismatches, etc.). Gated behind `RUST_LOG=camdl_sim=debug`. Not exposed to
end-users by default.

### 1.2 The cost of fragmentation

Three concrete failure modes:

1. **gh#71 incident**: the acceptance-rate warning (severity=warning) fired in
   the seed-timing fit but was "dismissed as 'slightly hot'." It surfaced via
   (a) — inference diagnostics — and made it into `pmmh_summary.json`, but with
   no CLI prominence. The dismissal was partly a rendering problem.
2. **Survival corner-divergence guard** (proposal 2026-05-23 §3.4): the v1
   implementation routes through (b) —
   `eprintln!("error:
   …"); std::process::exit(1)`. The message text is
   well-structured, but it doesn't appear in `run_meta.json` or any post-run
   programmatic surface. A camdl-book agent regression-checking that a model
   refuses correctly has no JSON to assert on; it has to parse stderr.
3. **Diagnostic-shape drift**: each subsystem invents its own remedy text
   format, severity-emoji conventions, etc. The `gh#71 stuck-chain` proposal
   §5.2 separately argues for "Error diagnostics print first, Warning after
   tables." Same idea as (a)'s existing rendering but proposed in isolation —
   because there's no project-wide channel to attach the rule to.

### 1.3 Why now

The next two in-flight pieces of work both want this:

- **Survival corner-divergence guard** has the structured error message ready
  (proposal 2026-05-23 §3.4 / math doc §4.5 / the `min_takeoff_probability`
  field landed in commit `1ef1f26` on the worktree). The text is written for a
  future retrofit; the v1 implementation ships through (b). Migrating to the
  unified surface is one of the next steps.
- **gh#71 stuck-chain proposal** specifies three new diagnostic kinds
  (`SingleInitWithMultipleChains`, `LowEssWithConvergedRhat`,
  `AllChainsLowDrift`) and a CLI rendering update (§5.2). Building these against
  the existing `inference::diagnostic` surface is natural; the §5.2 rendering
  change becomes a general rule, not a PMMH/PGAS-specific tweak.

Doing the consolidation now means both of those features ship on the new surface
from day one, rather than landing on the legacy patterns and being retrofitted
later.

---

## 2. Design

### 2.1 Architectural shape

Promote `inference::diagnostic` to a top-level `crate::diagnostics` module
(location debated in §6) with:

```rust
pub enum Severity { Info, Warning, Error }
                            // ^ unchanged

pub struct Diagnostic {     // unchanged shape; new variants
    pub kind: DiagnosticKind,
    pub severity: Severity,
    pub message: String,
    pub subsystem: String,  // generalised from `stage`
    pub timestamp: String,
}

pub enum DiagnosticKind {   // existing 20 variants + new categories
    // ── Convergence (existing) ──────────────────────────────────
    RhatHigh { … },
    LowESS { … },
    AcceptanceRateUnhealthy { … },
    // ... unchanged

    // ── Config-load / pre-flight (new) ───────────────────────────
    /// A required TOML field is missing or invalid.
    ConfigInvalid { section: String, field: String, reason: String },
    /// Two stage settings are mutually exclusive.
    IncompatibleStageCombo {
        stage: String,
        a: String, b: String, reason: String,
    },
    /// Survival-conditioning corner-divergence guard tripped.
    MinTakeoffProbabilityViolated {
        stage: String,
        method: String,    // "analytic_sir" etc.
        p_takeoff: f64,
        floor: f64,
        suggested_lo: f64,
        suggested_p: f64,
    },
    /// gh#71 single-init + multi-chain warning.
    SingleInitWithMultipleChains {
        stage: String,
        n_chains: usize,
        algorithm: String,
    },
    /// gh#71 R̂-converged-but-low-ESS conjunction.
    LowEssWithConvergedRhat {
        stage: String,
        parameters: Vec<LowEssParam>,
        max_rhat: f64,
    },
    /// gh#71 all-chains-low-drift signature.
    AllChainsLowDrift {
        stage: String,
        parameter: String,
        drift_ratios: Vec<f64>,
        threshold: f64,
    },

    // ── Backend / configuration (new) ────────────────────────────
    /// Backend auto-matched from fit provenance.
    BackendAutoMatched {
        subcommand: String,
        backend: String,
        dt: f64,
        source: String,        // path to mle_*.toml
    },
    /// CLI --param override of an earlier value.
    ParameterOverridden {
        param: String,
        new_value: f64,
        old_value: f64,
        source: String,        // path / "CLI argv"
    },

    // ── Runtime info (new) ───────────────────────────────────────
    /// Stage transition; the lifecycle event a CLI banner anchors on.
    StageBoundary {
        stage: String,
        phase: String,         // "started" | "completed" | "failed"
        wall_clock_s: Option<f64>,
    },
}
```

The collector becomes top-level:

```rust
pub struct Diagnostics {
    items: Vec<Diagnostic>,
    sinks: Vec<Box<dyn DiagnosticSink>>,
}

pub trait DiagnosticSink {
    fn emit(&self, d: &Diagnostic);
    fn finalize(&self, all: &[Diagnostic]) -> Result<(), io::Error>;
}

impl Diagnostics {
    pub fn push(&mut self, d: Diagnostic) {
        for sink in &self.sinks {
            sink.emit(&d);
        }
        self.items.push(d);
    }

    pub fn finalize(&self) -> Result<(), io::Error> {
        for sink in &self.sinks {
            sink.finalize(&self.items)?;
        }
        Ok(())
    }
}
```

Three concrete `DiagnosticSink` implementations:

- `CliSink` — renders to stderr in real-time, severity-coded.
- `RunMetaSink` — accumulates to a `diagnostics` array in `run_meta.json` at
  finalize.
- `FitSummarySink` — accumulates to `pmmh_summary.json` / `pgas_summary.json` /
  `if2_summary.json` as today; just generalised across stages.

A subcommand wires the sinks it needs:

```rust
// camdl simulate: CLI + run_meta only (no fit_summary; not a fit)
let diagnostics = Diagnostics::with_sinks(vec![
    Box::new(CliSink::new(stderr_handle)),
    Box::new(RunMetaSink::new(&run_meta_path)),
]);

// camdl fit: all three
let diagnostics = Diagnostics::with_sinks(vec![
    Box::new(CliSink::new(stderr_handle)),
    Box::new(RunMetaSink::new(&run_meta_path)),
    Box::new(FitSummarySink::new(&fit_dir)),
]);
```

### 2.2 CLI rendering rules

`CliSink` enforces a consistent severity-aware render order. The core rule,
adapted from gh#71 §5.2 and generalised:

- **Errors render at top of stderr with a banner**, before any tables or other
  output, regardless of when they're pushed:
  ```
  ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
  ERROR (min_takeoff_probability_violated)
  ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
  survival-conditioning config-load check failed for stage 'posterior'.
    ...
  ```
  Plus exit code 1 if the subcommand is config-load-style; warnings-only runs
  exit 0.

- **Warnings render after main output**, in a section labelled with the count:
  `2 warnings — see below`. Each warning gets a yellow prefix.

- **Info renders in-line** at the timestamp of emission, dimmed (subdued colour)
  so it doesn't compete with primary output. Today's
  `[info] backend auto-matched ...` continues to read the same way; it just gets
  a `DiagnosticKind::BackendAutoMatched` provenance tag for free.

- **Verdict line is last.** For fit subcommands, the last line of stderr is
  `verdict: PASS | WARN | FAIL`, so a user grepping the tail lands on the
  bottom-line gate.

This is what gh#71 §5.2 already proposed for PMMH/PGAS; this RFC makes it the
project-wide rule.

### 2.3 Backwards compatibility

Three layers:

1. **`DiagnosticKind` is
   `#[serde(tag = "type", rename_all =
   "snake_case")]`** and downstream
   deserializers either know a variant or treat it as an unknown tag. Adding
   variants doesn't break old consumers; they just don't see the new categories.
   **Adding is safe.**
2. **`Diagnostic.stage` → `Diagnostic.subsystem` rename**: this is the one
   schema-breaking change. Two options:
   - (a) Keep `stage`, add `subsystem` as an alias via
     `#[serde(alias = "stage")]`. Backwards-compatible on read; legacy writers
     continue working.
   - (b) Bump `fit_summary.json`'s `summary_schema_version` from
     whatever-it-is-today by one, write both fields during a transition period,
     drop `stage` after one minor release. Strong preference for (a) — it's
     cheaper and the field is only labeled differently anyway.
3. **Ad-hoc eprintln! migration**: not a forced break. Each subsystem keeps
   emitting via `eprintln!` until it's touched for other reasons, at which point
   the migration is opportunistic. The two committed first users (survival
   corner-divergence + gh#71 stuck-chain) seed the pattern; everything else
   follows naturally.

---

## 3. Migration plan

Three phases, scoped so each can land independently:

### Phase 1 — Promote `inference::diagnostic` to top-level (3–5 days)

- Move `rust/crates/sim/src/inference/diagnostic.rs` to a new top-level module.
  Module path debated in §6 — current preference is
  `rust/crates/diag/src/lib.rs` as a new crate, so both `sim` and `cli` depend
  on it without inversion.
- Generalise `Diagnostic.stage: String` → `Diagnostic.subsystem:
  String` with
  `#[serde(alias = "stage")]` for backwards compat.
- Build the `DiagnosticSink` trait and the three default implementations
  (`CliSink`, `RunMetaSink`, `FitSummarySink`).
- Existing inference call sites continue to push the same variants; the only
  change is the import path and the field name.
- **Tests**: existing inference-diagnostic tests must pass byte- identical. New
  tests for the sink trait + CLI rendering ordering.
- **No new variants** in this phase; pure refactor.

### Phase 2 — Add new variant categories (2–3 days)

- Add the `Config / pre-flight`, `Backend / configuration`, and `Runtime info`
  variants from §2.1.
- Wire two flagship consumers:
  - **Survival corner-divergence guard**: migrate the `eprintln!("error: …")` in
    the worktree's `validate_survival_conditioning` to
    `diagnostics.push(MinTakeoffProbabilityViolated { … })`. The error message
    text is already structured per the proposal §3.4 — becomes the `message`
    field; the typed payload provides programmatic access.
  - **Backend auto-match**: migrate the existing
    `eprintln!("[info] backend auto-matched to …")` calls to
    `diagnostics.push(BackendAutoMatched { … })`. No user-visible change in CLI
    output (the `CliSink` renders Info exactly as today); but the diagnostic now
    appears in `run_meta.json`.
- **Tests**: per-variant snapshot tests for the rendered output. Confirm the new
  variants are picked up by `RunMetaSink` / `FitSummarySink` correctly.

### Phase 3 — gh#71 stuck-chain warnings on the new surface (concurrent)

- Implement the gh#71 stuck-chain proposal directly against the new surface —
  three new variants (`SingleInitWithMultipleChains`, `LowEssWithConvergedRhat`,
  `AllChainsLowDrift`), all routed through `Diagnostics::push`.
- The §5.2 CLI re-ordering from the gh#71 proposal becomes a no-op — the unified
  surface enforces severity ordering by default.

Phases 1 and 2 land sequentially; Phase 3 lands concurrently with Phase 2
(different files, no architectural conflict).

---

## 4. Validation

### 4.1 Existing inference diagnostics unchanged

The Phase 1 refactor must produce byte-identical `pmmh_summary.json` /
`pgas_summary.json` / `if2_summary.json` output for a fixed fit. The migration
test runs a known synthetic fit before and after the refactor and asserts the
JSON is unchanged modulo whitespace.

### 4.2 New variants serialise stably

Per-variant snapshot test: build each new `DiagnosticKind` variant with a
fixture payload, serialise to JSON, snapshot. Reviewers can read the snapshot
file as the public schema contract. Same pattern as the existing
inference-diagnostic tests.

### 4.3 CLI rendering ordering

End-to-end test: spin up a synthetic fit that emits one Info, two Warnings, and
one Error in non-sorted-emission order. Capture stderr, assert: Error at top
with banner, Warnings after the (empty) primary output table, Info dimmed,
verdict on the last line.

### 4.4 Survival corner-divergence guard migration

The corner-divergence guard's current `eprintln!("error: …")` text ships
verbatim as the `message` field of the new `MinTakeoffProbabilityViolated`
variant. The unit test that snapshots the error message text continues to pass;
a new integration test asserts that the variant also appears in `run_meta.json`.

### 4.5 Backend auto-match doesn't regress

The `[info] backend auto-matched ...` message text is preserved verbatim in the
CLI render. The existing CLI-output snapshot tests (e.g. `seed_timing_e2e.rs`)
must continue to pass.

### 4.6 No new failure modes from the rename

`Diagnostic.subsystem` aliased from `stage` — a test deserialises a known-good
legacy `pmmh_summary.json` from the `ir/golden/` corpus (or equivalent) and
confirms the alias resolves cleanly.

---

## 5. Caveats and non-goals

### 5.1 We are not building a logging framework

This is a **diagnostic** surface, not a logging framework. The distinction:

- Diagnostics are end-user-facing, severity-typed, structured for programmatic
  consumption, and intended to be rendered prominently (or hidden depending on
  severity). Bounded inventory (~50 typed variants).
- Logging is developer-facing, free-text, controlled by `RUST_LOG`, unbounded
  inventory.

`log::info!`/`warn!` macros stay where they are — they're for debugging, not for
user-facing diagnostics. The `[info] backend
auto-matched ...` line that
masquerades as a log entry today is actually a diagnostic and gets migrated; the
per-PF-step internal `log::debug!` calls in `pgas.rs` stay as logging.

### 5.2 We are not forcing migration

Each subsystem migrates when it's touched. The "diagnostic-shape-drift" problem
self-corrects over time as new code gets written against the unified surface and
old code gets touched during normal maintenance. A hard migration would be a lot
of churn for no acute benefit.

### 5.3 No new severity levels

Three levels (`Info`, `Warning`, `Error`) is enough. Adding `Debug`, `Trace`,
`Critical`, etc. invites scope creep into logging territory (§5.1). If we
discover a real need we add later; v1 stays simple.

### 5.4 Verdict gating is orthogonal

The gh#71 stuck-chain proposal §4.4 introduces verdict-gating (`Severity::Error`
flips fit verdict PASS → FAIL). That's a separate concern from "should this be a
structured diagnostic." Both proposals work; the verdict gate hooks into the
`FitSummarySink` finalize step.

### 5.5 What we're not consolidating

- `panic!` / `assert!` calls — those are programmer errors, not user-facing
  diagnostics. They stay panics.
- Per-iteration `log::debug!` calls inside the PF / NUTS loops — those are
  debugging, not diagnostics (§5.1).
- The OCaml-side compile errors (`Diagnostics` module in OCaml) — separate code
  path, separate concern, separate proposal if we ever want to unify Rust-side
  and OCaml-side diagnostic surfaces.

### 5.6 Decisions for RFC review

1. **Crate location**: top-level `rust/crates/diag/` (new crate) vs
   `rust/crates/sim/src/diagnostic.rs` (in `sim` but lifted out of
   `inference::`) vs new `rust/crates/cli/src/diagnostics.rs` (in CLI).
   Crate-vs-module is the substantive decision; module path is bikeshed.
2. **Renaming `stage` → `subsystem`**: confirm the alias-based path is
   acceptable, or do we bump `summary_schema_version` and migrate harder?
3. **CLI banner styling**: ASCII heavy lines (the `━━━━━` in §2.2) work in
   modern terminals but look ugly in some CI logs. Confirm the heavy-line banner
   is OK, or go subtle (single `---` line)?
4. **Severity inflation**: with this RFC, the survival corner- divergence guard
   goes from "error+exit" to "structured Error diagnostic." Confirm the exit
   code semantics: any `Severity::
   Error` pushed during config-load means
   exit 1 at finalize, never silently continued. The existing
   `inference::diagnostic` does _not_ gate on Error; this RFC changes that for
   the config-load sites.
5. **Migration cadence**: do we sprint Phase 2 + Phase 3 in parallel, or land
   Phase 2 first and then re-survey the codebase? Strong preference for parallel
   (different files).

---

## 6. Implementation cost estimate

- **Phase 1** (refactor + sink trait): 3-5 days. Mostly mechanical; the test bar
  is the byte-identical-JSON-output guard.
- **Phase 2** (new variants + two flagship consumers): 2-3 days. Each variant
  ~30 LoC including its render and tests.
- **Phase 3** (gh#71 implementation on the new surface): 2-3 days for the three
  diagnostics + the §7.1 incident-replay tests. This cost is _already in the
  gh#71 plan_; doing it on the new surface costs the same as doing it on the
  old.

Total: ~8 days, with Phases 2 and 3 parallelisable. The not-immediately-obvious
benefit: every subsequent diagnostic that would have shipped with bespoke
`eprintln!` infrastructure now ships with structured output for free, so the
marginal cost of "add a diagnostic" goes down monotonically from here.

---

## 7. References

External:

- **Severity conventions**: rough consensus across `log` crate, `tracing` crate,
  `syslog` (RFC 5424) — three to five levels with `Info` / `Warning` / `Error`
  always present. We follow that convention.

Internal (read before reviewing):

- `rust/crates/sim/src/inference/diagnostic.rs` — the existing surface this RFC
  extends. ~700 LoC, well-commented.
- `docs/dev/proposals/2026-05-23-survival-conditioned-likelihood.md` §3.4 — the
  corner-divergence guard error message format that becomes the
  `MinTakeoffProbabilityViolated` variant.
- `docs/dev/proposals/2026-05-24-stuck-chain-diagnostics.md` §5.2 — the CLI
  re-ordering proposal that becomes a special case of §2.2 of this RFC.
- `docs/methods/survival-conditioning.md` §4.5 — the criticality regime
  discussion; the diagnostic text lives in this proposal but the math is
  documented there.

---

## Notes on this document

- Provenance dates verified against the worktree commit history. The survival
  corner-divergence guard error message text in §4.4 is the exact text from
  commit `1ef1f26` (worktree `agent-a49cbecf3be6615b5`), preserved verbatim for
  the migration.
- Existing `DiagnosticKind` variant inventory in §1.1 was read from the head of
  `inference/diagnostic.rs` (line 35–~125) — full inventory is longer; only the
  representative excerpt is quoted.
- This proposal is _consolidating_, not _novel_. The structured diagnostic
  pattern already exists in the codebase; the gh#71 §5.2 CLI re-ordering is
  already proposed for stuck-chain warnings. This RFC just argues "do both
  consistently across the project, not just for two subsystems."
