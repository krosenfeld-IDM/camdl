# Proposal: Anchored vs unanchored models; exact vs calendar durations

**Status:** draft for review.
**Builds on:** [`docs/dev/proposals/2026-05-22-calendar-time.md`](2026-05-22-calendar-time.md)
(the dated-I/O boundary).
**Supersedes** an earlier draft of this proposal which (a) reinvented the
`days(N)` duration helper without realising the language already provides
`5 'days` and (b) deferred calendar arithmetic as "pending audit." Both
errors corrected here.

**One-line thesis.** Two minimal rules at compile time make the entire
calendar-drift bug class unrepresentable: (1) calendar durations
(`'months`, `'years`) cannot translate an `Instant`; (2) calendar-anchored
models require a constant-day axis. Two new primitive functions
(`add_calendar_months`, `add_calendar_years`) supply the only correct way
to step a date by calendar months/years. Everything else in the language
already exists, including the unit-literal surface (`5 'days`,
`0.087 'per_month`), the dimensional checker, and the `instant`/`duration`
parameter kinds.

---

## 1. Up-front definitions

camdl models come in two modes, distinguished by a single bit: whether
`origin = date(...)` is declared.

- **Anchored model** — declares `origin = date(...)`. The internal time
  axis is anchored to a real calendar date; `t = 0` is `origin`, and any
  `t > 0` corresponds to a real calendar date via the model's `time_unit`.
  Calendar dates flow at I/O (data columns, `--dates` output, `date()`
  literals in DSL constant positions). The seed-timing chapter's COVID-WA
  and Hagelloch fits are anchored.
- **Unanchored model** — no `origin`. The internal axis is abstract;
  `t = 0` has no calendar meaning. Time positions are bare numbers in
  the model's chosen `time_unit`. SBC, synthetic-indexed time, textbook
  SIR, and the dacca cholera SIRS models (where `t = month-number from
  1 Jan 1891` is documented as an *informal* anchor but not declared as
  `origin`) are unanchored.

The two modes share most of the language. Only the rules in §3 below
behave differently across them.

The proposal commits to introducing this vocabulary into
[`docs/dates.md`](../../dates.md) and §2 of
[`docs/camdl-language-spec.md`](../../camdl-language-spec.md) when it
ships; until then, "calendar-anchored" / "indexed-time" appear in some
older docs and should be read as synonyms for "anchored" / "unanchored."

### 1.1 Anchor-only primitives

The language has a small set of constructs that *only make sense* in
anchored mode. They are the **anchor-only primitives**:

- `date("YYYY-MM-DD")` — already E220 when `origin` is missing
  (pre-existing rule, spec §2.3 at
  `docs/camdl-language-spec.md` §2.3).
- `origin` — newly reserved in anchored mode as a referenceable
  read-only identifier of type `Instant`. The spec already reserves
  `t_start` and `t_end` (`docs/camdl-language-spec.md` §14
  "Timepoints and Reserved Identifiers" at line 2118 — verified by
  `grep -n "^## 14" docs/camdl-language-spec.md`), but not `origin`.
  That blocks the most natural calendar idioms like
  `add_calendar_months(origin, 6)` and `origin + 90 'days`. Adding
  `origin` to the reserved-identifier set, available in anchored
  mode only, unblocks them without any other surface change.

  **Referenceability scope (specified):** `origin` is usable wherever
  a DSL constant-position `Instant` is accepted — `origin + days(N)`,
  `add_calendar_months(origin, N)`, `simulate { from = origin }`,
  `at [origin, ...]` schedules, and `let landmark = origin + 90 'days`.
  Not usable inside rate expressions or any compartment-state context,
  since it's a compile-time constant, not a runtime value.
- `add_calendar_months(d, n)` — new in this proposal.
- `add_calendar_years(d, n)` — new in this proposal.

**On `instant`-kind parameters.** The kind itself is well-defined in
both modes (a `[T]`-dimensioned value, per the existing spec §4.1).
What differs across modes is what an `instant`-typed value *means*:

- In **anchored mode**, an `instant` value is a calendar point — it
  participates in the typed-time torsor (can be added to an
  `ExactDuration`, can be passed to `add_calendar_*`, renders as a
  date in `fit summary`).
- In **unanchored mode**, there's no calendar reference, so an
  `instant`-typed value is just a `[T]`-dimensioned scalar on the
  abstract axis — behaviourally equivalent to a `duration`-kind
  parameter for arithmetic purposes. The torsor's `Instant` /
  `Duration` distinction collapses because there's no anchor to
  reference an instant *to*; nothing in the language can ambiguously
  step or render it. Rule 1 is thereby vacuous in unanchored mode
  (see §3 for the formal statement).

The clean way to read this: `instant` kind is a *dimension claim*
(always `[T]`), and the torsor refinement on top of `[T]` activates
only when origin is declared.

**Distinct from anchor-only primitives: calendar-named affine
constructs.** These work in both modes because they're scalars in the
affine arithmetic of the model's `time_unit`:

- `5 'months`, `5 'years` — affine duration literals (≈ 152.18 days,
  ≈ 1826.21 days under the Gregorian constant in use; see §2 for the
  verification command). Legal in either mode wherever a duration is
  acceptable (table entries, parameter bounds, rate-expression
  arithmetic). Rule 1 forbids them *only* when added to an `Instant`
  in anchored mode.
- `0.087 'per_month`, `0.02 'per_year` — affine rate literals.
  Always legal; compiled to per-axis-unit values via `days_per_unit`.
- `time_unit = 'months` or `'years` — legal in unanchored mode (Rule
  2 forbids only the anchored case).

The distinction matters because the dacca SIRS configuration —
`time_unit = 'months`, `beta : rate 'per_month`, durations like
`6 'months` — is entirely composed of calendar-named *affine* constructs
and works fine in unanchored mode. No anchor-only primitive appears,
no calendar reference exists, no rule fires.

**The targeted check on the two new primitives:** any *call to*
`add_calendar_months` or `add_calendar_years` (a use of the
identifier as a function name in a parse-tree call node — not the
identifier as a substring inside a comment or string literal) in an
unanchored model emits one focused error rather than the type-cascade
through E220. The check fires post-parse on the resolved AST. The
single error gives the user the shortest path to the real fix:

> E3xx: `add_calendar_months` is a calendar-stepping function and
> requires the model to be calendar-anchored. Add
> `origin = date("YYYY-MM-DD")` at the top of the file, or — if you
> wanted an affine offset on the abstract time axis — use a duration
> literal like `30 'days`.

The same check on `add_calendar_years`.

This is the contract the rest of §3–§4 builds on, and it should land
in the cheatsheet's anchored/unanchored summary so the contract is
visible at orientation time rather than buried in the spec.

## 2. What the language already provides (and isn't changing)

For a colleague reviewing this proposal fresh: the language already has
most of the calendar-first surface that earlier drafts of this proposal
were proposing to add. The proposal *narrowly* adds two rules and two
functions; everything below is already in the lexer, parser, expander,
and dimensional checker, and is documented in
`docs/camdl-language-spec.md` §2 and `docs/user-features.md`.

- **Numeric-literal-with-unit:** `5 'days`, `14 'days`, `5 'years`,
  `0.087 'per_month`, `0.02 'per_year`. Prefix-apostrophe unit syntax —
  `<number> <unit-literal>`. The dimensional checker treats `'days` as
  `[T]` and `'per_day` as `[T⁻¹]`; mixed-unit arithmetic normalises to
  the model's `time_unit` at compile time.
- **Unit literals:** `'days`, `'weeks`, `'months`, `'years`,
  `'per_day`, `'per_week`, `'per_month`, `'per_year`, `'count`,
  `'ratio`. All defined in `lexer.mll` and dimchecked in
  `dimcheck.ml`. Conversion table: `1 'week = 7 'days`,
  `1 'month = 365.2425/12 'days ≈ 30.4369`,
  `1 'year = 365.2425 'days`. Proleptic-Gregorian throughout.
  Verified by

  ```
  $ rg -n '365|30\.4' ocaml/lib/compiler/expander.ml rust/crates/ir/src/caltime.rs
  ocaml/lib/compiler/expander.ml:117:    Use the same Gregorian constant (365.2425) everywhere.
  ocaml/lib/compiler/expander.ml:121:    | Months | PerMonth -> 365.2425 /. 12.0
  ocaml/lib/compiler/expander.ml:122:    | Years  | PerYear  -> 365.2425
  ocaml/lib/compiler/expander.ml:572:    | Months  -> 365.2425 /. 12.0 | PerMonth -> 365.2425 /. 12.0
  ocaml/lib/compiler/expander.ml:573:    | Years   -> 365.2425         | PerYear  -> 365.2425
  rust/crates/ir/src/caltime.rs:70: "months" => Ok(365.2425 / 12.0),
  rust/crates/ir/src/caltime.rs:71: "years"  => Ok(365.2425),
  ```

  Both sides agree on the Gregorian average. (An earlier version of
  this proposal claimed an OCaml/Rust disagreement; that claim was
  unverified and incorrect — the spec text §2.1 had a stale Julian
  value but the code did not. Spec corrected separately.)
- **Three tiers of dimensional information** (spec §2.3): kind keywords
  (`rate`, `probability`, `count`, `positive`, `real`, `instant`,
  `duration`), bracket annotations (`[T]`, `[T⁻¹]`, `[P]`, `[P/T]`,
  `[1]`), and unit literals (tier 3, carrying scale).
- **Parameter type kinds** including `instant` and `duration`, both
  carrying dimension `[T]`. `instant`-kind parameters render as dates in
  `fit summary` (requires `origin`); `duration`-kind parameters render
  as spans.
- **Date literals:** `date("YYYY-MM-DD")` in constant positions
  (`origin`, `simulate { from, to }`, scheduled event/intervention
  times). Requires `origin`; E220 if not declared.
- **Range syntax in periodic forcings:** `on = [7:100, 115:199,
  252:300, 308:356]` — lo:hi ranges materialise at compile time given
  `step = ... 'days` and `period = ... 'days`.
- **Forcing unit literals** (tier-3 required, GH #8): every forcing
  declaration carries a unit literal between the kind keyword and the
  block — `sinusoidal 'ratio { ... }`, `interpolated 'count { ... }`,
  etc.
- **Dimensional error codes:** E300 (transition rate wrong dimension),
  E301 (non-dimensionless argument to `exp`/`log`), E302
  (addition/subtraction of mismatched dimensions), E303 (inconsistent
  parameter use across transitions), E304 (`sqrt` of odd dimension),
  E305 (balance must have dimension P), E306 (ODE derivative must have
  dimension P·T⁻¹), E308 (overdispersion σ² must be dimensionless).

If you find yourself reaching for a new construct while reading this
proposal, check that list — the language probably already supports it.

## 3. The two rules

The whole correctness story compresses to two compile-time rules. They
share a single root principle: *if the language admits an expression
whose meaning depends on which days-per-month constant is used, the
expression is silently wrong.* Both rules forbid exactly those
expressions and route the user to a correct alternative with a
fix-hint diagnostic.

### Rule 1 — Calendar durations don't translate instants

A calendar month is not an *invertible* span — and "+1 month" itself
depends on which year you're in. Take `date("2021-01-31")` (non-leap):
`add_calendar_months(date("2021-01-31"), +1) = date("2021-02-28")` —
the day-of-month clamps from 31 to 28 because Feb 2021 has 28 days.
Going back one month:
`add_calendar_months(date("2021-02-28"), −1) = date("2021-01-28")`,
not Jan 31 — the day-of-month was lost to clamping. The leap-year
case has the same shape with different numbers:
`date("2020-01-31") → date("2020-02-29") → date("2020-01-29")`.
Either way, the round trip lands somewhere other than the start. For
any genuine duration `d`, `(x + d) − d = x`; calendar-month-stepping
breaks that. So "+1 calendar month" is mathematically an *operation on
a date* (`Instant → Instant`), not a duration that can be added
anywhere.

#### How the type system tells them apart — the Exact/Calendar refinement

The dimensional checker already synthesises a dimension for every
expression bottom-up — `'days` is `[T]`, `'per_day` is `[T⁻¹]`, and so
on. Rule 1 adds one bit alongside `[T]`: a classifier that says
"is this magnitude derived from the affine month/year constant?"
Following standard usage we call the two values `Exact` and `Calendar`,
and they form a subtype relation:

> **`Exact <: Calendar`.** An exact duration can be used anywhere a
> calendar duration can, *plus* it can translate an instant. A
> calendar duration can't translate an instant, so it isn't usable
> everywhere exact is. Exact is the more capable subtype.

It's a static type-level refinement, not a runtime property and not a
dataflow analysis — at runtime every duration is just an `f64` number
of axis-units. The classifier rides on the dimcheck pass dimcheck
already does. We considered the word "taint" for this and rejected it:
it imports dataflow-analysis baggage that isn't applicable here.

**Where each value comes from** (the synthesis rule at leaves):

| Leaf form                                | Classification |
|------------------------------------------|----------------|
| `'days`, `'weeks` literal                | `Exact`        |
| `'months`, `'years` literal              | `Calendar`     |
| `Instant − Instant`                      | `Exact`        |
| Reference to a `duration`-kind parameter | `Exact`        |
| Reference to a `[T]`-annotated parameter | `Exact`        |

**How it propagates through arithmetic** (the synthesis rule at
internal nodes — least upper bound on the subtype lattice):

| Operation                          | Resulting classification          |
|------------------------------------|-----------------------------------|
| `Exact ± Exact`                    | `Exact`                           |
| `Exact ± Calendar`                 | `Calendar` (LUB)                  |
| `Calendar ± Calendar`              | `Calendar`                        |
| scalar × duration, duration × scalar, duration / scalar, −duration | preserves the duration's classification |
| `duration / duration`              | dimensionless; classifier drops (it's no longer a duration, can't translate an instant) |

**The one check**: at `Instant ± duration`, the duration must be `Exact`.
A `Calendar` duration there is an E3xx hard error. The hint:

> E3xx: `5 'months` is a calendar duration and cannot be added to a
> date — calendar months are not invertible (e.g.
> `date("2021-01-31") + 1 month = date("2021-02-28")` because day-31
> clamps to day-28 in Feb 2021, and going back gives
> `date("2021-01-28")`, not Jan 31). For a calendar-exact date use
> `add_calendar_months(date("..."), 5)`. For an explicit affine span
> use `152 'days`.

The laundered case (`let d = 6 'months; date(...) + d`) is caught for
free by this scheme: `d`'s synthesised type contains a `'months` leaf
so the LUB walks up to `[T, Calendar]`; the `let` propagates the
synthesised type unchanged; the check at `date(...) + d` sees
`Calendar` and rejects it. No special-casing of `let` needed.

#### The invariant that prevents the parameter-bounds trap

Reading the propagation table above, you might pause at one row: a
`duration`-kind parameter reference is `Exact`. But what about
`delay : duration in [1 'months, 6 'months]` — does the parameter
inherit `Calendar` from its bound spelling? **No, and getting this
wrong breaks bread-and-butter models.**

The trap is that a declaration mentions time units in two unrelated
roles. The **kind** `duration` is the parameter's *type* — a `[T]`
quantity whose runtime value is just a number of axis-units, to be
filled in by inference. The **bounds** `[1 'months, 6 'months]` are
concrete *literals* the expander evaluates at compile time (via the
affine month constant) into the numeric day-range
`[30.4369, 182.6]`. Bounds are metadata fencing where the sampler
may put `delay`; they are not the parameter's value.

The failure mode if you conflate them: take a seed-timing model with
`tau : instant` (estimated introduction date) and
`delay : duration in [1 'months, 6 'months]` (estimated lag, bound
spelling chosen because the literature reports the range in months).
Somewhere downstream you write `tau + delay` — the expected detection
instant, ordinary. A naive implementer who classifies `delay` by
*scanning its declaration for unit literals* sees `'months` and tags
`delay` `Calendar`. Then `tau + delay` is `Instant + Calendar →
E3xx`, suggesting `add_calendar_months(delay, ...)` on a continuous
fitted quantity. Nonsense.

The semantic reason it's wrong, not just annoying: the `Calendar`
classification flags *exactly one* hazard — a magnitude derived from
the affine month constant being added to a calendar instant, where
the user might have meant calendar-stepping. `delay` isn't that. Its
runtime value is a fixed number of days; `tau + delay` advances by
that many days, exact and invertible. Affine months as **lengths**
are well-defined; they're only ambiguous as **steps from a date**. A
bound is a length-use, never a step-from-a-date, so its month-spelling
is harmless shorthand.

The synthesis rule that makes this airtight, and the line whoever
implements dimcheck must implement exactly: **classification is
synthesised per expression occurrence from that occurrence's leaves;
it is never a sticky attribute read off a parameter declaration.** At
the occurrence `tau + delay`, the leaf `delay` is a parameter
reference, and its classifier comes from its declared **kind**:
`duration → Exact`. The bounds `1 'months` and `6 'months` are
synthesised in their own context (the bound annotation), where each
is `[T, Calendar]`. But a bound's classifier gates only two things —
dimensional agreement with the kind (`[T]` vs `[T]`, fine) and
compile-time evaluability (yields 30.44 and 182.6, fine). A bound is
never added to an instant; its `Calendar` bit reaches no sink and is
discarded into the numeric range. It does not leak to uses of
`delay`.

**Stated as a one-line invariant:**

> **`Calendar` is a property only `'months`/`'years` unit literals can
> originate.** Parameter references with `[T]` dimension and instant
> differences are *always* `Exact`. There is no `calendar_duration`
> parameter kind, and there can't be one — "step N calendar months"
> isn't a scalar to fit, it's the operation `add_calendar_months(d, n)`
> whose `n` is a discrete count.

(Bare numbers — `5.0`, not `5 'months` — never reach this rule at all:
they're dimensionless `[1]`, and `date + 5.0` is the existing E302
(time + dimensionless), upstream of the Exact/Calendar classifier.
The Exact/Calendar classifier only sees `[T]`-dimensioned operands.)

#### Rule 1 covers recurring schedules too (stated statically)

The "`Instant + duration` requires `Exact`" rule catches the explicit-
arithmetic case (`date(...) + 6 'months`). It needs to extend to
recurring schedules on interventions and events
(`docs/camdl-language-spec.md` §14.2):

```
NAME : ACTION {
  every = DURATION
  from  = DURATION
  until = DURATION
}
```

Stated statically — without reference to runtime fire-time computation
— the rule is: **in anchored mode, the duration-typed value of
`every`, `from`, or `until` must be `Exact`.** The dimcheck synthesises
the classifier of the `DURATION` expression bottom-up and rejects any
`every`/`from`/`until` whose classifier is `Calendar`. Calendar `every`
is the same E3xx as `date + 6 'months`, with the same hint shape —
pointing to `every = 30 'days` for an affine ~monthly recurrence, or
to an explicit calendar-listed schedule for true month-aligned
recurrence.

In unanchored mode there's no calendar reference at all, so Rule 1 is
vacuous and `every = 1 'months` is fine.

### Rule 2 — Anchored mode requires a constant-day axis

`time_unit = 'months` or `time_unit = 'years` is forbidden when
`origin = date(...)` is declared. **E3xx hard error**, with the
following hint (expanded to surface the migration trap that
accompanies the axis change):

> E3xx: `time_unit = 'months` cannot be combined with
> `origin = date("...")` — the date↔number conversion would drift
> because a calendar month is not a constant number of days. Switch
> to `time_unit = 'days` (or `'weeks`).
>
> When you switch the axis, every *bare-numeric* time position in
> your model silently changes meaning to the new axis:
> `simulate { from/to/dt }`, `at [...]` schedules on interventions
> and events, and the time column of any `--data` file. Annotate
> each with a unit literal (e.g. `to = 600 'months`) or a date
> literal (e.g. `to = date("1940-12-01")`) to preserve intent.
>
> Typed positions are unaffected: rate parameters declared with
> `'per_month` continue to work (the expander converts), and
> duration values like `1 'months` continue to work as affine spans
> (≈ 30.44 days).
>
> Full migration walkthrough:
> `docs/dates.md#migrating-an-unanchored-monthly-model-to-anchored`.

The reason has nothing to do with arbitrary policy: the date↔number
conversion must be *non-drifting* under repeated conversion, which
requires the divisor to be a constant number of days (`'days = 1`,
`'weeks = 7`). Months and years use *average* day-lengths (30.4369,
365.2425), so a long-running `'years`-axis date renderer mis-aligns
to the calendar by an accumulating residual: with
`origin = date("2020-01-01")` and a `'years` axis, `t = 5` is affine
1826.2125 days, landing 2024-12-31 — not 2025-01-01, which is five
real calendar years and 1827 days away (two leap years in the
span). The drift is small per step but real, and visible the first
time a user compares a rendered date to a calendar.

This is the same principle as Rule 1, applied to the axis rather
than to individual durations.

#### Bare-numeric time positions in anchored mode — W3xx warning

Rule 2 keeps `time_unit = 'months/'years` out of anchored mode, but
it doesn't catch the *companion* trap the migration hint warns about:
in anchored mode, *every* bare-numeric time position
(`simulate.from`, `simulate.to`, `simulate.dt`, `at [k, ...]`
schedules in interventions and events, and `--data` numeric time
columns) silently means "internal time units from origin" — which
is correct if the user means it, but is exactly where the
silent-shift trap lives during migrations.

The proposal therefore introduces **W3xx**: in anchored mode, any
bare-numeric value in a time-typed position emits a warning with
hint:

> W3xx: `to = 600` is a bare number in a time position, with
> `origin = date("...")` declared — interpreted as 600 in the
> model's `time_unit`. To make the intent explicit, write
> `to = 600 'days` (or `'weeks`/`'months` — converted via
> `days_per_unit`), or use a date literal like
> `to = date("1940-12-01")`. To suppress this warning intentionally
> (legacy/internal-time semantics), pass
> `--time-format internal-days` for the `--data` time column or
> annotate the position explicitly.

Numeric data columns with `origin` declared remain a *hard* error
per §4.5 (same family, stricter rule at the I/O boundary because
data files are the most common silent-shift surface). `on=[...]`
bare-numeric in periodic forcings with `origin` also remain a hard
error per §4.4.

The warning-versus-error split: the language already accepts
`simulate { to = 18260 }` as a legitimate "internal days from
origin" idiom (the legacy and SBC-style cases). Warning preserves
that. Numeric data columns and periodic-forcing `on=[]` lists are
the surfaces where the legitimate case is rare and the trap is
overwhelming — hard error there.

One precision subtlety worth naming: the date↔t round-trip is
**bit-exact** for `'days` (integer rata-die difference, divisor of 1)
and **ULP-precise** for `'weeks` (divisor of 7 is exact, but `1/7`
isn't representable in `f64`, so a date that doesn't land on a
7-day boundary from `origin` has a fractional `t` that's
ULP-correct but not exact). Both are categorically safe — they don't
*drift* under repeated conversion the way months and years would.
The same caveat already applies to off-grid observation times under
`'days`, which `docs/dates.md` accepts.

The unanchored case is unaffected: `time_unit = 'months` with no
`origin` is fine (the dacca configuration). The constant-day rule only
fires when an `origin` is present, because that's the only situation
where the axis scale becomes a *conversion factor*.

### Why two rules instead of one big "no calendar units in anchored mode"

You might be tempted to fold Rule 2 into Rule 1 (just forbid any
calendar unit in anchored mode). That would be wrong — and the
distinction is what lets the dacca-style "I want calendar dates *and*
per-month rates" case work cleanly.

In anchored mode under these two rules:

- **Affine durations** like `5 'months` as a table value or parameter
  bound — *allowed.* It's just a number of days
  (`5 × 30.4369 ≈ 152.18 'days`); the dimensional checker accepts it
  wherever a duration is acceptable, as long as it isn't being added
  to an Instant.
- **Rates** like `0.087 'per_month` — *allowed.* The
  rate-denominator/axis-unit separation is already in the language;
  the expander converts to per-axis-unit at compile time. A user can
  write `beta : rate 'per_month` in a `time_unit = 'days` anchored
  model and get King-style per-month rate values with no manual
  conversion.
- **Calendar stepping** with `add_calendar_months(d, 6)` — *the only
  way to advance a date by 6 calendar months* (Rule 1 closed the
  other paths).
- **Calendar axis** `time_unit = 'months` — *forbidden* (Rule 2).

That single configuration — *anchored, with per-month rate parameters,
on a daily axis, using `add_calendar_months` for any explicit calendar
stepping* — is what the dacca chapter would migrate to if it ever
wanted calendar dates. The migration is a few lines and no rescaling.

## 4. Two new primitive functions

`add_calendar_months : (Instant, Int) → Instant`
`add_calendar_years  : (Instant, Int) → Instant`

These are the *only* way to step a date by calendar months/years in the
language. Compile-time functions in the expander (DSL constant
positions only); they do not exist as runtime values that flow through
the IR.

**Admissible argument forms.** The date argument `d` is any
compile-time-constant `Instant` expression:

- a `date("YYYY-MM-DD")` literal;
- the reserved `origin` identifier (in anchored mode, per §1.1);
- a nested `add_calendar_months` / `add_calendar_years` call (the
  result is itself a compile-time-constant `Instant`).

The `n` argument is a compile-time integer constant.

So `add_calendar_months(origin, 6)`,
`add_calendar_months(date("2020-02-24"), 3)`, and
`add_calendar_months(add_calendar_years(origin, 1), 6)` are all legal.

**Not yet admissible: table-lookup entries.** The spec's per-patch
scheduling form (`docs/camdl-language-spec.md` §14.3 at line 1986)
uses tables like `sia_day : patch × round = read(...)` — *unitless*,
numeric. Verified by

```
$ rg -n 'sia_day|table.*Instant' docs/camdl-language-spec.md ocaml/lib/ir/*.ml | head
docs/camdl-language-spec.md:2015:  sia_day : patch × round = read("data/sia_schedule.tsv")
docs/camdl-language-spec.md:2020:    at [sia_day[p, 0], sia_day[p, 1]]
```

Table entries are not Instant-typed today, so
`add_calendar_months(sia_day[p, 0], 3)` doesn't typecheck under this
proposal. Adding date-valued tables (e.g. `sia_day : patch × round 'date = read(...)`)
is a separate language change; flagged as a probable follow-up once
this proposal lands and the per-patch calendar-stepping idiom is
demanded in practice.

**Month-end clamping** is the canonical algorithm:

```
add_calendar_months(date(y, m, d), n):
  m' = ((m - 1 + n) mod 12) + 1
  y' = y + (m - 1 + n) div 12
  d' = min(d, days_in_month(y', m'))
  return date(y', m', d')
```

Examples:

- `add_calendar_months(date("2020-01-31"), 1) = date("2020-02-29")` (leap)
- `add_calendar_months(date("2021-01-31"), 1) = date("2021-02-28")`
- `add_calendar_months(date("2020-01-31"), 13) = date("2021-02-28")`
- `add_calendar_years(date("2020-02-29"), 1) = date("2021-02-28")`

These functions are **constant-free** in the days-per-month sense: they
do real `(year, month, day)` arithmetic via the proleptic-Gregorian
calendar, never touching the `30.4369` average-month factor. That's the
final confirmation that the two machineries (affine duration arithmetic
and calendar arithmetic) don't even share a constant.

**Non-invertibility is documented at the function definition.**
`add_calendar_months(date("2020-01-31"), 1)` then `(date, −1)` is
*not* in general `date("2020-01-31")` — the day-of-month is lost to
clamping. The docstring states this explicitly; an optional warning
W3xx fires on the literal nested form
`add_calendar_months(add_calendar_months(d, n), -n)` in DSL constant
positions to flag the assumption. **Scope note:** the warning matches
this single syntactic shape only; let-separated or otherwise-spelled
equivalents will not trigger it. The docstring carries the rest of
the non-invertibility story; the warning catches the most common
literal mistake, not all of them.

**No `date_range` function in this proposal.** Three cases cover the
realistic space:

- **Repeating seasonal/school structure**: handled by the existing
  periodic forcings with `on = [7:100, 115:199]` range syntax and
  `step`/`period` unit-bearing fields.
- **Data-driven monthly/yearly covariates** (the "120 breakpoints from
  a real dataset" case): handled by `interpolated` forcings reading
  a dated TSV. This is the escape valve — when the cadence list is
  long because the data is real, the data file is where the cadence
  belongs, not the source.
- **Small-N calendar-aligned breakpoints** (e.g. quarterly
  intervention dates over a couple of years): hand-listed with
  `add_calendar_months` applications in DSL constant positions —
  explicit, finite, no engine support needed.

So `date_range` is deferred *conditionally*: if a future audit of
real models turns up recurring hand-listed monthly breakpoint lists
beyond what `interpolated` covers, that's the signal to add it. The
language already has enough surface for the common cases.

## 5. Diagnostics — every new error has an E-code and a fix-hint

Per the CLAUDE.md "errors are a feature" stance, every new diagnostic
this proposal introduces ships with both an E-code and a fix-hint. The
non-exhaustive catalog:

| Code | Trigger                                                                                  | Hint shape                                                            |
|------|------------------------------------------------------------------------------------------|-----------------------------------------------------------------------|
| E3xx | `Instant + CalendarDuration` (literal or LUB-propagated through `let`)                   | "calendar months/years aren't invertible; use `add_calendar_*`"        |
| E3xx | `Calendar`-classified duration in `every`/`from`/`until` of anchored recurring schedule  | "calendar cadence not allowed in anchored recurring schedule"          |
| E3xx | `time_unit = 'months` (or `'years`) with `origin` declared                               | "constant-day axis required; use `'days`, keep per-month rates"        |
| E3xx | call to `add_calendar_months` / `add_calendar_years` in an unanchored model              | "calendar stepping requires anchored mode; add `origin = date(...)`"   |
| E4xx | bare-numeric `--data` time column with `origin` declared                                 | "use ISO dates or `--time-format internal-days`"                       |
| E3xx | bare-numeric entries inside `on=[...]` periodic-forcing list with `origin` declared      | "use `date(...)` entries or `--time-format internal-days` opt-in"      |
| W3xx | bare-numeric time position in anchored mode (`simulate.from/to/dt`, `at [k, ...]`)       | "annotate with `'days` or use a date literal; or opt-in to legacy"    |
| W3xx | `add_calendar_months(add_calendar_months(d, n), -n)` literal nested round-trip           | "month-end clamping is non-invertible; result is not the input"        |

The retrospective hint-quality audit of *existing* E-codes is
intentionally out of scope here and deferred to a follow-up proposal.

## 6. The judgment call landed: hard error on `date + N 'months`

The single ergonomic question this proposal makes a call on rather than
deferring: when a user writes `date("2020-02-24") + 6 'months`, do we
hard-error or proceed with the affine result and warn?

**Hard error.** The reasons:

1. Silent affine drift on a date is exactly the bug class this
   proposal exists to close. A warning that proceeds with the wrong
   answer is a warning agents and time-pressed users will skim past.
2. The fix is one line and visible in the error message: the user
   gets a clear pointer to `add_calendar_months(d, 6)` (for
   calendar-exact) or `182 'days` (for deliberate affine offset).
   Neither requires the user to internalise a constant or remember a
   convention.
3. The keystroke cost is small; the safety gain is large; the
   alternative (a warning) trains users to ignore future warnings,
   including the ones that matter.

This applies symmetrically to the laundered case
(`let d = 6 'months; date(...) + d`) via the Calendar-classifier
LUB propagation rule in §3.

## 7. Implementation phasing

Phases are ordered so each ships independently and so behaviour
changes precede pure structural refactors.

### Phase 1 — Rules 1 & 2 in the dimensional checker

- Refine the `duration` kind in `dimcheck.ml` to distinguish
  `ExactDuration` from `CalendarDuration`.
- Implement the Calendar-classifier LUB propagation rule.
- Add E3xx for `Instant + CalendarDuration` with hint text.
- Add E3xx for `time_unit = 'months/'years` with `origin` declared
  with hint text.
- Tests: positive cases (legal `5 'months` table values, legal
  `0.087 'per_month` rate parameters in anchored mode); negative cases
  (rejected `date(...) + 6 'months`, rejected `time_unit = 'months`
  under `origin`).

Additive. The dacca SIRS models continue to compile unchanged
(unanchored, no `Instant`).

### Phase 2 — calendar-arithmetic primitives

- `add_calendar_months(d, n)` and `add_calendar_years(d, n)` as
  expander functions, in DSL constant positions only.
- Month-end clamping with proleptic-Gregorian `days_in_month`.
- W3xx warning on round-trip composition.
- Tests covering the canonical cases — each with explicit years
  since the operation is year-dependent:
  - `date("2020-01-31") + 1 month → date("2020-02-29")` (leap)
  - `date("2021-01-31") + 1 month → date("2021-02-28")` (non-leap)
  - `date("2020-02-29") + 1 year → date("2021-02-28")` (leap → non-leap year-end clamp)
  - `date("2020-01-31") + 13 months → date("2021-02-28")` (large `n` crosses year-end)
  - Leap-year transitions in both directions.

Required because Rule 1 closes the other paths to calendar stepping.

### Phase 3 — documentation refinement

This is the bit the colleague should specifically push back on if they
see anything missing.

- **`docs/dates.md`**: add the anchored / unanchored vocabulary up
  front. Add the duration-kind split (exact vs calendar). Add the
  constant-day rule for anchored mode. Add the
  `add_calendar_months`/`add_calendar_years` primitives. Add the
  "Migrating an unanchored monthly model to anchored" section
  (already drafted ahead of Phase 3 — see `docs/dates.md`) and the
  "Why `'months` is fine in some places and forbidden in others"
  section. Both are anchor-only-orientation aids and are the
  user-facing complement to Rule 2's hint text.
- **`docs/camdl-language-spec.md` §2**: update §2.1 with the
  duration-kind split and the corrected constants. Update §4.1
  (parameter type kinds) to note that `duration` is now the umbrella
  for ExactDuration | CalendarDuration. Update §7 if any forcing
  semantics shift.
- **`docs/user-features.md`**: a short section showing the canonical
  patterns — anchored model with per-month rates, unanchored
  monthly-axis model (the dacca shape), calendar stepping via
  `add_calendar_months`.
- **`docs/dsl-cheatsheet.md`** (new, see §9): tight orientation doc
  pointing at the normative sources.

### Optional Phase 4 — engine-side typed time

The original draft of this proposal proposed an `Instant` torsor over
`Duration` in the Rust sim crate. That's still defensible as a follow-up
(it would make engine-side fabrication like `t: 0.0` in `EvalCtx`
unrepresentable rather than guarded-against), but it's *not* needed to
close the bug classes Phases 1–3 close. The obs-eval fix (commit
`e69516d`) already addressed the specific incident that motivated the
torsor idea; the typed-time engine refactor is a durability investment
that can ship later when the cost/benefit is more favourable.

If we revisit, the original draft is in `git log` and can be revived as
its own proposal.

## 8. Acceptance criteria

Per phase:

**Phase 1.** All existing models still compile. Each new diagnostic has
a test case asserting both that it fires when expected and that the
emitted message contains the documented hint text. The dacca SIRS
models compile unchanged.

**Phase 2.** Documented examples for `add_calendar_months` /
`add_calendar_years` round-trip identically through the IR and the
expander. Month-end clamping is covered by tests at:
- `date("2020-01-31") + 1 month → date("2020-02-29")` (leap)
- `date("2021-01-31") + 1 month → date("2021-02-28")` (non-leap)
- `date("2020-02-29") + 1 year → date("2021-02-28")` (year-end clamp)
- `date("2020-03-31") + 1 month → date("2020-04-30")` (going forward into Apr; year-independent)
- `date("2020-03-31") − 1 month → date("2020-02-29")` (going back, leap)
- `date("2021-03-31") − 1 month → date("2021-02-28")` (going back, non-leap)

**Phase 3.** Each doc named in §7 has the named subsection or
update. The cheatsheet is ≤ 2 pages.

## 9. Companion artifacts shipping with this proposal

Two small additions that prevent recurrence of the failure modes this
proposal corrects:

1. **`docs/dsl-cheatsheet.md`** — a tight 1–2 page orientation doc.
   What unit literals exist; what dimensional kinds exist; the three
   tiers (kind / bracket / unit literal); the two duration sub-kinds
   (after Phase 1); the calendar primitives (after Phase 2). The
   cheatsheet is *not* the normative source — it points at
   `docs/camdl-language-spec.md` for that — but it's the first thing
   an agent or new contributor should read when working on DSL
   changes.

2. **CLAUDE.md "Required reading before proposing X"** — a small
   subsection in `CLAUDE.md` that names the load-bearing docs to read
   before structural proposals. For DSL changes:
   `docs/camdl-language-spec.md` + `docs/user-features.md` + the
   cheatsheet + `parser.mly` / `lexer.mll` unit-literal sections. For
   IR changes: `ir/schema.json` + the calendar-time proposal. The
   point is to make the required-reading discipline durable, not
   something a future agent has to discover by missing the same
   things I missed.

Both are durable instructions to the next agent (or to me on a fresh
context) and ship as separate small commits after the proposal lands.

## 10. Out of scope

- The engine-side `Instant`/`Duration` torsor (deferred per §7, Phase
  4 optional).
- A `date_range` generator (§4 — not needed; the existing range
  syntax and explicit `add_calendar_*` calls cover the cases we have).
- Method-style duration syntax (`7.days`, `1.month`) — `5 'days`
  already exists.
- Retrospective audit of existing E-code hint quality (§5).
- `today()` literals (non-reproducible; permanent non-goal).
- Sub-day units, times of day, timezone-aware dates (already non-goals
  per `docs/dates.md`).

## 11. References

- Triggering commit: `e69516d` (obs-eval frozen-`t` fix).
- Prior calendar-time proposal:
  [`docs/dev/proposals/2026-05-22-calendar-time.md`](2026-05-22-calendar-time.md).
- Calendar reference: [`docs/dates.md`](../../dates.md).
- Language spec: [`docs/camdl-language-spec.md`](../../camdl-language-spec.md)
  (§2 in particular).
- DSL design principle:
  [`CLAUDE.md`](../../../CLAUDE.md) §"Design the DSL for humans
  first; agents follow."
