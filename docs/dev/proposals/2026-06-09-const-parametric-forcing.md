# Const‖Parametric forcing: keep parameter-referencing coefficients live (and differentiable)

- **Status:** Draft **v2** — reworked after an adversarial review (2026-06-09,
  17 confirmed findings incl. 3 blockers) that corrected v1's central design.
  Changelog at end. This proposal is the **implementation plan** for examples
  **§7 (gradient half)** and **§8 (value half)** of
  [`docs/dev/notes/2026-06-08-static-typing-as-bug-prevention.md`](../notes/2026-06-08-static-typing-as-bug-prevention.md),
  which already designed the typed fix; read those two examples first — this doc
  does not re-derive them, it stages and tests them against the now-filed
  incident.
- **Fixes:** the freeze in
  [`docs/dev/incidents/2026-06-09-forcing-coefficient-param-frozen-at-construction.md`](../incidents/2026-06-09-forcing-coefficient-param-frozen-at-construction.md).
  Both halves of gh#119; plus gh#186 (value), gh#128/gh#180 (gradient family).
- **Required reading:** the incident; the static-typing note §7+§8;
  `docs/camdl-language-spec.md` §7 (line ~1080 advertises the broken feature);
  the [typed-parameter-surface proposal](2026-06-08-typed-parameter-surface.md).
- **Discrepancy class:** **code-vs-code** (rate eval live, forcing/table eval
  frozen — same `Expr::Param`) + **doc-vs-code** (spec promises the feature;
  code is the loser → sync code to spec).
- **Correction to v1's framing:** v1 claimed "no inference edits." **That is
  wrong.** The gradient half edits the OCaml autodiff (`autodiff.ml`), which is
  the _only_ source of the gradients PGAS+NUTS consumes. The value half alone
  fixes the gradient-free methods (IF2, bootstrap PF) but leaves NUTS broken in
  a _worse_ way (value responds, gradient stays zero — they disagree). §4 is the
  core, not a footnote.

## 1. What is broken (with the reproduction and the real blast radius)

The spec promises a forcing coefficient may reference a parameter "for
inference" (`docs/camdl-language-spec.md:1080`, `amplitude = alpha`). The
runtime silently breaks it: at `CompiledModel::new` every coefficient is
collapsed to one `f64` and cached; during a fit the model is built once and
parameters vary as a borrowed slice, so the cached value never updates. Concrete
reproduction (build once, vary `amp` only in the live slice → byte-identical
trajectory; vary `sigma`, used in a rate → totally different) is in the
incident.

All six forcing variants and inline tables bake every coefficient
(`compiled_model.rs:724-840`): `Sinusoidal` (amplitude/period/phase/baseline),
`Piecewise` (breakpoints[]/values[]), `Interpolated` (times[]/values[]),
`Periodic` (period/values[]), `Fourier` (period/harmonics[]), `PeriodicSpline`
(period/coefs[]), inline `table` (values[]).

**Blast radius — four committed goldens, not one** (verified by `jq` over
`ocaml/golden/*.ir.json`; no inline table references a param):

| golden                    | forcing param(s)               | IR `ParamValue` |
| ------------------------- | ------------------------------ | --------------- |
| `seir_seasonal_patch`     | amp_urban, amp_rural, baseline | **Estimated**   |
| `seir_vaccine_seasonal`   | alpha, phi_season (spec ex.)   | **Required**    |
| `seir_pop_balance`        | pop_amp, pop_mean              | **Required**    |
| `phenom_mixing_unchecked` | amp                            | **Required**    |

Any past fit of these has a posterior that reflects only the prior for the
forced parameter. **Three of four are `Required`** — see §3, this is why keying
on `Estimated` is wrong, and why a **user-facing advisory** is warranted (§7).

## 2. Why it happens

### 2.1 Rates live; forcing/table coefficients frozen; everything else live

- **Rate (live):** `ResolvedExpr::Param(idx) => ctx.params[idx]`
  (`resolved_expr.rs:408`) reads the live slice.
- **Forcing/table coefficient (frozen):**
  `eval_table_expr(expr, &param_index,
  &default_params)` at 16 sites collapses
  to `f64`; `Expr::Param(p) =>
  Ok(params[idx])` (`compiled_model.rs:277-281`)
  resolves against the _construction_ vector, no error.
- **The read can't recover:** `eval_time_func(&cache[idx].kind, ctx.t)`
  (`propensity.rs:186`, `resolved_expr.rs:510`); `eval_time_func(kind, t)`
  (`:340`) takes no params.
- **Verified NOT siblings** (review): initial conditions
  (`compiled_model.rs:1154-1169`), `balance`, `events`/`interventions`
  (`effects.rs`), ODE equations, parametric schedules, and observation models
  (`obs_model.rs:60-136`) all evaluate live against the per-iteration params
  slice. The freeze is confined to `time_func_cache` + `table_values_cache`.

### 2.2 The two halves (and why the value half alone is not enough)

`#119` is two bugs sharing one parameter:

- **Value half (§8 of the note):** the frozen `f64` cache above. Fixing it makes
  the _likelihood_ respond to the parameter — which fixes **IF2** and the
  **bootstrap particle filter** (gradient-free).
- **Gradient half (§7 of the note):** `autodiff.ml:23-24` differentiates
  `TimeFunc _ -> Const 0.0` and `TableLookup _ -> Const 0.0` unconditionally, so
  the compiler-emitted `rate_grad` for `seasonal[p]*S*I/N` carries **no entry**
  for `amp` (verified: `seir_seasonal_patch` `infection_*` transitions have null
  `rate_grad`). `eval_expr_deriv` mirrors it (`propensity.rs:253-255`). So
  **PGAS+NUTS** (the production Bayesian method) sees ∂loglik/∂amp ≡ 0 and never
  moves the parameter by gradient.

If we ship the value half alone, NUTS gets a _responsive likelihood with a zero
gradient_ — they disagree, which is a worse, subtler failure than the original
uniform freeze. `gradient_check.rs:448-454` already documents this
"doubly-silent zero gradient" and deliberately avoids estimating
`alpha`/`phi_season`. **Both halves must land before forcing params are
advertised as NUTS-estimable.**

### 2.3 Correction: forcing coefficients are NOT constant-folded

v1 claimed the OCaml fold pre-separates `Const` from `Param` coefficients.
False: `fold_model` (`constant_fold.ml:103-118`) folds only transitions,
bindings, and ODE equations — never `time_functions` or `tables` (and it
short-circuits when there are no inline tables, `:105`). A coefficient like
`amplitude = 2 * 0.15` reaches Rust as a `BinOp`, not `Const`. The build-time
decision therefore relies on a **Rust-side recursive scan**, not on any fold
property.

## 3. Value half: a coefficient is `Const(f64)` or `Live(ResolvedExpr)`

```rust
enum CompiledCoeff { Const(f64), Live(ResolvedExpr) }
```

**Build-time decision — key on "references ANY parameter", not `Estimated`.**
Use the **existing** `expr_refs_param` predicate (`compiled_model.rs:236`,
recursive, already used for bindings at `:673`). If a coefficient references any
param → `Live(resolve_expr(expr))`; else → `Const(eval_table_expr(...))`.

Why not key on `ParamValue::Estimated` (v1's error, a confirmed **blocker**):
the fit pipeline estimates parameters that are **`Required` or `Fixed` in the
IR, not `Estimated`**. A bare `alpha : probability` compiles to `Required`
(`expander.ml:3234`); put under `[estimate]`, the runner seeds it with
`with_value` (`runner.rs:180`), and `with_value` turns `Required → Fixed`
(`parameter.rs:238`). `CompiledModel::new` receives only the model, never the
`estimate_set` (`runner.rs:211`), so it sees `Fixed` and would bake —
re-freezing 3 of the 4 goldens, including the spec example. Keying on **any
param** is correct regardless of kind: a `Fixed`/`Required` param's coefficient
live-evaluates to the same number it would have baked, so it is never _wrong_;
only param-referencing coefficients pay per-substep eval (the all-constant hot
path is untouched). This is exactly the note's "layering rule": encode the
structural fact (literal vs expression), never the fit-config fact (estimated vs
fixed).

**Preserve the coefficient grammar whitelist** (confirmed major). Today
`eval_table_expr` hard-errors on anything but
`Const/Param/BinOp/UnOp/UncheckedDim` (`compiled_model.rs:328-330`).
`resolve_expr` accepts `Pop/PopSum/Time/Dt/
TimeFunc/TableLookup/BindingRef`
too. The `Live` path must **keep the same whitelist** — reject a coefficient
that reads compartment state, time, or another forcing — preserving today's hard
error. State-dependent forcings are a separate feature with their own
spec/dimcheck rules, not a silent side effect of this fix.

**`eval_time_func` stays a pure `(kind, t)` function; add a dispatch.** It has
**~30 call sites across 3 pure-math test files** (`interpolation.rs`,
`periodic_forcing.rs`, `fourier_oracle.rs`) that pass no context — not "two
callers / two lines" as v1 claimed. Keep `eval_time_func(kind, t)` for the
`Const`-only path (so those tests are untouched), and add
`eval_coeff(&CompiledCoeff, &ctx) -> f64` (Const → the f64; Live →
`eval_resolved(e, ctx)`) used where a forcing field may be live. The two
production read sites (`propensity.rs:186`, `resolved_expr.rs:510`) already hold
`ctx`.

**Splines need a decision (confirmed blocker).** `Interpolated` with
`method = cubic_spline` does not store knot values as `f64` — it runs
`CubicSpline::new` (an O(n) Thomas solve, `compiled_model.rs:46-98`) at
construction to derive b/c/d. `PeriodicSpline` reads `coefs: &[f64]` via a de
Boor evaluator. A `Live` knot would require **rebuilding the spline every
substep**, which `CompiledCoeff::Live` cannot represent. **v1 recommendation:
reject an estimated/param-referencing coefficient in `Interpolated`-spline knots
and `PeriodicSpline` coefs with a clear E-code** (estimating spline knots is
rare and the rebuild cost is the dominant term, not "sin/spline math"); defer
live-spline-coefficient estimation to a follow-up that specifies the rebuild and
measures it.

**Perf: memoize per (substep, t).** Cost is per _referencing transition_, not
per substep: a per-patch `seasonal` referenced by N infection transitions pays N
`eval_resolved` walks per substep per particle (`propensity.rs:461-475`). Add a
small per-step cache (analogous to the binding `CacheScope`) keying each
forcing's scalar on `(idx, t)` so N transitions share one eval — bounding the
regression independent of patch count. The `Const` fast path is a bare f64 load.

## 4. Gradient half: differentiate through `TimeFunc`/`TableLookup` (the core)

The floor is the note §7 ADT — make the dropped derivative explicit so it can
never be a silent zero:

```ocaml
type deriv = Known of expr | Unsupported of { node : string; reason : string }
```

`differentiate_rate` must then _choose_: `Known (Const 0.0)` → omit (genuinely
absent); `Known d` → emit; `Unsupported u` → a compile-time `E`-code naming the
param + forcing.

The **feature** above the floor: emit the analytic ∂forcing/∂coef per kind —
e.g. `Sinusoidal`: ∂/∂amplitude = `sin(2π(t−phase)/period)`, with nonzero
∂/∂phase and ∂/∂period; `Fourier`/`PeriodicSpline`: linear in their coefs. This
requires the gradient evaluator to know **which time-function coefficient each
`TimeFunc` node carries** — today `ResolvedExpr::TimeFunc` is an opaque index
(`resolved_expr.rs:59`). So this half touches `autodiff.ml`, `eval_expr_deriv`
(`propensity.rs:253-255`), and the `TimeFunc` node's gradient representation.
`TableLookup` with a param-dependent value is analogous (a selector/weighted
sum); a param-dependent _index_ is non-differentiable → emit `Unsupported`.

**Scope decision (the central one — see Open Questions):** _v1_ ships the value
half (§3) **plus a NUTS guard** that rejects a fit when an estimated parameter
is referenced _only_ inside a forcing/table coefficient and `use_nuts` is on,
naming the limitation — so IF2/PF work immediately and NUTS fails loud instead
of silently mis-sampling. _v2_ ships the gradient half (this section), lifts the
guard, and makes forcing params NUTS-estimable. Recommended, because the
gradient half is genuine compiler work and gating it keeps the interim correct.

## 5. Tests (corrected — the v1 set was unusable as written)

- **Red test (the repro):** build once, vary the param in the live slice; today
  `assert_eq!(lo, hi)` (frozen) → fix flips to `assert_ne!`. Cover **a
  `Required` case (`seir_vaccine_seasonal`)** and a `Periodic` value and an
  inline table — _not only_ the `Estimated` `seir_seasonal_patch`, which is the
  easy case v1's rule happened to handle.
- **"No dead estimated parameter" invariant — scoped per class** (v1's single
  "propensity changes" assertion false-positives on nearly every golden:
  init-only N0/I0, obs-only rho/k, overdispersion sigma_se, intervention
  vacc_frac all legitimately leave the propensity unchanged). Scope: a param
  that enters a **rate or forcing coefficient** must move the propensity; init
  params → assert `initial_state` changes; obs params → obs-likelihood changes;
  overdispersion → draw variance; intervention → post-fire state. Forcing params
  _do_ enter the rate (via the `TimeFunc` node), so the scoped propensity check
  still catches the target bug.
- **Gradient gate — two stages.** Until §4 lands, the gate is **FD-of-value
  only** (`loglik(θ±ε)`), never analytic-vs-FD — because the analytic gradient
  is structurally zero and `gradient_check.rs` would (correctly) go _red_
  against the value-only fix. After §4, add the analytic-vs-FD check in
  `gradient_check.rs` for forcing params (the existing seasonal test at `:448`
  un-`xfail`ed).
- **Perf guard:** an all-constant forcing stays `Const` (no `eval_resolved`);
  the `Live` Fourier/spline path allocates zero per substep (reused scratch).

## 6. Interim guard — in Rust, post-resolve (not OCaml)

v1 put the guard in the OCaml compiler. **Wrong** (confirmed): OCaml sees
`Estimated`/`Fixed`/`Required` but not the _fit's_ `estimate_set`, so an OCaml
guard both false-positives (a `--fixed alpha` run on an `Estimated`-in-IR param
is _not_ estimating it — freezing is correct) and false-negatives (a bare
`Required` param under `[estimate]` is the at-risk case and OCaml can't see it).
Put the guard in the **Rust fit path after `resolve_parameters`**
(`params_resolver.rs`), where `estimate_set` is known: error iff a param in
`estimate_set` is referenced _only_ inside a forcing/table coefficient. This is
also the NUTS-guard home (§4). It is the right immediate user-protection given
the four affected goldens.

## 7. Blast radius & advisory

The four goldens in §1 are each silently broken for their forcing parameter
today. Because three are the common `Required` pattern and one is the documented
spec example, this warrants a **user-facing advisory** (e.g. in the incident and
release notes): any completed fit of a model that estimates a parameter
appearing only inside a forcing/table coefficient has a posterior reflecting
only the prior for that parameter. The Rust guard (§6) makes this loud going
forward.

## 8. Sequencing & ownership

- **Value half (§3):** `sim` — `compiled_model.rs`, `propensity.rs`,
  `resolved_expr.rs`, plus the `eval_coeff` dispatch and the ~30 pure-math test
  call sites (kept compiling by preserving `eval_time_func(kind, t)`). No
  `inference/*` edits. Independent of RC1/#191.
- **Gradient half (§4):** OCaml `autodiff.ml` + Rust `eval_expr_deriv` + the
  `TimeFunc` gradient representation. This is compiler/gradient work; coordinate
  with RC1 (it also touches the density/grad path) but it does not share files
  with RC1's `pgas*.rs`.
- **`default_params` rename: DEFER.** The active `worktree-param-values` branch
  concurrently edits `compiled_model.rs` (incl. the `default_params` build loop)
  and `effects.rs` and _retains_ the field — renaming now is a live merge
  conflict, not a quiet-lane cleanup. Land the rename inside that branch's work
  or after it merges.

## Open questions for the maintainer

1. **Scope: v1 (value + NUTS-guard) then v2 (gradient), or both at once?**
   Recommend staged — the gradient half is real compiler work and the guard
   keeps the interim correct (IF2/PF fixed immediately, NUTS fails loud).
2. **Splines: reject estimated knots in v1 (recommended), or specify the
   per-substep rebuild now?**
3. **Advisory wording & placement** for the four affected goldens (§7).

## Changelog (v1 → v2, from the adversarial review)

- **Keying corrected (blocker):** `expr_refs_param` (any param), not
  `ParamValue::Estimated` — which misses `Required`→`Fixed` params (3 of 4
  goldens, incl. the spec example). Reuse the existing predicate at `:236`.
- **Gradient half promoted to core (blocker):** `autodiff.ml` zeroes `TimeFunc`;
  value-only ships a NUTS gradient/likelihood _disagreement_. "No inference
  edits" was wrong. Added §4 + the staged scope + the NUTS guard.
- **Splines (blocker):** `CubicSpline` can't be `Live` (construction-time Thomas
  solve); reject estimated knots in v1.
- **Whitelist preserved (major):** keep `eval_table_expr`'s grammar restriction
  on the `Live` path; don't let `resolve_expr` silently admit state-dependent
  forcings.
- **§2.3 corrected (major):** forcings are never constant-folded; the decision
  is a Rust scan.
- **Caller count (major):** ~30 test sites; keep `eval_time_func` pure + add
  `eval_coeff`.
- **Tests (blocker):** per-class scoping for the invariant; gradient gate is
  FD-of-value until §4 (analytic-vs-FD would go red on the value-only fix).
- **Guard relocated (major):** Rust post-resolve on `estimate_set`, not OCaml.
- **Blast radius (major):** four goldens enumerated; advisory warranted.
- **Rename (major):** deferred — live collision with `worktree-param-values`.
- **Perf (major):** cost is ×N-referencing-transitions; add a per-(substep,t)
  memo.
