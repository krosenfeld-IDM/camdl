# Gillespie silent-wrong: known bugs sidestepped in tests, missed by final-state-only cross-backend comparison

Date: 2026-06-16 Issues: gh#70 (absorbing-state flush + reverse-time boundary),
gh#208 (sparse propensity negative-rate clamp) Status: bugs open + `blocker`;
fixes pending. This report is about the _process_ failure that let two
silent-wrong bugs persist undetected.

## Summary

A feature/bug triage pass surfaced two Gillespie backend bugs, both **silent
wrong-output** on the simulation path:

- **gh#70** — with an absorbing initial state (`I0 = 0`) plus a scheduled
  importation (`add(I, 5) at [10]`), the event is recorded at t=2 instead of
  t=10 and **trajectory time runs backward** (the loop jumps t: 10 → 2).
- **gh#208** — a transition rate that goes negative on a _sparse_ propensity
  update is silently clamped to 0 (the transition vanishes), where every other
  backend and Gillespie's own _full_ propensity path raise `NegativePropensity`.

Both were reproduced empirically (concrete input → wrong output, commands in the
issues). Neither is a regression introduced by a recent change — both are
long-standing, and the unified-timeline rewrite carried gh#70's semantics
forward verbatim.

The uncomfortable finding: **gh#70 was already known, documented in test
comments, and deliberately routed around — not missed.** And the cross-backend
agreement test that _should_ have caught it was scoped to the one data point the
bug doesn't corrupt. This document records why, because the gap is a process
gap, not a clever-bug gap.

## How it was detected

A triage subagent auditing the open `kind/bug` backlog against current `main`
flagged gh#70's static signature; a follow-up verification agent built `camdl`
and reproduced both bugs end-to-end against the live binary, confirming the
wrong output and root-causing each to a specific line.

## Root cause of the _miss_ (the part that matters here)

Cross-backend comparison **did exist** — the bug was not invisible. It was
under-scoped and, in one place, explicitly disabled.

### 1. The cross-backend test asserts only the final snapshot

`rust/crates/sim/tests/cross_backend_lifecycle_agreement.rs` compares Gillespie,
chain-binomial, and ODE on a lifecycle model — but the `final_a_b` helper probes
**only `traj.snapshots.last()`**, with the comment (verbatim):

> gillespie's absorbing-state output cadence back-fills earlier output rows
> differently (it jumps to the t=5 boundary), but the terminal state is the
> canonical post-lifecycle state on every backend.

So the comparison ran, passed, and was _correct about the terminal state_ — but
the terminal state is precisely the row that time-reversal does not corrupt. A
backend can disagree on every intermediate row and the timing of events and
still converge to the same final compartment counts. Final-state agreement is a
necessary, not sufficient, cross-backend invariant.

### 2. A second test sidesteps the bug by construction

`rust/crates/cli/tests/events_backend_parity.rs` carries the comment:

> gillespie has a separate pre-existing absorbing-state output-flushing bug we
> sidestep here

and seeds `I = 1` so the model is never absorbing at t=0 (avoiding the trigger),
sampling I only at t=9 / t=11 (skipping the corrupted intermediate rows). The
bug was known _at the time this test was written_, named in a comment, and
stepped around rather than filed and pinned.

### 3. No trajectory-time invariant exists

Grepping every test crate for a monotonicity or intermediate-row invariant
(`monoton`, `backward`, `windows(2)`, non-decreasing snapshot `t`) finds nothing
that asserts a trajectory's snapshot times are non-decreasing, or that a
snapshot's recorded state matches the time it claims. The single thing that
would catch gh#70 directly is absent.

### 4. gh#208's sparse path is never exercised with a negative rate

`rust/crates/sim/tests/gillespie_invariants.rs::test_propensity_non_negativity`
asserts `p >= 0.0` — but only on the **full** `eval_propensities` path, on
hand-picked SIR states where rates are naturally non-negative. It never
constructs a model whose rate crosses zero, and never touches `eval_one`'s
sparse path (`gillespie.rs:53`) where the silent `.max(0.0)` clamp lives. The
test that looks like it guards this invariant guards the wrong code path.

## Why "comparison with other backends" didn't catch it — directly

It did compare; it compared the wrong thing. Two failure modes compounded:

- **Scope.** Agreement was checked on the final state only. Both bugs preserve
  (or can preserve) the terminal state — gh#70 corrupts intermediate rows;
  gh#208 is seed-dependent, so a single-seed final-state check can land on a
  seed where the negative rate never bit. Cross-backend equivalence has to be
  **full-trajectory and multi-seed** to be load-bearing.
- **Opt-out.** Where a backend was _known_ to disagree, the disagreement was
  encoded as a test comment + a routed-around fixture, which reads as
  "documented" but functions as "suppressed." A known divergence between
  backends is a bug report, not a test annotation.

## Remediation

**The two fixes are independent** (different subsystems — gh#70 is in the
time/boundary control flow, gh#208 in propensity evaluation), so they land as
**separate commits**, each with its own red→green. But they belong to **one
hardening effort**, because they share the test infrastructure that should have
caught them:

1. **gh#70** — flush outputs through to the effect time in the absorbing branch
   (`gillespie.rs:173-191`); add a `> t` filter to the _output_ candidate in
   `Schedule::clip` (`schedule.rs:298-299`) mirroring the effect filter. _Risk:
   `clip` is shared spine code used by all backends — gate with the
   cross-backend full-trajectory test below + the existing byte-identity gates._
2. **gh#208** — make `eval_one` `Result`-returning; reject negative / NaN / Inf
   with `NegativePropensity { transition, value, t }`, consistent with
   `eval_propensities`; thread through the three sparse-update call sites.
   _Localized, low-risk._

**The systematic guard — a cross-backend full-trajectory equivalence harness.**
The single highest-leverage change is a test that asserts Gillespie, ODE, and
chain-binomial agree on the **entire** trajectory (every snapshot row, not just
the last) across a battery of models and multiple seeds — and a separate
**trajectory-time-monotonicity invariant** (snapshot `t` strictly
non-decreasing; recorded state consistent with its time). Either of these would
have failed on gh#70 on day one. This harness is the durable artifact; the two
point fixes are downstream of it.

## What this changes (process)

- **A test that documents a backend disagreement must file it, not annotate
  it.** When a test routes around a known divergence with a comment, that is a
  silent-wrong bug being suppressed at the test layer. Replace the comment with
  an `#[ignore = "gh#NN"]` / failing-test-pinned-to-an-issue, so the suppression
  is visible and tracked, never load-bearing.
- **Cross-backend agreement is full-trajectory + multi-seed, or it isn't an
  agreement.** Final-state-only comparison is a weak invariant that both of
  these bugs slipped under.
- **Gillespie's hand-rolled incremental paths are the standing risk.** The
  backend re-implements, incrementally, what the authoritative full path
  computes — sparse propensity updates (gh#208) and absorbing-state output
  flushing (gh#70). Both bugs are instances of "the optimized incremental path
  silently disagrees with the full path." The full-trajectory cross-backend gate
  is the right systematic defense against the whole class.
