# Commit & PR style

Normative guide for commit messages and merges in this repo. Derived from the
project's own history (977 commits as of 2026-05-18), not generic
conventional-commits boilerplate. When in doubt, `git log` a neighbouring change
to the same subsystem and match it.

This is the detailed spec. The load-bearing rules are summarized in the root
`CLAUDE.md`; this document is the full reference.

## Hard rules

These are non-negotiable. Evidence in parentheses.

1. **No AI / tooling trailers. Ever.** No `Co-authored-by: Claude`, no
   `Generated with …`, no `🤖`. Zero of 977 commits carry one; the practice does
   not exist here. If a squash-merge concatenates one in from a branch commit,
   strip it before confirming the merge (see _Squash merges_ below).
2. **Conventional-commits subject:** `type(scope): summary` — or the
   issue-tracked form `gh#NN: summary`. Nothing else.
3. **Imperative mood** in the subject: "add", "fix", "remove", "rename" — not
   "added", "adds", "adding".
4. **A body is the default, not the exception.** 90% of commits (881/977) have
   one. Subject-only is reserved for genuinely trivial changes (version bumps,
   typo fixes, mechanical renames).
5. **The body explains _why_ and the mechanism**, not _what_ — the diff already
   shows what. State the failure mode, the constraint, the tradeoff, the reason
   this is correct.

## Subject line

```
type(scope): summary
gh#NN: summary            # issue-tracked work (also seen: ghNN:, gh#NN vN:)
```

- **Length:** target ≤ 72 chars (median in history is 66, p90 is 79). Hard
  ceiling ~80. Occasional overflow is tolerated when a parenthetical ref earns
  it, not as a habit.
- **Lowercase** after the colon. No trailing period.
- **Parenthetical refs** are idiomatic at the end of the subject:
  `(audit M2+M4)`, `(closes gh#61)`, `(#63)`, `(Sprint 6)`.

### Types (observed frequency)

| type                                     | use                                                            |
| ---------------------------------------- | -------------------------------------------------------------- |
| `feat`                                   | new behaviour or capability (263)                              |
| `fix`                                    | bug fix; pair with a body that names the failure mode (230)    |
| `docs`                                   | documentation, including `docs/dev/*` notes (196)              |
| `refactor`                               | behaviour-preserving restructuring (81)                        |
| `test`                                   | tests only, no production change (47)                          |
| `chore`                                  | deps, tooling, housekeeping (25)                               |
| `perf`                                   | performance, behaviour-preserving (15)                         |
| `cleanup`                                | dead-code / cruft removal (11)                                 |
| `proposal`                               | a `docs/dev/proposals/` design doc (27 incl. `docs(proposal)`) |
| `incident`                               | a `docs/dev/incidents/` writeup                                |
| `revert`                                 | reverting a prior commit                                       |
| `build`, `ci`, `style`, `debug`, `spike` | as named, rare                                                 |

Parallel form: issue-tracked work often uses `gh#NN:` or `ghNN:` as the prefix
instead of a type, sometimes versioned (`gh#59 v2:`, `gh#59 follow-up:`). Use
this when the commit belongs to a tracked GitHub issue and the issue is the
better organizing key than the type.

### Scopes

The scope is the crate or module the change lives in. Observed: `fit`, `pgas`,
`cli`, `compiler`, `sim`, `inference`, `dsl`, `cas`, `expander`, `if2`, `ci`,
`web`. Use the narrowest accurate scope. Omit the scope (`feat:`, `fix:`) only
when the change genuinely spans the repo or fits no single module.

## Body

- Wrap at ~72 columns.
- Lead with the reason or the mechanism, not a restatement of the subject.
- **Multi-concern commits label their parts.** When one commit addresses several
  tracked items, head each block with its ID: `M2:` / `M4:` (audit items), or a
  one-line `## subsystem` style. Prefer one concern per commit; when batching is
  justified, make the structure explicit.
- **Lists:** numbered (`1.` `2.`) for enumerated classes of a problem or
  sequenced reasoning; bulleted (`-`) for file lists or parallel change items.
- **Cite primary sources in the body** when a change implements an algorithm or
  matches an external reference (e.g. "de Boor 1978 §X, Numerical Recipes 3rd ed
  §3.3"). This mirrors the project's scholarly-citation norm — name the
  load-bearing reference, not a decorative one.
- State the blast radius and any compatibility decision explicitly ("Field name
  unchanged so downstream readers don't migrate; semantics is now correct").

## Footers

Recognized footer lines (Key: value, at the end of the body, after a blank
line). Use the ones that apply:

| footer             | meaning                                               |
| ------------------ | ----------------------------------------------------- |
| `Proposal:`        | path to the `docs/dev/proposals/` doc this implements |
| `Audit ref:`       | path to the `docs/dev/reviews/` audit + item IDs      |
| `Fixes:`           | issue or commit this fixes                            |
| `Spec:`            | path/section of the spec this conforms to             |
| `BREAKING CHANGE:` | conventional-commits breaking marker + what broke     |

`Audit ref:` and `Proposal:` are the two most common (18 each) — audit-fix and
proposal-implementation commits are expected to point back at their governing
document.

## Squash merges

GitHub's default squash message concatenates every branch commit message plus a
`---------` separator, and drags along any trailers (including AI co-author
lines from branch commits). This is the single most common way bad messages
enter `main`.

**Always rewrite the squash message before confirming the merge.** Either edit
it in the GitHub merge box, or merge from the CLI with an explicit message:

```bash
gh pr merge N --squash \
  -t "feat(scope): summary (#N)" \
  -b "$(cat <<'EOF'
<clean body following this guide>
EOF
)"
```

The squash commit's **author** stays the PR author (GitHub preserves it); the
**committer** is whoever merges. Do not add a `Co-authored-by:` for the PR
author — they are the author, not a co-author. Never add an AI trailer (rule 1).

Subject of a squash commit follows the same `type(scope):` rule as any other
commit — not GitHub's default `Title (#N)` with no type prefix.

## Examples (from history)

A fix with a named failure mode and audit footer:

```
fix(inference): IF2 cooling off-by-one + PMMH post-burn acceptance (audit M2+M4)

M2: IF2 cooling formula used `cooling_target_iters * n_obs` for the
total step count, but each iteration consumes `(1 + n_obs)` global_step
ticks ... The (1 + n_obs) form matches the actual tick count exactly.

M4: PMMH acceptance_rate divided n_accepted by config.n_steps ...
Track n_accepted_post_burn separately and divide by (n_steps -
burn_in). Field name unchanged so downstream readers don't migrate.

Audit ref: docs/dev/reviews/2026-05-12-full-audit.md (M2, M4).
Proposal:  docs/dev/proposals/2026-05-13-pre-alpha-audit-remediation.md (Sprint 6).
```

An issue-tracked feature with primary-source citations:

```
gh#59 follow-up: proposal — proper periodic B-spline algorithm

Supersedes the v1 PeriodicSpline evaluator from ff7f8cc ...
Replaces with the standard de Boor recurrence + periodic wrap-fold
+ (degree-1)/2 centering shift. Algorithm sourced from de Boor
1978 §X, Numerical Recipes 3rd ed §3.3, Eilers & Marx 1996.

IR schema changes:
- PeriodicSpline { knots; coefs } → PeriodicSpline { n_basis;
  degree; coefs }. Uniform knots (King 2008 / P-spline standard).
```

## Code style

Code style (Rust/OCaml idioms, type-first design, no dead code, error-message
quality) is governed by the "Design Principles" and "Implementation standard"
sections of the root `CLAUDE.md`, plus `rustfmt` / `dune fmt`. This document
does not duplicate that — it covers commit and merge hygiene only.
