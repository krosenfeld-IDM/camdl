# CAMDL language changes

Breaking and notable changes to the **CAMDL language** — grammar, dimensions,
and semantics — newest first. This is the history an agent needs when a model
that "should" compile is rejected: find the change, apply the migration.

Scope: the *language surface* (what you write in a `.camdl` file). CLI and
`fit.toml` changes live in the full changelog (`camdl docs changelog`). For the
*current* syntax, see `camdl docs language` (the spec).

How to read an entry: **what changed**, the **migration** (old → new), and the
**diagnostic** you'll hit if you use the old form.

---

## 2026-06-04 — phantom `output {}` sub-blocks removed

**What.** The `summary {}`, `flows {}`, `synthetic {}`, and experiment/compare
sub-blocks inside `output {}` never did anything and were removed; using them is
now an error.

**Migration.** Delete them. Trajectory cadence and format are configured on
`output {}` directly (see `camdl docs language`); there is no per-quantity
sub-block surface.

**Diagnostic.** `error[E106]` on the removed sub-block.

## 2026-05-26 — strict dimensions on likelihood arguments (gh#116)

**What.** Observation-likelihood arguments with a fixed dimensional contract —
`Binomial.p`, `Bernoulli.p`, `BetaBinomial.alpha`/`beta`, `NegBinomial.dispersion`
— are now strictly checked. A *count* where a probability/dimensionless value is
required (the textbook missing-`/N` bug) is rejected instead of silently
accepted.

**Migration.** Make the argument dimensionless: `binomial(n = N, p = projected)`
where `projected` is a *count* → `p = projected / N` (a proportion). A projection
that is already a proportion (`projected = I / N`) is fine.

**Diagnostic.** `error[E304]` "must be dimensionless (probability); a count here
is almost certainly a missing `/N`."

## 2026-04-22 — every forcing requires a unit-kind tag (GH #8)

**What.** A forcing declaration must carry a unit-kind literal after its type, so
the compiler knows whether the forcing is a count, a rate, a ratio, etc. The
un-annotated form no longer parses.

**Migration.**
```
forcing {
  pop    : interpolated { ... }      →   pop    : interpolated 'count { ... }
  birthrate : interpolated { ... }   →   birthrate : interpolated 'per_year { ... }
  school : periodic { ... }          →   school : periodic 'ratio { ... }
}
```
Same for `sinusoidal`/`piecewise`. Pick the kind from what the forcing *is*
(a population is `'count`, a multiplier is `'ratio`); see the forcing-kinds
taxonomy in `camdl docs language`.

**Diagnostic.** `error[E001]: syntax error` at the forcing type (no migration
hint yet — see the policy in CLAUDE.md; this log is the bridge until the
diagnostic points here directly).

## 2026-03-28 — `functions {}` renamed to `forcing {}`

**What.** The block declaring time-varying covariates (population, birth rate,
seasonal terms) was renamed from `functions {}` to `forcing {}`.

**Migration.** Rename the block keyword: `functions {` → `forcing {`. The
contents are unchanged (modulo the unit-kind tag added 2026-04-22, above).

**Diagnostic.** `error[E001]: syntax error` on the `functions` keyword.

---

*This log is seeded with the breaking changes surfaced so far; older or smaller
changes may not yet be backfilled. Add an entry (on top) whenever a breaking
language change lands — see CLAUDE.md, "Breaking language changes must signpost
the migration."*
