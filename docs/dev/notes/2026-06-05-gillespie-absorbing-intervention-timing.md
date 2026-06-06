# Question: gillespie intervention timing in the absorbing state

Date: 2026-06-05
Project: camdl
Tags: gillespie, interventions, absorbing-state, open-question

## Status

Open **question**, not an incident: there is a reproduction, but it is not yet
classified as wrong-vs-expected. Surfaced while wiring gillespie through the
merged `Schedule` (the unified-timeline Stage-1 refactor); the behaviour is
**pre-existing** — old and new code produce byte-identical trajectories (verified
below), so it is not introduced by the refactor and is out of scope for it.

## Reproduction

A fast decay that burns out early, then an intervention scheduled `at [10]`:

```
# /tmp/absorb_check/decay.camdl (closed A --> B, k=1.0, A0=3)
transitions { decay : A --> B  @ k * A }
interventions { restart : add(A, 2) at [10] }
simulate { from = 0 'days to = 20 'days }   # dt defaults to None -> 1.0
```

```
camdl simulate decay.ir.json --backend gillespie --seed 1 --enable restart --stdout
t  A  B  flow_decay
0  3  0  0
1  1  2  2
2  0  3  1
3  2  3  0     <- A gains +2 here (B unchanged): the `add(A,2)` effect appears
4  1  4  1        at t≈3, NOT at the scheduled t=10
5  0  5  1
6  0  5  0
... (A stays 0 through t=20)
```

The `add(A, 2)` effect is visible at t≈3, while the intervention is scheduled at
t=10. Expectation (unconfirmed) is that it should apply at t=10.

## Old == new (byte-identical, the load-bearing fact for the refactor)

Combined trajectory hash over seeds 1–6, version-header excluded:

```
OLD (HEAD gillespie, inline boundary fold):  fc2ed87b…d228b748
NEW (routed through Schedule + clip):        fc2ed87b…d228b748   # identical
```

So whatever this behaviour is, the Stage-1 `Schedule` wiring preserves it
exactly. The question is about the underlying gillespie semantics, independent of
the refactor.

## What to check next (when bugs are back in scope)

- Trace the absorbing branch (`gillespie.rs`, `lambda_total <= 0.0`): it computes
  `next_special = min(t_end, next_output, next_effect)`, flushes outputs up to
  `next_special`, then jumps `t` to the next effect time and applies it. Confirm
  whether the jump lands on t=10 (and the early-looking `add` is an output-
  recording artifact of post-jump state) or whether the effect is genuinely
  applied early.
- Distinguish from gh#95 (inhomogeneous-Poisson bias) — this is intervention
  *timing* under absorption, a different surface.
- If genuinely wrong: TDD repro (assert A unchanged until t=10), then fix; pin
  with a gillespie absorbing-state baseline.

## Not doing now

Per the maintainer's directive (land the unified scheduler + obs uplift first;
all bugs wait), this is logged and deferred — not investigated or fixed here.
