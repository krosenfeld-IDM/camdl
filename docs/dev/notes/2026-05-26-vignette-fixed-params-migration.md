# Maintainer-handoff: migrate `vignettes/he2010*/Makefile`

# `FIXED_PARAMS` to `--fixed-file`

Date: 2026-05-26 Project: camdl Tags: cli-ux, camdl-book, vignettes, handoff
Related: gh#83, gh#85; `docs/dev/proposals/2026-05-25-cli-init-and-params-ux.md`
§"Blast radius" item 4 (load-bearing prose rewrite #4) Author: cli-ux-rev2 step
11.4

## Why this is a handoff

The vignettes live in the **downstream** `camdl-book` repo
(`/Users/vsb/projects/work/camdl-book/`), not in the camdl main repo. The CLI
break that removed the name-only `--fixed "A,B,C"` form went out in camdl
`f8150c5` (step 7); the next render of these Makefiles against a current camdl
will fail with the actionable error spelled out in
`args/mod.rs::InferenceModelOverrides::check_removed_flags`.

This note is the maintainer's worked patch.

## Affected files

Verified by
`grep -ln "FIXED_PARAMS\|--fixed " /Users/vsb/projects/work/camdl-book/vignettes/*/Makefile`:

1. `vignettes/he2010/Makefile` —
   `FIXED_PARAMS = mu,iota,sigma_se,cohort,rho,psi,e0,i0,N0` at L68, used in 5
   `camdl profile` invocations (L79, L94, L110, L126, L142)
2. `vignettes/he2010-v0/Makefile` —
   `FIXED = N0,mu,iota,sigma_se,cohort,rho,psi,e0,i0` at L39, used in 2
   `camdl profile` invocations (L97, L108)

(`vignettes/he2010-bayesian/Makefile` does not currently use the name-only
`--fixed` form; verified by the same grep — its profile invocations are
PMMH-on-toml and don't carry this pattern.)

## What needs to change (per invocation)

Every affected `camdl profile` invocation currently has both:

- `--params $(PROFILE_PARAMS)` (a refine MLE for warm-start, NOT truth —
  anti-leakage per `camdl-book/CLAUDE.md` synthetic-recovery rule)
- `--fixed $(FIXED_PARAMS)` (a comma-separated _name list_, meaning "pin these
  at the model defaults")

Both forms are removed on inference subcommands under CLI UX rev 2. The
replacements are:

| Old                                        | New                                                                                                   |
| ------------------------------------------ | ----------------------------------------------------------------------------------------------------- |
| `--params $(PROFILE_PARAMS)`               | `--init from_params --params $(PROFILE_PARAMS)`                                                       |
| `--fixed $(FIXED_PARAMS)` (name-only list) | `--fixed-file $(FIXED_FILE)` where `FIXED_FILE` is a small flat TOML committed alongside the Makefile |

## The new `fixed.toml`

Verified values from
`/Users/vsb/projects/work/camdl-book/vignettes/he2010/params/he2010_london.toml`
(2026-05-26 HEAD; this is the truth file the vignette already checks in). Take
just the nine names that were in `FIXED_PARAMS` and copy them into a sibling
`fixed.toml`:

```toml
# vignettes/he2010/fixed.toml
# Parameters pinned during `camdl profile` cells (extracted from
# params/he2010_london.toml). The He, Ionides & King (2010)
# MLE values for London measles; these don't change across the
# profile likelihood sweeps over s0, R0, alpha.
mu = 0.0000548 # death rate (= 0.02/yr)
iota = 2.9 # importation floor (He et al.)
sigma_se = 2.816 # measurement σ_SE² × 365.25 (daily units)
cohort = 0.557 # fraction of births entering S at school start
rho = 0.488 # reporting probability
psi = 0.116 # measurement overdispersion
e0 = 0.0000517 # initial exposed fraction
i0 = 0.0000514 # initial infected fraction
N0 = 2462500 # pop at t=0 (London, 1944-01-07)
```

`he2010-v0/fixed.toml` is identical to the above minus `N0` (the v0 vignette
pins N0 separately, in the v0-only narrow-bounds context). The ordering doesn't
matter — `--fixed-file` reads the flat TOML and the precedence is "later file
overrides earlier."

## The new Makefile shape (he2010/Makefile)

Replace at the top of the file:

```make
PROFILE_PARAMS = /tmp/refine_mle_for_profile.toml
FIXED_PARAMS = mu,iota,sigma_se,cohort,rho,psi,e0,i0,N0
```

with:

```make
PROFILE_PARAMS = /tmp/refine_mle_for_profile.toml
FIXED_FILE     = fixed.toml
```

and replace every `camdl profile` invocation pair of lines like:

```make
--params $(PROFILE_PARAMS) \
--data $(SYNTH) \
--sweep '...' \
--fixed $(FIXED_PARAMS) \
```

with:

```make
--init from_params --params $(PROFILE_PARAMS) \
--data $(SYNTH) \
--sweep '...' \
--fixed-file $(FIXED_FILE) \
```

Apply the same shape to `he2010-v0/Makefile` (different variable name `FIXED` →
`FIXED_FILE`, different toml content as noted above).

## Verification after applying

```bash
cd /Users/vsb/projects/work/camdl-book/vignettes/he2010
make profile-s0 SEED=42       # one cell only; ~1.8h
```

Should run to completion with no `--params` / `--fixed` parse errors. The output
TSV (`validation/profile_s0.tsv`) should be **bit-identical** to a pre-rev-2 run
at the same seed — the precedence chain preserves the model_default →
fit_toml_fixed → fixed_file → scenario → fixed_cli order and the resolver's
output values are the same; only the CLI surface changed.

If the TSV is not bit-identical and the diff is in parameter values: re-derive
`fixed.toml` from `params/he2010_london.toml` and check that every name from the
old `FIXED_PARAMS` list got copied with its correct value.

## Why `--fixed-file` over `--fixed NAME=VALUE` ×9

Two reasons specific to vignettes:

1. The Makefile would grow nine extra lines per invocation (~45 lines added
   across the five profile cells in he2010/Makefile). `--fixed-file fixed.toml`
   is one line.
2. `fixed.toml` is grep-friendly — a reviewer asking "which parameters were held
   at their He et al. (2010) values?" gets a direct answer by `cat`ing the file.
   The names-list form required cross-referencing against
   `params/he2010_london.toml`.

This is the recommended pattern for vignettes that pin many parameters. The
proposal §"Blast radius" item 4 names this as the "canonical pattern for
many-fixed-param vignettes."

## Commit message suggestion

```
vignettes(he2010): migrate name-only --fixed to --fixed-file (CLI UX rev 2)

camdl removed `--fixed "a,b,c,..."` (name-only) and `--params` on
inference subcommands on 2026-05-25. Migrate the he2010 +
he2010-v0 Makefiles:

- Extract the nine pinned parameter values from
  params/he2010_london.toml into a sibling fixed.toml (one per
  vignette).
- Replace `--fixed $(FIXED_PARAMS)` with `--fixed-file fixed.toml`.
- Replace `--params $(PROFILE_PARAMS)` with
  `--init from_params --params $(PROFILE_PARAMS)`.

Numerics unchanged: the precedence chain preserves the
model_default → fixed_file order and the resolver's output values
are bit-identical to pre-rev-2 runs at the same seed.

Proposal: camdl/docs/dev/proposals/2026-05-25-cli-init-and-params-ux.md
Handoff:  camdl/docs/dev/notes/2026-05-26-vignette-fixed-params-migration.md
```
