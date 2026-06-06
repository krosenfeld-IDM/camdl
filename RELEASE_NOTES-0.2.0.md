<!--
STATUS: DRAFT / preview — a worked example of the /release-notes pipeline.
SCOPE:  this covers only the documentation-hardening branch (15 commits). The
        actual 0.2.0 is cut from main over v0.1.0-alpha..main and folds in ~423
        more unreleased commits — regenerate over the full range at release time
        (make changelog → /release-notes). The content here is a slice, not the
        whole release.
VERSION: the only existing tag (v0.1.0-alpha, 2026-05-15) is a PRERELEASE, so
        `git-cliff --bumped-version` continues it to v0.1.0-alpha.1. To open a
        clean 0.2.0 line, finalize first — tag a stable v0.1.0 (drop -alpha) or
        manually tag 0.2.0. "0.2.0" below is the intended first real minor; the
        tooling won't pick it automatically from a prerelease tag.
-->

# camdl 0.2.0 — 2026-06-05 (draft / doc-hardening slice)

Hardening release: the documentation is now compiler-verified, and a dimensional
false-positive that blocked a class of observation models is fixed.

## Highlights
- **Documentation is now checked against the real compiler.** Every gated `camdl`
  example must compile and every documented `camdl …` command must reference a
  real flag — enforced in CI, so the docs can't silently rot.
- **Fixed: prevalence-as-proportion observation models now type-check.** A
  likelihood like `binomial(p = projected)` with `projected = I / N` was wrongly
  rejected; it now works, and the missing-`/N` safety check is preserved.
- **New CLI drift gate.** A parse-only `camdl __check-args` mode catches
  documented commands/flags that don't exist.

## Breaking changes
None. The dimcheck fix only accepts previously-rejected valid models; the
corrected CLI examples referenced flags that never existed.

## Language (DSL)
- **Fixed (E304):** the `projected` keyword carried a hard-coded population
  dimension, so a proportion projection used as a probability false-fired a
  dimension error. It now takes the projection expression's actual dimension.
  Count projections (`prevalence`/`incidence`) used as a probability still error,
  as intended.
- **Docs:** 26 more language-spec examples (observations, interventions, events,
  balance, stratification, forcing, scenarios, ODE, init, tables,
  multi-source/branching/Erlang transitions) are now compiler-verified. Two
  long-broken examples were fixed: the real-compartment declaration (a missing
  comma) and the onboarding age-SIR (a missing `parameters{}` block).

## CLI
- **New:** `camdl __check-args` — a hidden parse-only mode (exits 2 on an unknown
  subcommand/flag) backing `make test-cli-docs`, which gates the `camdl …`
  commands in the docs (37 commands today, 0 drift).
- **Docs corrected** (these never worked as written): `profile --focal R0 --grid …`
  → `--sweep "R0=lin(10,80,8)"`; dropped the non-existent `pfilter
  --obs-model`/`--tol`; `simulate --trace` → `-o traj.tsv`.

## Formats & compatibility
- No IR schema change (`ir/VERSION` unchanged); saved `.ir.json` files are
  unaffected.

## Internal / docs / CI
- New doctest + CLI gates wired into a doc-triggered CI workflow; engineering and
  findings notes under `docs/dev/notes/`; versioning policy (`VERSIONING.md`) and
  the `/release-notes` skill.

*Full changelog: `v0.1.0-alpha..HEAD`. Recommended bump: MINOR → `0.2.0`.*
