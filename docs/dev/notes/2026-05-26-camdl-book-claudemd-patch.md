# Maintainer-handoff: rewrite `camdl-book/CLAUDE.md:642-661`
#   (synthetic-recovery rule under CLI UX rev 2)

Date: 2026-05-26
Project: camdl
Tags: cli-ux, camdl-book, handoff
Related: gh#83, gh#85; `docs/dev/proposals/2026-05-25-cli-init-and-params-ux.md`
         §"Blast radius" (load-bearing prose rewrite #1)
Author: cli-ux-rev2 step 11.3

## Context

The 2026-05-25 CLI UX revision removed `--params` from inference
subcommands (profile, if2, fit run, survey). The downstream
`camdl-book` repo (separate repository at
`/Users/vsb/projects/work/camdl-book/`) has a load-bearing
pedagogy section in its top-level `CLAUDE.md` that documents the
old flag names. The teaching point — "starting an optimizer from
truth is data leakage; evaluating the likelihood at truth is fine"
— is still correct, but the worked examples reference flags that
no longer exist on inference subcommands.

This note is the maintainer's worked patch. The work was not
applied directly because `camdl-book` is a separate repo, and the
camdl worktree does not have permission to push there.

## File and lines

`/Users/vsb/projects/work/camdl-book/CLAUDE.md`, lines 685–722
(the `## Synthetic recovery: NEVER start IF2 (or any optimizer)
from truth` section). The proposal cites this as
"`camdl-book/CLAUDE.md:642-661`" — that was the line range at
proposal-drafting time; verified line range at hand-off is 685–722
via `rg -n 'Synthetic recovery: NEVER start IF2' CLAUDE.md`.

## Verbatim current text (verified 2026-05-26 against camdl-book HEAD)

```markdown
## Synthetic recovery: NEVER start IF2 (or any optimizer) from truth

Synthetic-recovery experiments exist to answer "can inference recover
the truth parameters we put in, *without being told what they are*?"
The moment any step in the pipeline — scout, refine, profile
likelihood — is initialized at (or warm-started from) the truth
parameters, you have **data leakage**. You are not measuring what
inference can do; you are measuring whether a short perturbation
from truth stays near truth. The results are optimistic at best and
meaningless at worst.

Concrete rules:

- `camdl profile --params X` uses X as the IF2 starting point at each
  grid point. **X must not be the truth params file in a synthetic
  recovery setting.** Use the scout's MLE (`fit_synthetic-*/real/fit_42/scout/mle_params.toml`)
  or a mid-range-priors param file — whatever you'd have access to in
  a real analysis where truth is unknown.
- `camdl fit run` with `start = ...` per-parameter entries should
  reflect domain-reasonable guesses, not truth values. The scout's
  auto-dispersed random starts around these declared `start`s are
  what actually reach the basin.
- `camdl pfilter --params X` *evaluates* the likelihood at parameter
  vector X (no optimization). This is fine to use with truth X — it's
  a slice-likelihood visualization, not an inference step. Just be
  clear in captions that "slice through truth" is a descriptive
  reference, not a recovered estimate.
- When writing notes or captions, treat any compute that starts from
  truth as having a loud "leaked truth" asterisk. Re-running with
  scout-MLE or prior-draw starts is always the cleanest fix.

**Incident of record**: `vignettes/he2010-synthetic.qmd` early drafts
computed every `camdl profile` run with `--params params/he2010_london.toml`
(truth). Three contaminated TSVs (`profile_s0_true.tsv`,
`profile_s0_true_ext.tsv`, `profile_r0_gamma.tsv`) had to be regenerated
with scout-MLE starts before the chapter could be trusted. The specific
"2D profile peak at γ = 0.047" finding reversed direction once the
re-run used honest starts.
```

## Replacement text

```markdown
## Synthetic recovery: NEVER start IF2 (or any optimizer) from truth

Synthetic-recovery experiments exist to answer "can inference recover
the truth parameters we put in, *without being told what they are*?"
The moment any step in the pipeline — scout, refine, profile
likelihood — is initialized at (or warm-started from) the truth
parameters, you have **data leakage**. You are not measuring what
inference can do; you are measuring whether a short perturbation
from truth stays near truth. The results are optimistic at best and
meaningless at worst.

Two operational flag patterns matter here, both new under the
2026-05-25 CLI UX revision (which removed `--params` from inference
subcommands):

- **Warm-starting an optimizer** is expressed by
  `--init from_params --params <toml>` (hand-written or
  exported-from-elsewhere flat params file) or by
  `--init from_mle --mle <fit-dir>` (a prior camdl fit's MLE
  output). The `--params` form is now strictly a *companion of
  `--init from_params`*; bare `--params` on inference subcommands
  is rejected with an actionable error.
- **Evaluating the likelihood at a specific point** is expressed by
  `--fixed-file <toml>` on `camdl pfilter` (or `--fixed NAME=VALUE`
  for one-off pins). Pfilter is non-inference — no optimization —
  so pinning every parameter is exactly "score the likelihood at
  this point."

Concrete rules:

- `camdl profile --init from_params --params X` uses X as the
  per-cell IF2 starting point. **X must not be the truth params
  file in a synthetic recovery setting.** Use the scout's MLE
  (`camdl profile --init from_mle --mle
  fit_synthetic-*/real/fit_42/scout/`) or a mid-range-priors
  params file — whatever you'd have access to in a real analysis
  where truth is unknown.
- `camdl fit run` with `start = ...` per-parameter entries in
  `[estimate]` should reflect domain-reasonable guesses, not truth
  values. The scout's LHS-stratified random starts around these
  declared `start`s are what actually reach the basin. (Stage-level
  `init = "lhs"` is the default in fit.toml.)
- `camdl pfilter --fixed-file X.toml` *evaluates* the likelihood at
  parameter vector X (no optimization). This is fine to use with
  truth X — it's a slice-likelihood visualization, not an inference
  step. Just be clear in captions that "slice through truth" is a
  descriptive reference, not a recovered estimate. (`camdl pfilter`
  is non-inference and still accepts `--params X.toml` as a synonym
  for `--fixed-file X.toml`; the `--fixed-file` form is preferred
  in new writing for cross-subcommand consistency.)
- When writing notes or captions, treat any compute that starts
  from truth as having a loud "leaked truth" asterisk. Re-running
  with `--init from_mle --mle <scout-dir>` or `--init from_prior`
  starts is always the cleanest fix.

**Incident of record**: `vignettes/he2010-synthetic.qmd` early drafts
computed every `camdl profile` run with `--params params/he2010_london.toml`
(truth) — the pre-2026-05-25 spelling of what is now
`--init from_params --params params/he2010_london.toml`. The
underlying mistake (truth-init in a synthetic-recovery setting)
is exactly the same; only the spelling of the flag changed.
Three contaminated TSVs (`profile_s0_true.tsv`,
`profile_s0_true_ext.tsv`, `profile_r0_gamma.tsv`) had to be
regenerated with scout-MLE starts before the chapter could be
trusted. The specific "2D profile peak at γ = 0.047" finding
reversed direction once the re-run used honest starts.
```

## Diff-summary view (for the maintainer)

The teaching point is unchanged. The mechanical changes:

| Pattern in old text                            | Replacement                                                                                                              |
| ---------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------ |
| `camdl profile --params X`                     | `camdl profile --init from_params --params X` (or `--init from_mle --mle <fit-dir>` for warm-start from a prior camdl fit) |
| `camdl pfilter --params X`                     | `camdl pfilter --fixed-file X.toml` (preferred new spelling; bare `--params` still works on pfilter as a non-inference subcommand)  |
| `--params params/he2010_london.toml` (incident) | (kept verbatim in the incident text, but flagged inline as the pre-2026-05-25 spelling — preserves historical fidelity)   |

Two structural additions versus the original:

1. A two-paragraph "operational flag patterns" block at the top
   that names the new `--init from_params` / `--fixed-file`
   verbs and points at the proposal for the rationale. The
   teaching is in *the same place* as before; the new block just
   localises the new vocabulary so the rules below it use the
   right spellings.
2. An explicit "fit.toml init = 'lhs' is the default" parenthetical
   in the `camdl fit run` rule, so a reader doesn't think LHS is
   something they have to opt into.

## Apply

`cd /Users/vsb/projects/work/camdl-book` and paste the
"Replacement text" block above over the existing
`## Synthetic recovery: NEVER start IF2 (or any optimizer) from truth`
section (verified at lines 685–722 of HEAD on 2026-05-26). No other
changes are required in `camdl-book/CLAUDE.md` for the CLI UX
revision; the file's many other rules (no-freezing, plain
progress, etc.) are not affected.

Commit message suggestion:

```
docs(CLAUDE): synthetic-recovery rule — update flags for CLI UX rev 2

camdl removed `--params` from inference subcommands on 2026-05-25;
warm-starting is now `--init from_params --params <toml>` (or
`--init from_mle --mle <fit-dir>`), and likelihood evaluation
under pfilter is preferentially `--fixed-file <toml>`. The
synthetic-recovery teaching point (truth-init is data leakage) is
unchanged; only the flag spellings updated.

Proposal: camdl/docs/dev/proposals/2026-05-25-cli-init-and-params-ux.md
Handoff:  camdl/docs/dev/notes/2026-05-26-camdl-book-claudemd-patch.md
```
