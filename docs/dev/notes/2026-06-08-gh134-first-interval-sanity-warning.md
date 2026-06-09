# gh#134 request 2: first-interval sanity warning — where it would go (deferred)

**Status:** deferred. gh#134's request 1 (model-side calendar nudge) was already
shipped as **W324/W325** in the 2026-05-26 typed-time Phase 1 work; this note
covers the remaining "first-interval sanity warning" ask, which is materially
more involved than the model-side nudge and lives on the Rust fit side, not in
the OCaml compiler.

## What the warning should catch

The §"Why it bites" trap in gh#134: `simulate { from = 0 }` (or any `from` well
before the first data point) against a data window that begins much later makes
the **first inter-observation interval** `[t_start, first_obs_time]` huge
relative to the modal observation cadence — e.g. a ~1000-day first window
against a 7-day weekly cadence. Two silent consequences: the model free-runs
unconditioned for that span, and the first one-step-ahead incidence prediction
accumulates over ~1000 days, wrecking the start of any prequential / fit
diagnostic. Nothing currently points at the cause.

The warning: when `first_obs_time - t_start` is `≫ K ×` the modal spacing of the
sorted observation times (sensible `K`, e.g. 5–10), warn. This is independent of
date-vs-numeric and also catches genuine data gaps.

## Verified location (the natural home)

`rust/crates/cli/src/fit/runner.rs`, in `prepare(...)` where both inputs are
already in scope and already used together:

- `t_start` is `compiled.model.simulation.t_start` (runner.rs ~line 282 / 349).
- the sorted canonical observation times are `streams[0].data` —
  `let observations = streams[0].data.clone();` (runner.rs ~line 338); the
  obs-time vector is `observations.iter().map(|o| o.time)` (already collected as
  `obs_times` at line 303 and again at 350).

There is an exact sibling precedent right here:
`check_incidence_origin_window(stream_name, &obs_model.projection, t_start,
&obs_times, first_value)`
(runner.rs:305 → `rust/crates/cli/src/util.rs:884`). That one is a _hard error_
on a zero-width first window for incidence observations. The first-interval
sanity warning is the soft-warning sibling: same inputs (`t_start`, sorted
`obs_times`), emitted in the same block (runner.rs ~line 312, just after
`check_incidence_origin_window`, or just after `let observations = ...` at 338
so it fires once on the canonical stream rather than per-stream).

Suggested shape (sketch, NOT implemented):

```rust
// gh#134 request 2: first-interval sanity warning.
// pub fn check_first_interval_window(t_start, obs_times) in util.rs,
// mirroring check_incidence_origin_window's signature/eprintln style.
//   - need >= 3 obs to have a meaningful modal spacing
//   - modal spacing = mode (or median) of consecutive diffs of sorted obs_times
//   - first_window = obs_times[0] - t_start
//   - if first_window > K * modal_spacing (K ~ 5–10): eprintln!("[warn ...] ...")
```

## Why deferred (not half-done here)

- It is Rust-side fit plumbing, not the OCaml model surface this branch touches;
  bundling it would mix two unrelated diffs.
- It sits one block away from inference setup in `runner.rs`; it must not
  perturb the existing `check_incidence_origin_window` / obs-alignment gate
  semantics. That warrants its own red→green test (a fixture with `from = 0` + a
  late, evenly-spaced data window asserting the warning; a normal first window
  asserting silence) and a `cargo test` gate — out of scope for a docs+test
  OCaml-side change.
- It needs a design call on `K` and on mode-vs-median for "modal spacing", plus
  a decision on whether it is a `[warn ...]` eprintln (like W326) or a catalog'd
  code. Worth a focused follow-up issue rather than an improvised constant.

## Naming

If it becomes a catalog'd warning it is fit-side/runtime (like W326), so it
would not be an OCaml `Wxxx` compiler diagnostic. Most likely a `[warn ...]`
eprintln in `util.rs` matching the W326 / `check_incidence_origin_window` style.
