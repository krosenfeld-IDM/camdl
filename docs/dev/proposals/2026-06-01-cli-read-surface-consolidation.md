# CLI read-surface consolidation

Date: 2026-06-01 Status: Draft — weigh after the CAS run-identity migration
lands.

## Problem

The CLI read/inspect surface has grown organically into overlapping verbs. The
recurring target user — a health-ministry epidemiologist, not a software
engineer — now has several ways to look at the same run and can't hold the
surface in their head. The proximate symptom: even the maintainer didn't recall
what `fit table` does.

The read surface today:

- **Generic browse:** `list`, `show`, `cat`.
- **Fit-specific readers:** `fit table` (cross-fit aggregator + config diffs),
  `fit summary` (single-fit detail), `fit status` (progress), `fit diff`.
- **A standalone inference verb that overlaps orchestration:** `if2` vs
  `fit run --method if2`.
- **Low-salience helpers:** `eval`, `fit new`, `fit where`.

The CAS run-identity migration is reshaping this: it makes `list`/`show`/`cat`
the primary, content-addressed read path for _every_ run kind. That is exactly
the moment the fit-specific readers become candidates for folding into the
generic surface — a fit is just a run kind, and `show <fit>` / `list --kind fit`
can carry what `fit summary` / `fit table` carry today.

## Options

**1. Fit-read cluster (`fit table/summary/status/diff`) vs `list/show/cat`.**

- (a) Fold into the generic surface: `list --kind fit` (the table), `show <fit>`
  (summary + status), `show --diff` / `compare` (config diff); delete the
  fit-specific readers.
- (b) Keep them as opinionated projections, documented for their distinct value.
- (c) Hybrid: keep only the genuinely fit-specific projection — the cross-fit
  config-diff experiment table — and fold `summary`/`status` into `show`/`list`.

**2. `if2` standalone vs `fit run`.**

- (a) Fold: make `if2` a documented thin alias for `fit run --method if2`, or
  remove it (a one-stage IF2 fit _is_ `fit run`).
- (b) Keep as a deliberate no-fit.toml quick-MLE entry, documented as such.
- Decision gate: does `if2` have feature parity with `fit run`'s IF2 path?
  Parity → alias/remove; genuinely lighter entry → keep + document.

**3. Low-salience helpers (`eval`, `fit new`, `fit where`).** Keep-or-cut each
on its merits against the "does a target user need this verb" test. `eval` is a
debug/inspection tool; `fit new` scaffolds a fit.toml; `fit where` is a
path/identity query. None are broken; each adds surface.

## Recommendation

Defer until the CAS migration lands, so the `list/show/cat` primary path is
settled. Then bias toward folding the fit-read cluster into the generic CAS
browse surface (1a/1c), keeping only projections that are genuinely distinct
(the cross-fit config-diff table is the strongest keep). Resolve `if2` by
checking fit-run parity first. The governing principle is the one the DSL
already follows: a small surface a non-software-engineer can hold in their head,
one obvious way to read a run. Breaking changes are fine at alpha; this should
be net-negative LOC and net-positive clarity, not feature work.

## Non-goals / tradeoffs

- **Not now.** Doing this mid-migration would expand in-flight work and churn
  the read surface twice.
- **Self-describing output (provenance-in-TSV) is a separate, orthogonal idea**
  — it concerns what each output file _carries_, not which verbs exist.
- Folding costs muscle-memory churn for existing scripts; acceptable at alpha
  with updated docs.

## Open questions

- Does `if2` have fit-run feature parity? (The parity check is the decision
  input.)
- What does `fit where` uniquely answer that `show`/`list` can't?
- Is `fit table`'s cross-fit config-diff view `list --kind fit --diff`, or
  distinct enough to keep its own verb?
