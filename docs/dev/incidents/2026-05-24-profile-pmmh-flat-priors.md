# Profile-PMMH silently targeted likelihood-with-flat-priors before 2026-05-24

Date: 2026-05-24 (fix landed in `5f658a16`; incident retroactively
documented 2026-05-26)
Severity: critical (silent wrong inference target; user-reported
posterior is actually a scaled likelihood)
Subsystem: `camdl profile --algorithm pmmh`
Status: fixed in `5f658a16`; affected version range is
`profile-PMMH first ship` → `5f658a16^`

## What happened

`camdl profile --algorithm pmmh` hard-coded `Prior::Flat` for every
estimated parameter at `profile.rs:1013–1021` (pre-fix line range)
regardless of what the model IR declared via `~` syntax or what a
`--fit` toml supplied via `[estimate.<param>.prior]`. The MH
acceptance ratio therefore reduced to a pure likelihood ratio for
every step on every cell — i.e. PMMH targeted

```
π_implemented(θ) ∝ L(θ|y)
```

instead of

```
π_intended(θ) ∝ L(θ|y) · p(θ)
```

The commit message for `5f658a16` (gh#73 fix) acknowledged this
directly:

> Net effect: PMMH-via-profile was silently MLE-with-flat-priors
> regardless of what the model declared.

No warning, no diagnostic, no run.json field flagged that the
posterior semantics differed from what the user asked for.
`--algorithm pmmh` produced TSVs labelled "posterior" and `mle.toml`
fields named `final_log_posterior` — both of which were arithmetically
just the likelihood plus a constant 0 (the Flat prior's
`log_density`).

## How it was detected

By a real user running the camdl-book seed-timing chapter
(`docs/dev/proposals/2026-05-24-stuck-chain-diagnostics.md` documents
the reproduction). Symptoms were:

- `t_rep = −40` at the surveyed MAP, but the model declared
  `t_rep ~ Normal(4, 5)`; under the declared prior, t_rep at the
  mode is ≈4 with sd ≈5, so the MLE at -40 is ~5.8 prior-sd outside
  the mode — impossible under correct PMMH targeting of the posterior.
- `n_seed = 1000` pinned at its upper bound, but the model declared
  `n_seed ~ LogNormal(log 5, 1)`, which has 99% mass below ≈54.
- The "profile posterior" was therefore just the profile likelihood,
  and the user was reading off MLE-pinned-to-bounds values as if they
  were posterior modes.

## Why it matters

camdl informs public-health decisions. A user running an alpha-tagged
camdl profile before `5f658a16` and interpreting the output as a
Bayesian profile-posterior sweep would conclude:

1. The MAP estimate sits at a value the model's declared prior excludes.
2. Bound-pinned values are real posterior modes.
3. Compared-across-cells log-posterior contours describe the
   posterior surface — they actually describe the likelihood surface.

Any decision recommendation extracted from a profile-PMMH run from
this window is wrong in proportion to how informative the priors are.
For tight priors (e.g. structural epidemiological knowledge of R₀
band, generation interval, vaccine efficacy), the difference is
large.

## Affected version range

- **First shipped**: profile-PMMH first appeared in commit `26657cd3`
  (`feat(profile): add PMMH as a per-cell algorithm`) on 2026-05-23.
- **Fixed**: commit `5f658a16` (`gh#73: honor priors in profile
  --algorithm pmmh`) on 2026-05-24.
- **Window**: any profile-PMMH run produced between `26657cd3` and
  `5f658a16^` carries the wrong inference target. Window is short
  in days but the alpha announcement (`9481135b`, 2026-05-25) post-
  dates the fix, so externally distributed alpha builds are not
  affected by default unless a user built from the
  `26657cd3..5f658a16^` range explicitly.

To audit a saved profile run for affected-ness, check
`run.json[stage.algorithm]` against the binary's commit at run time.
Or inspect any saved `mle.toml`: if `final_log_posterior ≈
final_loglik` exactly (no prior contribution) and the model has any
non-flat declared prior, the run was affected.

## What was wrong, what is right

**Wrong (pre-`5f658a16`)**: `profile.rs:1013–1021` constructed a
fresh `Vec<Prior>` of length `per_start_specs.len()` populated with
`Prior::Flat` and passed it to `run_pmmh`. The model IR's
`parameters[i].prior` field was loaded but never consulted; the
`--fit` toml's `[estimate.<param>.prior]` block was parsed but
never threaded.

**Right (post-`5f658a16`)**: `profile.rs:1510-1515` now calls
`crate::fit::priors_precedence::resolve_priors_with_precedence`
against `(per_start_specs, fit_estimate, model)` and passes the
resolved priors to `run_pmmh`. The same resolver is reused by
`camdl fit run` (per gh#75), so profile and fit-run carry
byte-identical prior semantics for the same `(model, fit toml)`
inputs.

## What this changes

1. **The fix.** The buggy code is gone; the right code shipped in
   `5f658a16`. No further fix needed for the wrong-target bug
   itself.
2. **Related defect found while documenting this** (filed as
   gh#118; fixed in `ca893bf` on 2026-05-26): the fix above used
   the resolver against the *nuisance-only* set (focal params are
   pinned and excluded from PMMH's estimated set), so the emitted
   `log_posterior` was `loglik + Σ log_prior(nuisance only)` —
   correct PMMH targeting but a wrong `log_posterior` column.
   That column-correctness issue is closed by gh#118.
3. **Testing gap acknowledged.** No test caught either failure
   mode. The pre-`5f658a16` bug went undetected because no profile
   integration test asserted "MH acceptance must include prior
   ratio when priors are non-flat." The gh#118 bug went undetected
   because no test asserted "the emitted `log_posterior` column
   equals `loglik + Σ log_prior(all estimated params)`." A new
   cross-method invariant test suite is filed as part of Phase 1
   remediation; see `docs/dev/reviews/2026-05-26-week-audit-
   findings.md` C2 + the staged Phase-1 work.

## Reproduction (for forensic audit of saved runs)

If a saved profile run sits in `results/profiles/<name>-<hash>/`,
check:

```
$ jq '.stage' results/profiles/<name>-<hash>/run.json | grep -E 'algorithm|commit'
$ cat results/profiles/<name>-<hash>/replicates/seed_*/grid_*/start_*/mle.toml | grep final_log
```

If `final_log_posterior ≈ final_loglik` (difference < ~1 nat) on
a model with any non-flat declared prior, the run was affected.
Discard and re-run on a post-`5f658a16` binary.

## Why no incident report at fix time

The fix (`5f658a16`) was structured as a gh#73 feature-completion
commit ("honor priors in profile --algorithm pmmh") rather than as
an incident — the framing was "feature was incomplete" not "wrong
answers shipped." Per the 2026-05-26 week audit (C2), the right
framing is the latter because the user-facing surface (a column
named `final_log_posterior`, an algorithm advertised as Bayesian)
made the bug invisible to downstream consumers. Documenting
retroactively so external alpha users in the affected version
window have a checkable reproduction path.

## Lessons

1. **Silent reduction-to-degenerate-case is the worst bug class.**
   PMMH-with-flat-priors is mathematically identical to MLE up to
   a normalising constant; the output of a wrong-prior PMMH run
   looks superficially like a correct posterior sweep. Cross-
   method invariants (fit run vs profile must agree on
   log_posterior at the same θ) catch this whole class.
2. **Feature-completion commits that silently fixed wrong answers
   should be filed as incidents.** A user can't tell from a feature-
   completion commit whether their prior output was wrong; an
   incident report puts the affected version range and detection
   recipe in front of them.
3. **Tests must lock in the invariant, not the surface.** A test
   that asserts `mle.toml` contains a `final_log_posterior` field
   passes against the buggy code. A test that asserts the field's
   value equals `loglik + Σ log_prior(all estimated)` against a
   hand-computed reference catches it.

## References

- Fix commit: `5f658a16` (gh#73)
- Related fix: `ca893bf` (gh#118 — focal-prior offset)
- Week audit C2: `docs/dev/reviews/2026-05-26-week-audit-findings.md`
- Resolver shared with fit run: `gh#75` (`dd016f87`)
- Forensic detection recipe: see "Reproduction" above
