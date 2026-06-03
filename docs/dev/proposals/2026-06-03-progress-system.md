# Consolidated progress system

Status: in progress (gh#147 follow-up)
Scope: `rust/crates/cli` progress rendering across all subcommands.

## Problem

Progress *policy* is centralized in `progress.rs` (`--progress
{auto,pretty,plain,none}` → `Resolved`; `draw_target()`, `is_plain()`,
`is_none()`, `Throttle`). Progress *rendering* is hand-rolled at each call
site, and the sites have diverged:

- `fit` IF2 / PMMH: `MultiProgress` + per-chain bars + `ll`/`acc` message.
- `profile`: `MultiProgress` + one overall bar.
- `simulate` single-cell: one per-timestep bar.
- `simulate`/`batch` multi-cell: a raw `eprintln!("[i/N] scenario=… seed=…")`
  per finished cell — no indicatif, and not gated on pretty-vs-plain, so it
  prints those lines even under `--progress auto`.
- `fit` PGAS, `pfilter`, `survey`: **no progress at all**.

So: the production Bayesian method (PGAS) and two diagnostics (pfilter, survey)
run silent; multi-cell `simulate` shows plain lines instead of a bar; and the
bar code is copy-pasted five ways.

## Design

Add a thin rendering layer to `progress.rs` so every subcommand renders the
same way and the hand-rolled `MultiProgress` blocks collapse to one API. Two
shapes — fan-out (overall bar) and per-worker iterative (N chain bars) — both
reduce to "a group of bars, each `pos/len` + a metric," so one type covers
both.

```rust
/// Owns the MultiProgress for one invocation; honors the resolved mode
/// (Pretty = bars, Plain = throttled log lines, None = silent).
pub struct Reporter { /* mp, mode */ }
impl Reporter {
    pub fn new() -> Self;                                  // draw_target() from mode
    pub fn task(&self, len: u64, label: impl Into<String>) -> Task;  // one bar
    pub fn bytes(&self, total: u64, label: impl Into<String>) -> Task; // download shape
}

/// One bar. Pretty: redraws. Plain: throttled `label pos/len <metric>` log
/// line. None: no-op. it/s + ETA come free from the shared template.
/// `inc`/`set` take `&self` (interior-mutable throttle + metric) so one bar
/// can be ticked from a parallel sweep; `Task` is `Send + Sync`.
pub struct Task { /* pb, mode, throttle, metric, label */ }
impl Task {
    pub fn inc(&self, n: u64);
    pub fn set(&self, metric: impl Into<String>);          // researcher line → {msg}
    pub fn finish(self);                                   // clears the bar
}

// The metric is a caller-formatted string (not an enum — keeps each phase's
// commit dead-code-free, since the format helper lands with its first user).
// Standardized format helpers, so the live bar reads identically everywhere:
pub fn best_ll(x: f64) -> String;          // "best ll=-12.3"  (survey / profile-search)
pub fn ll(x: f64) -> String;               // "ll=-12.3"       (IF2 / PGAS / pfilter)
pub fn mcmc(loglik: f64, accept: f64) -> String; // "ll=-12.3  acc=24%" (PMMH / PGAS-MCMC)
```

Shared style (one place): `{prefix} {bar:.cyan/dim} {pos}/{len} {per_sec}
{eta} {msg}`, chars `━╸─`. `bytes()` uses `{bytes}/{total_bytes}
{binary_bytes_per_sec}`.

### Modes

- Pretty → bars on stderr.
- Plain → `Task::inc`/`set` emit a throttled (`Throttle::default()`, 30s) log
  line `label pos/len <metric>`; no carriage returns or ANSI (tee/CI safe).
- None → all no-ops.

### `--no-progress` + auto default

`ProgressMode::Auto` is already `#[default]` and `--progress` is already
global — auto-default needs no change. Add a global `--no-progress` bool that
forces `None` and wins over `--progress`:
`progress::init(if cli.no_progress { None } else { cli.progress })`.

## Survey — which commands get what

| Command | Unit | Bars | Metric |
|---|---|---|---|
| `simulate`/`batch` (N cells) | cells | 1 overall | `Sim` (rate+ETA) |
| `simulate` (1 cell) | timesteps | keep existing inner bar | — |
| `fit` IF2 | chains × iters | N chain bars | `Loglik(best)` |
| `fit` PGAS | chains × sweeps | N chain bars | `Mcmc`/`Loglik` |
| `fit` PMMH | chains × iters | N chain bars | `Mcmc{ll,acc}` |
| `fit` NLopt | evals | 1 overall | `Loglik(best)` |
| `pfilter` | replicates | 1 | running-mean `ll` |
| `profile` | jobs | 1 overall | (none — see note) |
| `survey` | grid points | 1 overall | `Search{best}` |
| `data` (download) | bytes | 1 `bytes()` | — |
| `compile`/`check`/`inspect` | subprocess | keep spinner | — |
| `reindex` | store walk | spinner | — |
| `eval`/`list`/`show`/`cat`/`compare`/`label`/`lineage tree,sojourn,cohort` | instant | none | — |

Out of the *live* bar (too noisy / end-of-run): R̂, ESS, divergences,
per-parameter values. Those stay in each subcommand's existing end-of-run
report (`best chain: …`, `acceptance rates: …`, `loglik = … ± …`) — `finish()`
just clears the bar, it does not print a summary.

## Determinism invariant

Every bar is passive: `inc()`/`set()` never touch the RNG. The simulate inner
timestep bar already uses an RNG-free tick; the overall bar advances on cell
completion. PGAS/PMMH/IF2 bars are driven from the chain *driver* (callbacks),
never from inside the propensity/proposal math, and carry no state back into
it. A test in the engine pins that multi-cell trajectories are byte-identical
with `--progress none` vs `pretty`.

## Phasing (each its own gated commit)

0+1. **Done (5c935cf).** `Reporter`/`Task` + `--no-progress` + `simulate`/
   `batch` multi-cell overall bar (CasSink); deleted the `[i/N]` eprintln.
2+3. **Done.** Gaps filled: `survey` (best-ll bar), `pfilter` (per-replicate
   bar + running-mean ll), PGAS (per-chain bars via the runner-level
   `on_sweep` seam in `crates/cli/src/fit/pgas.rs` — the `sim` inference math
   is untouched). Migrated IF2/PMMH/profile onto `Reporter` (the metric setter
   `Task::set` + `progress::ll`/`mcmc` arrived here).
4. (optional, not done) `reindex` spinner, `data` download bar.

Two scope decisions made during implementation:
- **pfilter is per-replicate, not per-obs-window.** `sim::bootstrap_filter`
  has no progress callback, and it lives in an inference-math file; adding one
  is a separate `sim`-crate change. The per-replicate bar (only for
  `--replicates > 1`) with a running-mean `ll` is the honest CLI-level
  granularity until that lands.
- **profile carries no per-tick metric.** Each cell computes its own
  `final_loglik`; surfacing a global best would need a shared accumulator the
  bar deliberately avoids. It's an `inc`-only overall bar.

Inference files (`pgas.rs`, `pmmh.rs`, `if2.rs`) are high-risk: only a passive
bar callback is added, no change to draw order or numerics, full function read
before edit, determinism test green.
