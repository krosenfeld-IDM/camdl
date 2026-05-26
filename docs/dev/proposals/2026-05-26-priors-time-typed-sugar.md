# Priors on time-typed parameters: unit-aware sugar

Date: 2026-05-26
Author: vsb
Status: draft for review
Follow-up to: [`2026-05-22-typed-time-and-dsl-ergonomics.md`](2026-05-22-typed-time-and-dsl-ergonomics.md)
Related: gh#103 (warn-on-`instant`-with-no-origin), gh#107 (typed
         `ParamKind` enum)

## Class

**code-vs-code**: the dim-checker already validates unit literals
(`5 'days`, `date(...)`) in parameter bounds, transition rates,
forcing periods, and `simulate.from/to/dt`. It does **not** reach
into the arg positions of `~ <distribution>(...)` prior declarations.
The result: priors on time-typed parameters silently accept bare
floats whose units are invisible from the prior site. The typed-time
proposal (`2026-05-22-typed-time-and-dsl-ergonomics.md`) closed the
calendar-drift bug class for bounds and arithmetic; this proposal
closes the analogous gap for priors.

## TL;DR

Extend the dim-checker's reach to prior argument positions. Three
deliverables:

1. **Unit literals and `date(...)` accepted in prior arg positions**,
   per the parameter's kind. `tau : instant` ⇒ `~ uniform(lower =
   date("2019-12-01"), upper = date("2020-02-24"))` works; same
   E3xx hard-error machinery as the typed-time proposal fires on
   kind mismatch.
2. **`median = X` sugar for log_normal** alongside the existing
   `mu = ...` form. Exactly one of `mu` / `median` must be specified;
   compiler hard-errors if both are present. Allows users to write
   `log_normal(median = 5 'days, sigma = 0.4)` directly.
3. **Bare-number fallback inherits the model's `time_unit`**, with
   an INFO diagnostic at compile time naming each fallback site so
   the user can audit which prior args were unit-imputed.

Plus a follow-on consequence of Q1 (see "Iteration history"
below): `~ uniform()` with no args defaults to the parameter's
declared `in [...]` bounds, removing the redundancy + off-by-one
trap on `tau : instant in [...] ~ uniform(−86, 0)` patterns.

## The gap, by example

From `camdl-book/guide/fitting/seed-timing/models/seir_synth.camdl`,
verified at HEAD on 2026-05-26 (lines 14, 18, 19):

```camdl
tau   : instant  in [date("2019-12-01"), date("2020-02-24")]
                 ~ uniform(lower = -86.0, upper = 0.0)
t_rep : instant  in [date("2020-02-09"), date("2020-03-25")]
                 ~ normal(mu = 4.0, sigma = 5.0)
w_rep : duration in [2.5, 30.0]
                 ~ log_normal(mu = 1.609, sigma = 0.4)
```

Three concrete trip points a reviewer should not have to manually
compute around:

- **`tau`** bounds span `[2019-12-01, 2020-02-24]` = 85 days inclusive;
  uniform spans 86 days. The off-by-one is invisible at the prior
  site without manual calendar arithmetic against `origin`. Either
  a typo or a deliberate one-day buffer — the language can't tell.
- **`t_rep`** prior parameter `sigma = 5.0`: units are implicitly the
  model's `time_unit`. A reader has to know the model's time_unit
  to interpret. The bounds spelling `date(...)` makes the units
  explicit; the prior spelling silently doesn't.
- **`w_rep`** prior `mu = 1.609`: not just unit-implicit but
  scale-implicit — `mu` for log_normal is on the log scale, so the
  natural-scale median is `exp(1.609) ≈ 5.0 'days`. A reader has to
  know both "log_normal mu is logged" and "the log is taken in
  time_unit space." Doubly invisible.

The bounds already use date/duration literals. There's no principled
reason the prior args can't.

## Iteration history (so reviewers can see the design space)

This proposal converged through one round of iteration with the
maintainer; recording the rejected alternatives here keeps the
design provenance auditable:

- **Q1 — uniform-redundancy with bounds**: rejected "strict-redundancy
  with hard-error consistency check" in favour of "**`~ uniform()`
  with no args defaults to bounds**." Rationale: a uniform prior
  with explicit narrower bounds (`~ uniform(lower = a, upper = b)`)
  is a legitimate user move; the all-or-nothing strict-redundancy
  rule would conflate "user wants bounds-uniform" with "user wants
  narrower-uniform." The default-to-bounds sugar gives both surfaces
  cleanly: omit args for bounds-uniform, supply args for a narrower
  prior. Redundant-and-consistent declarations are silently allowed
  (no point hard-erroring on a benign restatement); explicit
  declarations whose bounds disagree with the parameter's
  `in [...]` clause fire E3xx (the prior spans values the parameter
  literally cannot take).
- **Q2 — log-scale parameterisation for log_normal**: rejected
  "median-only" and "mu-only" in favour of **Q2-C: both `mu` and
  `median` accepted; exactly one required**. Rationale: `mu` is
  what statistically literate users think in (it's the underlying
  Normal's mean); `median` is what application-domain users think
  in (it's the most-likely value on the natural scale). Forcing one
  parameterisation loses one audience. The "exactly one of" rule
  prevents the ambiguity of both being specified.
- **Q3 — type-mismatch behaviour**: hard error, per the typed-time
  proposal's §6 "warnings get skimmed" argument.
- **Q4 — relationship to typed-time proposal**: pure follow-up.
  The typed-time proposal is self-contained and reviewable as a
  unit; bolting priors onto it would make the merge harder.

## Proposal

### Surface: unit literals in prior arg positions

For natural-scale prior parameters whose argument is a value on the
parameter's own scale (uniform's `lower`/`upper`, normal's `mu`/`sigma`,
gamma's `rate`, exponential's `rate`, etc.), accept the same dim-typed
literals the bounds already accept:

```camdl
tau   : instant  in [date("2019-12-01"), date("2020-02-24")]
                 ~ uniform()                         # defaults to bounds

t_rep : instant  in [date("2020-02-09"), date("2020-03-25")]
                 ~ normal(mu = date("2020-02-28"),
                          sigma = 5 'days)

w_rep : duration in [2.5 'days, 30 'days]
                 ~ log_normal(median = 5 'days, sigma = 0.4)
                 # OR equivalently: log_normal(mu = log(5 'days), sigma = 0.4)

beta  : rate     in [0.01 'per_day, 5.0 'per_day]
                 ~ normal(mu = 0.5 'per_day, sigma = 0.1 'per_day)
```

The dim-checker validates each prior arg against the parameter's kind:

| Parameter kind  | Prior arg position kind                          |
|-----------------|--------------------------------------------------|
| `instant`       | `Instant` (e.g. `date(...)`, or Duration-shifted from origin) for location; `Duration` for `sigma` |
| `duration`      | `Duration` for location *and* scale              |
| `rate`          | `Rate` (`[T⁻¹]`) for location and scale          |
| `probability`   | unitless `[0,1]` for location and scale          |
| `count`, `positive`, `real` | unitless                             |

Mismatches fire E3xx with a fix-hint, exactly like the typed-time
proposal handles `date + N 'months`. Example diagnostic for
`sigma = 5 'count` on `tau : instant`:

```
E3xx [seir_synth.camdl:18]: prior arg `sigma` expects a Duration
  for an instant-typed parameter (`t_rep`); got `'count`. Use a
  time unit (`5 'days`, `0.5 'weeks`), a bare number (interpreted
  as the model's time_unit), or remove the unit annotation if it
  was unintended.
```

### Bare-number fallback + INFO diagnostic

Bare numbers in prior arg positions inherit the model's `time_unit`
when the parameter's kind is time-typed (`instant`, `duration`) or
rate-typed (`rate`). This is the same convention as
`simulate.from/to/dt` and parameter bounds.

The compiler emits an INFO diagnostic per bare-number fallback site,
naming the parameter, the prior arg, the inferred unit, and the
explicit form for migration:

```
I3xx [seir_synth.camdl:14]: prior `tau ~ uniform(lower = -86.0, ...)`:
  bare number -86.0 interpreted as `-86.0 'days` (model time_unit).
  Make this explicit with `lower = -86.0 'days` or `lower =
  date("2019-11-30")`.
```

INFO, not WARNING, so it doesn't pollute the warning channel that
agents typically grep, but agents *can* opt-in to see it via
`--info` or by parsing `run.json`'s `diagnostics.info` array (a new
slot mirroring the existing `diagnostics.warnings`). User-facing
default is "show INFO at compile time, not at run time."

This makes the bare-number form a soft, audited fallback: it works,
but every compile produces a log line per usage so the user can
audit.

### Log-scale parameterisation for log_normal: `mu` or `median`

The log-normal's natural-scale **median** equals `exp(mu)` where `mu`
is the underlying Normal's location parameter. The bijection is
trivial:

```
log_normal(mu = m,        sigma = s)  ≡  log_normal(median = exp(m), sigma = s)
log_normal(median = med,  sigma = s)  ≡  log_normal(mu = log(med),  sigma = s)
```

Both forms accepted. **Exactly one of `mu` / `median` must be
specified**; the parser hard-errors if both are present:

```
E3xx [model.camdl:19]: prior `~ log_normal(...)` specifies both
  `mu` and `median`; exactly one must be set. They are alternative
  parameterisations of the same distribution
  (median = exp(mu)); pick the one you think in.
```

When the parameter is time-typed, `median` accepts a duration literal
(`median = 5 'days`); the compiler emits the underlying
`mu = log(median_in_time_unit)`. The `sigma` of log_normal stays
unitless — it's a *log-ratio*, the geometric standard deviation of
the multiplicative spread (`median × exp(±sigma)` is the geometric
1-σ range). Users who think in geometric-SD terms can read `sigma`
directly; users who think in arithmetic terms should use `normal`,
not `log_normal`, on the natural scale.

### `~ uniform()` with no args defaults to bounds

When the parameter has an `in [lo, hi]` clause AND the prior is
`~ uniform()` with no explicit `lower=`/`upper=`, the compiler
synthesises `uniform(lower = lo, upper = hi)`. This is the common
case (the seed-timing chapter's `tau` is exactly this shape).
Narrower priors stay explicit:

```camdl
# Common case (bounds-uniform): no redundancy, no off-by-one risk.
tau : instant in [date("2019-12-01"), date("2020-02-24")]
              ~ uniform()

# Narrower-than-bounds: explicit, intentional.
tau : instant in [date("2019-12-01"), date("2020-02-24")]
              ~ uniform(lower = date("2020-01-01"),
                        upper = date("2020-02-01"))
```

`~ uniform()` on a parameter without `in [...]` is an E3xx hard
error — there's nothing to default to.

`~ uniform(lower = a, upper = b)` with `[a, b]` strictly wider than
the parameter's `in [...]` clause is also E3xx — the prior would
place density on values the parameter cannot legally take. The
prior's support must be ⊆ the bounds (the bounds are the hard
support; the prior is the soft preference within).

## Where this interacts with the typed-time proposal

The typed-time proposal (2026-05-22) provides the dim-checker
infrastructure this proposal extends. Specifically:

- The Exact / Calendar duration refinement and the LUB propagation
  already handle `5 'days + 2 'weeks` etc. inside prior args once
  the dim-checker reaches them. No new propagation rules required.
- The E3xx hard-error machinery is reused for kind mismatches in
  prior args; we add new code-table rows but no new diagnostic
  framework.
- `date(...)` literals work in prior args under exactly the same
  anchored-mode rules as bounds. An `instant`-kind parameter in an
  unanchored model with a `date(...)` prior arg fires the same
  E220 as today (`date(...) requires origin = date(...)`).

The typed-time proposal's §5 catalog gains four new rows when this
proposal ships (one per new diagnostic this proposal introduces):

| Code   | Trigger                                                    | Hint shape |
|--------|------------------------------------------------------------|------------|
| E3xx   | prior arg kind mismatch (e.g. `'count` for an `instant` param) | "prior arg `<n>` expects `<expected-kind>`; got `<actual-kind>`; use `<example-fix>`" |
| E3xx   | both `mu` and `median` on `log_normal`                     | "alternative parameterisations of the same distribution; pick one" |
| E3xx   | prior support strictly wider than declared bounds          | "prior places density on values outside the `in [...]` clause; bound the prior or widen the parameter" |
| I3xx   | bare number in prior arg position falls back to time_unit  | "interpreted as `<value> '<time_unit>`; make explicit with `<value> '<time_unit>` or a date literal" |

## Worked migration of the seed-timing model

Pre:

```camdl
tau     : instant     in [date("2019-12-01"), date("2020-02-24")]
                      ~ uniform(lower = -86.0, upper = 0.0)
n_seed  : count       in [1, 1000]
                      ~ log_normal(mu = 1.609, sigma = 1.0)
rho_max : probability in [0.0, 1.0]
                      ~ beta(alpha = 2.0, beta = 8.0)
t_rep   : instant     in [date("2020-02-09"), date("2020-03-25")]
                      ~ normal(mu = 4.0, sigma = 5.0)
w_rep   : duration    in [2.5, 30.0]
                      ~ log_normal(mu = 1.609, sigma = 0.4)
k       : positive    in [0.1, 1000.0]
                      ~ log_normal(mu = 3.0, sigma = 1.0)
```

Post:

```camdl
tau     : instant     in [date("2019-12-01"), date("2020-02-24")]
                      ~ uniform()                             # defaults to bounds

n_seed  : count       in [1, 1000]
                      ~ log_normal(median = 5.0, sigma = 1.0)
                      # count is unitless; median is a count

rho_max : probability in [0.0, 1.0]
                      ~ beta(alpha = 2.0, beta = 8.0)         # unchanged; unitless

t_rep   : instant     in [date("2020-02-09"), date("2020-03-25")]
                      ~ normal(mu = date("2020-02-28"),
                               sigma = 5 'days)

w_rep   : duration    in [2.5 'days, 30 'days]
                      ~ log_normal(median = 5 'days, sigma = 0.4)

k       : positive    in [0.1, 1000.0]
                      ~ log_normal(median = 20.0, sigma = 1.0)
                      # positive is unitless; median is unitless
```

Six lines, each carrying its full kind/unit story at the prior site.
The off-by-one on `tau` becomes literally invisible-by-construction:
the bounds are the only place dates live.

## Implementation phasing

1. **Dim-checker extension** (~50 LOC + tests): allow `Expr`-typed
   args in prior arg positions in `ocaml/lib/compiler/parser.mly` and
   `dimcheck.ml`. Cover all six built-in distributions (uniform,
   normal, log_normal, gamma, beta, exponential).
2. **`median = X` sugar for log_normal** (~30 LOC): parser accepts
   the new keyword, expander converts to `mu = log(median)` after
   dim-conversion to model's time_unit. Hard-error on
   both-mu-and-median (~10 LOC + 1 test).
3. **`~ uniform()` default-to-bounds** (~20 LOC): the parser-level
   "no args" form is a small pattern match; the synthesis happens at
   expander time once bounds are resolved.
4. **Bounds-vs-prior-support check** (~30 LOC + 2-3 tests): per-prior
   support derivation (uniform: explicit; normal: ∞; log_normal:
   positive; beta: [0,1]; gamma: positive; exponential: positive)
   compared against the parameter's `in [...]` clause.
5. **INFO diagnostic for bare-number fallback** (~40 LOC): emit per
   site, dedupe so the same expression doesn't spam, route through
   the existing diagnostic surface.

Each step independently testable. Total: ~170 LOC + ~150 LOC of
tests (positive cases + each E3xx variant). Could ship as one
PR or per-step; per-step is cleaner for review.

**Sequencing**: lands AFTER the typed-time proposal ships
(`2026-05-22-typed-time-and-dsl-ergonomics.md`), since this proposal
reuses its dim-checker infrastructure and E3xx machinery.

## Tests (TDD)

The headline regression test mirrors the `seir_synth.camdl` worked
migration above:

- **Bounds-uniform sugar**: parse `~ uniform()` on a parameter with
  `in [a, b]`; assert expansion equals `~ uniform(lower=a, upper=b)`.
- **`~ uniform()` without bounds is E3xx**: parse a parameter with no
  `in [...]` clause and `~ uniform()`; expect compile error with
  the right hint.
- **`uniform(lower=a, upper=b)` wider than bounds is E3xx**: parse
  `in [0.1, 1.0] ~ uniform(lower = 0.0, upper = 2.0)`; expect E3xx.
- **`median` sugar for log_normal**: parse `~ log_normal(median = 5
  'days, sigma = 0.4)` on a duration param; assert expansion
  produces `mu = log(5)` (under `time_unit = 'days`).
- **Both `mu` and `median` is E3xx**: parse `~ log_normal(mu = 0.0,
  median = 5 'days, ...)`; expect E3xx.
- **Date in normal mu**: parse `~ normal(mu = date("2020-02-28"),
  sigma = 5 'days)` on instant param with anchored mode; assert
  expansion produces the date-as-Duration form internally.
- **Kind mismatch on prior arg**: parse `sigma = 5 'count` on a
  duration param; expect E3xx.
- **Bare number → INFO**: parse `~ normal(mu = 4.0, sigma = 5.0)` on
  an instant param; assert compile produces two INFO diagnostics
  (one per bare-number arg) with the migration text.
- **Calendar duration in instant-typed prior arg**: parse
  `sigma = 6 'months` on an instant param's normal; expect E3xx
  (reuses the typed-time Exact/Calendar refinement; calendar
  durations can't translate an instant).

Plus a regression test for the `seir_synth.camdl` exact text: parse
the pre-migration model, assert all expected diagnostics fire; parse
the post-migration model, assert clean compile.

## Out of scope

- Multivariate priors (e.g. correlated Normal on (β, γ)). Not a
  current need; deferred indefinitely.
- Hierarchical / partially-pooled priors with units. Hierarchical
  priors already have their own DSL surface
  (`docs/dev/proposals/2026-04-16-prior-syntax.md`); extending units
  through that surface is a follow-up.
- Truncation syntax (e.g. `truncated_normal(mu, sigma, lo, hi)`).
  The `in [...]` bounds clause already truncates the support
  globally; explicit truncation is redundant and would need its own
  RFC.
- The `--info` flag and `diagnostics.info` JSON slot referenced in
  §"Bare-number fallback". These are minor additions that ship
  separately if the user-facing default ("show INFO at compile,
  don't pollute run.json's warnings") proves insufficient.

## Acceptance criteria

- The dim-checker extends to prior arg positions with the type
  rules in §"Surface" above.
- The `median = X` sugar for log_normal works under `time_unit`-aware
  conversion; the hard-error on both-`mu`-and-`median` fires.
- `~ uniform()` with no args defaults to bounds when bounds exist;
  E3xx when they don't.
- Bare numbers in time-typed prior args inherit `time_unit` and emit
  an INFO diagnostic per fallback site.
- The seed-timing chapter's `seir_synth.camdl` migrates to the new
  surface cleanly with no semantic change (golden trajectories
  byte-identical under the same seed; verified via `make
  update-expected` diff).
- Cross-checks (every parameter kind × every distribution) pass the
  dim-checker on positive cases and fire E3xx on negative cases.

## References

- `docs/dev/proposals/2026-05-22-typed-time-and-dsl-ergonomics.md`
  — the typed-time / Exact-Calendar refinement that this proposal
  extends. Reuses its dim-checker, its E3xx machinery, and its
  anchored/unanchored mode distinction.
- `docs/dev/proposals/2026-05-22-calendar-time.md` — the
  dated-I/O-boundary proposal underlying the typed-time work.
- `docs/dates.md` — calendar policy.
- `docs/camdl-language-spec.md` §2 (unit literals), §4.1 (parameter
  kinds).
- gh#103 — *"Profile diagnostics: split IF2/PMMH R̂ columns; surface
  resolved priors in fit summary/tree/mle.toml; warn on
  instant-with-no-origin"*. The last clause is partially closed by
  the typed-time proposal's TH5 amendment (info-on-mode-switch);
  this proposal references it for cross-context.
- gh#107 — *"IR typing: replace bool always_active with Kind enum;
  typed ParamKind enum"*. This proposal's dim-checker rules key off
  ParamKind; the implementation should target the typed enum gh#107
  is moving toward, not the legacy `Option<String>` `param_kind`.
