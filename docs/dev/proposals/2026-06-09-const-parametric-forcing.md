# Const‖Parametric forcing: a coefficient is an expression, not data (value + gradient)

- **Status:** Draft **v3** — design reworked around "coefficients are
  `ResolvedExpr`" (Design B) after a 3-reviewer design panel (per-forcing
  mechanics, performance, engineering) **unanimously** chose it over the v2
  `Const|Live` enum, including the reviewer assigned to steelman v2. A second
  3-reviewer pass then checked it lands clean against the code (no blockers; the
  gradient half is easier than first assumed — corrections folded in). Changelog
  at end. This proposal is the **implementation plan** for examples **§7
  (gradient half)** and **§8 (value half)** of
  [`docs/dev/notes/2026-06-08-static-typing-as-bug-prevention.md`](../notes/2026-06-08-static-typing-as-bug-prevention.md);
  read those first.
- **Fixes:** the freeze in
  [`docs/dev/incidents/2026-06-09-forcing-coefficient-param-frozen-at-construction.md`](../incidents/2026-06-09-forcing-coefficient-param-frozen-at-construction.md).
  **gh#119** (engine value freeze) and **gh#186** (the comprehensive
  inference-framed issue: all methods, both halves). gh#128 is **closed** and
  orthogonal (it is _why_ the gradient gap is silent — there is no `rate_grad`
  key to reject); gh#180 is a **separate** obs-projection gradient bug, out of
  scope.
- **Required reading:** the incident; the static-typing note §7+§8 and "The
  layering rule"; `docs/camdl-language-spec.md` §7 (line ~1080 advertises the
  broken feature); the
  [typed-parameter-surface proposal](2026-06-08-typed-parameter-surface.md).
- **Discrepancy class:** **code-vs-code** (rate/obs eval live, forcing eval
  frozen — same `Expr::Param`) + **doc-vs-code** (spec promises the feature;
  sync code to spec).

## 0. One root cause, two symptoms

The bug is a single mental-model error — **a forcing's scalar coefficient is
treated as _data_ (a fixed covariate) when it is an _expression over
parameters_.** That one premise produced both symptoms:

- **Value freeze** — the coefficient is baked to `f64` at `CompiledModel::new`
  (`compiled_model.rs:758-840`) and never updated as the params slice varies.
- **Zero gradient** — `autodiff.ml:23-24` differentiates
  `TimeFunc`/`TableLookup` to `Const 0.0` ("covariates are data, derivative =
  0").

The fix is to stop treating coefficients as data and **treat them as what the IR
already says they are: expressions.** Evaluate them live (value half, §3);
differentiate the closed form with respect to them (gradient half, §4). The two
halves are the same reframe applied in two places (Rust runtime eval; OCaml
autodiff). The genuinely _structural_ parts of a forcing — interpolation knots
from a CSV, a precomputed cubic-spline basis — are real data and stay
precomputed; only the scalar coefficient _inputs_ change.

## 1. What is broken (with the reproduction and the real blast radius)

The spec promises a forcing coefficient may reference a parameter "for
inference" (`docs/camdl-language-spec.md:1080`, `amplitude = alpha`). The
runtime silently breaks it: the model is built once per fit, parameters vary as
a borrowed slice, and the baked coefficient never updates. Reproduction (build
once, vary `amp` only in the live slice → byte-identical trajectory; vary
`sigma`, used in a rate → totally different) is in the incident.

**Blast radius — four committed goldens** (verified by `jq`; all are
`Sinusoidal`; no inline table references a param):

| golden                    | forcing param(s)               | IR `ParamValue` |
| ------------------------- | ------------------------------ | --------------- |
| `seir_seasonal_patch`     | amp_urban, amp_rural, baseline | **Estimated**   |
| `seir_vaccine_seasonal`   | alpha, phi_season (spec ex.)   | **Required**    |
| `seir_pop_balance`        | pop_amp, pop_mean              | **Required**    |
| `phenom_mixing_unchecked` | amp                            | **Required**    |

Three of four are `Required`. Any past fit of these has a posterior reflecting
only the prior for the forced parameter — a **user-facing advisory** is
warranted (§7).

## 2. Why it happens, and where it does NOT

### 2.1 Rate/obs eval is live; forcing eval is frozen; everything else is live

- **Rate (live):** `ResolvedExpr::Param(idx) => ctx.params[idx]`
  (`resolved_expr.rs:408`).
- **Observation models (live):** `ResolvedLikelihood` stores its coefficients
  (`mean`, `dispersion`, `sd`, …) as `ResolvedExpr` and evaluates them live
  every call (`obs_model.rs:60-136`, `resolve_likelihood` in
  `resolved_expr.rs`). **This is Design B already in production** — forcings are
  the one place that diverged.
- **Forcing/table coefficient (frozen):**
  `eval_table_expr(expr, &param_index,
  &default_params)` (16 sites) → `f64`;
  `eval_time_func(kind, t)` (`propensity.rs:340`) takes no params, so the read
  path cannot recover.
- **Verified NOT siblings** (review): initial conditions
  (`compiled_model.rs:1154-1169`), `balance`, `events`/`interventions`
  (`effects.rs`), ODE equations, parametric schedules — all live. The freeze is
  confined to `time_func_cache` + `table_values_cache`.

### 2.2 The value half alone is not enough for NUTS

Fixing the value freeze makes the likelihood respond → **IF2 and the bootstrap
particle filter (gradient-free) work.** But **PGAS+NUTS** drives the θ|X step
with compiler-emitted `rate_grad`, and `autodiff.ml` zeroes `TimeFunc`, so NUTS
sees ∂loglik/∂coef ≡ 0. Value-only would ship a _responsive likelihood with a
zero gradient_ — they disagree, a worse failure than the uniform freeze.
`gradient_check.rs:448-454` already documents this "doubly-silent zero
gradient." So §4 is required before forcing params are advertised as
NUTS-estimable.

### 2.3 Forcing coefficients are NOT constant-folded by the OCaml pass

`fold_model` (`constant_fold.ml:103-118`) folds only transitions, bindings, and
ODE equations — never `time_functions`/`tables`. So a coefficient `2 * 0.15`
reaches Rust as a `BinOp`. The Rust build must therefore resolve coefficient
expressions itself (it already has the machinery: `resolve_expr`).

## 3. Value half: a coefficient is a `ResolvedExpr`, evaluated live

Store each forcing's **scalar coefficients as `ResolvedExpr`** — exactly how
rates and observation likelihoods already store theirs. A literal coefficient is
a `ResolvedExpr::Const(v)`; a parameter coefficient is
`ResolvedExpr::Param(idx)` (or any expression). There is **no `Const|Live` enum
and no build-time classifier** — the v2 design reintroduced the
`expr_refs_param` scan that the note's "layering rule" explicitly names as _the
inverse mistake_, and that classifier is the exact surface v1's
`Estimated`-keying got wrong. With Design B there is **no `f64` slot for a
coefficient to freeze into**: the freeze is unrepresentable, not merely guarded.
(Panel: B is uniform with rates/obs across 6 of 8 forcing kinds; the v2 `Const`
variant optimizes a negligible term — see §perf below.)

```rust
// CompiledTimeFuncKind — scalar coefficients become ResolvedExpr; structural
// data (precomputed) is unchanged.
Sinusoidal { amplitude: ResolvedExpr, period: ResolvedExpr,
             phase: ResolvedExpr, baseline: ResolvedExpr }
Periodic   { period: ResolvedExpr, values: Vec<ResolvedExpr> }
Fourier    { period: ResolvedExpr, harmonics: Vec<(ResolvedExpr, ResolvedExpr)> }
// Interpolated / CubicSpline / PeriodicSpline: structural arrays + the
// precomputed spline basis stay as today (see "structural" below).
```

**Build (`CompiledModel::new`):** replace each
`eval_table_expr(expr, .., &default_params)?` with `resolve_expr(expr)` — but
**keep the current grammar whitelist**. `eval_table_expr` hard-errors on
anything but `Const/Param/BinOp/UnOp/UncheckedDim`
(`compiled_model.rs:328-330`); `resolve_expr` would silently admit
`Pop/Time/TimeFunc/TableLookup/BindingRef`, i.e. state/time/forcing-dependent
coefficients the spec never defined. Add an explicit validation pass over the
coefficient expression (reject those variants with a clear E-code) **before**
resolving — preserving today's hard error. **Constant-fold** the resolved
coefficient at build so an all-constant coefficient is a single
`ResolvedExpr::Const(f64)` (a bare leaf, no tree walk) — this recovers v2's fast
path for the common case without a separate type.

**Readiness snags to budget (none are blockers):** (a) **build-loop ordering** —
`ResolveCtx` is currently constructed _after_ the table/time-func build loops
(`compiled_model.rs:~892` vs the loops at `~724-844`), and it depends on
`table_meta`, which depends on `table_values_cache`. So this is not an in-place
`eval_table_expr → resolve_expr` swap: hoist a coefficient-only resolve context
(it needs `param_index`, available at `:609`, plus the indices used _only to
reject_ via the whitelist) above the loops, or move the loops below
`ResolveCtx`. This reorder inside a 500-line, CLAUDE.md-flagged constructor is
the value half's top risk. (b) **`fold_resolved` and the forcing memo are new
code** (no `ResolvedExpr` fold exists; the binding `CacheScope` is
binding-keyed, not a drop-in) — but both are _perf_, not correctness, so a
correct-but-unoptimized value half can land first and add them after. (c)
**`schema.json` already drifted** — `ir/schema.json:328-331` declares
`amplitude`/`period`/… as `"number"` while the IR emits exprs (e.g.
`seir_vaccine_seasonal.ir.json`: `"amplitude": {"param":"alpha"}`); it is
unenforced (no test validates goldens against it), but fold the
`number → $ref expr` correction in here (no `ir/VERSION` bump — serialized
content is unchanged). (d) Preserve the existing build-time numeric checks
(`period <= 0`, `coefs.len()==n_basis`) by also evaluating those at
`default_params` for the check — don't let them silently regress.

**Structural data stays precomputed (the real split — kind-by-kind, not
field-by-field-classified):** the cubic-spline basis (`CubicSpline::new`'s
construction-time Thomas solve, `compiled_model.rs:46-98`), `PeriodicSpline`
coefs consumed by the de Boor evaluator, and CSV/inline interpolation knot
arrays are **data**, not coefficients, and remain precomputed `f64`/derived
structures. **Correction (readiness review):** `Fourier`'s `period_inv` is _not_
structural — it is `1/period`, a derived **coefficient**. If `period` becomes a
`ResolvedExpr`, the cached `period_inv` (`compiled_model.rs:811`) cannot stay
precomputed; compute `1/eval(period)` live in `eval_forcing` (no golden uses
`Fourier`, so this is unexercised today, but the machinery must handle it).

**Reject estimated spline knots.** A param-referencing `Interpolated`-spline
knot or `PeriodicSpline` coef cannot be live (its value feeds a basis derived by
a construction-time solve; live would mean rebuilding the spline every substep).
Emit a clear E-code. This is a _structural_ property of those kinds, decidable
without any Const/Live classifier.

**Eval call structure (preserve the pure math).** Keep `eval_time_func`'s
closed-form math a pure function of **already-evaluated scalars + `t`** (so the
Fourier/spline/interp math stays testable in isolation, as the ~30 pure-math
tests in `interpolation.rs`/`periodic_forcing.rs`/`fourier_oracle.rs` need). Add
a thin `eval_forcing(&kind, t, ctx)` at the two production read sites
(`propensity.rs:186`, `resolved_expr.rs:510`, both already hold `ctx`) that
evaluates the `ResolvedExpr` coefficients against `ctx.params` and then calls
the pure math. The ~30 tests migrate to calling the pure per-kind math directly
(testing the math, not resolution) — a one-time, clarity-improving refactor.

**Perf — memo, not a `Const` type.** The dominant hot-path cost is _not_ the
coefficient leaf (a `Const` eval is ~1–3 cycles vs the forcing's own `sin()` at
15–40); it is that a forcing referenced by **N transitions** is evaluated N
times per substep per particle (`propensity.rs:461-475`) — a cost **identical
under v2 and B** and _not_ removed by a baked `f64`. Add a per-`(forcing, t)`
memo using the existing binding `CacheScope` machinery
(`resolved_expr.rs:360-397`) so the N referencing transitions share one
evaluation (collapsing N `sin()` to 1 — a win v2 also needed and did not get
from its `Const` type). `t` is constant within one `eval_propensities` call, so
the key is just the forcing index. The asymmetry that settles the design: B's
worst case (a constant misjudged as live) is _slower_; v2's worst case (a
misclassification) is _silently frozen_ — for a public-health inference tool,
"never silently wrong" wins.

## 4. Gradient half: differentiate the closed form w.r.t. coefficient params

Same root cause: once a coefficient is an expression, its derivative is
well-defined. Two pieces:

1. **The floor — make a dropped derivative explicit** (note §7):
   `type deriv = Known of expr | Unsupported of { node; reason }`.
   `differentiate_rate` must _choose_ — `Known (Const 0.0)` (genuinely absent) →
   omit; `Known d` → emit; `Unsupported` → a compile-time E-code naming the
   param + forcing. A silent zero becomes unrepresentable.
2. **The feature — emit ∂forcing/∂coef per kind:** `Sinusoidal` ∂/∂amplitude =
   `sin(2π(t−phase)/period)` (with nonzero ∂/∂phase, ∂/∂period); `Fourier`/
   `PeriodicSpline` linear in their coefs; `Piecewise`/`Periodic` differentiable
   w.r.t. step _values_, `Unsupported` w.r.t. a _param-valued breakpoint_ (the
   value is discontinuous in it); spline _knots_ → `Unsupported` (dense
   dependence through the solve — consistent with §3's rejection).

**Readiness finding — this is the _easy_ world (no IR schema change):** the expr
language **already has `Sin`/`Cos`/`Tanh`** (`ir.ml:9`, gh#58) with
differentiation rules already present (`autodiff.ml:165-173`), so
`sin(2π(t−phase)/period)` is expressible as a plain `Expr` — no new node, no
`ir/VERSION` bump, no all-golden re-bless. And the **production gradient path is
emitted `rate_grad` → `resolve_expr` → `eval_resolved`** (`pgas_grad.rs:128`),
_not_ the forward-mode `eval_expr_deriv` — so the emitted derivative is an
ordinary `Expr` and there is **no `ResolvedExpr::TimeFunc` identity-threading to
do** (v3 over-scoped this; `eval_expr_deriv`'s `TimeFunc→0` at
`propensity.rs:255` is a secondary path, fix-or-document separately). The real
work is: thread `model.time_functions` into `differentiate` (the data is already
in scope at the call site, `compiler.ml:354`; ~31 recursive arms re-signed), the
per-kind derivatives, and the `deriv` ADT (touches ~72 `differentiate` arms —
uniform, generalizes the existing `Mod` `failwith`). **It changes the IR
`rate_grad` content** (forcing params gain entries; today null) → an **atomic
golden re-bless** of the 4 affected `ocaml/golden/*.ir.json` via
`make update-golden` (forward-sim TSVs are unaffected — `rate_grad` is
gradient-only — so no `update-expected` churn). **Depends on §3 landing first:**
an emitted ∂/∂amplitude that references `phase`/`period` is only correct once
those coefficients are evaluated live (§3); against the baked `f64` it would use
stale default-param values.

**Scope decision (central):** ship the value half (§3) **plus a NUTS guard**
first; the gradient half second. The guard (Rust, post-resolve — §6) rejects a
fit when an estimated parameter is referenced _only_ inside a forcing/table
coefficient and `use_nuts` is on, naming the limitation — so IF2/PF work
immediately and NUTS fails loud rather than silently mis-sampling.

## 5. Tests

- **Red test (the repro):** build once, vary the param in the live slice; today
  `assert_eq!(lo, hi)` (frozen) → fix flips to `assert_ne!`. Cover a
  **`Required` case (`seir_vaccine_seasonal`)**, a `Periodic` value, and an
  inline table — not only the `Estimated` `seir_seasonal_patch`.
- **"No dead estimated parameter" invariant — scoped per class** (a single
  "propensity changes" assertion false-positives on init-only / obs-only /
  overdispersion / intervention params): a param entering a **rate or forcing
  coefficient** must move the propensity; init → `initial_state` changes; obs →
  obs-likelihood changes; overdispersion → draw variance; intervention →
  post-fire state. Forcing params enter the rate via the `TimeFunc` node, so the
  scoped propensity check catches the target bug.
- **Gradient gate — two stages:** until §4, **FD-of-value only**
  (`loglik(θ±ε)`), never analytic-vs-FD (the analytic gradient is structurally
  zero and would go red against the value-only fix). After §4, un-`xfail` the
  analytic-vs-FD seasonal check at `gradient_check.rs:448`.
- **Perf guard:** assert the per-`(forcing, t)` memo collapses N referencing
  transitions to **one** forcing evaluation per substep (the real win), and that
  an all-constant coefficient folds to a single `Const` leaf.

## 6. Interim guard — in Rust, post-resolve (not OCaml)

OCaml cannot distinguish "estimated in this fit" (a `fit.toml` `[estimate]`
fact) from `Fixed`/`Required` — so an OCaml guard both false-positives (a
`--fixed` run) and false-negatives (a bare `Required` param under `[estimate]`).
Put the guard in the **Rust fit path after `resolve_parameters`**
(`params_resolver.rs`), where `estimate_set` is known: error iff a param in
`estimate_set` is referenced _only_ inside a forcing/table coefficient. Same
home as the §4 NUTS guard.

**Readiness note:** the `estimate_set` membership test is cheap (it exists,
`params_resolver.rs:177`), but the discriminating predicate is **new code**. The
existing param-ref walks (`expr_refs_param` `compiled_model.rs:236`,
`collect_param_refs` `pgas.rs:34`) treat `Expr::TimeFunc` as **opaque** — and
the coefficient sub-exprs (`amplitude = alpha`) are **not in the rate AST at
all**; they live in `model.time_functions[*].kind`. So the guard needs a new
traversal over every forcing-coefficient and inline-table-value expr
(`coeff_refs`) versus all rate/obs/init exprs (`body_refs`); fire when
`θ ∈ estimate_set ∧ θ ∈
coeff_refs ∧ θ ∉ body_refs`. ~40–60 lines + tests;
budget it as new code, not a reuse of the existing walks.

## 7. Blast radius & advisory

The four goldens in §1 are each silently broken for their forcing parameter
today. A **user-facing advisory** (incident + release notes) is warranted: any
completed fit of a model estimating a parameter that appears only inside a
forcing/table coefficient has a posterior reflecting only the prior for that
parameter. The Rust guard (§6) makes this loud going forward.

## 8. Sequencing, ownership, and lift

- **Value half (§3): medium lift, ~self-contained in `sim`.**
  `compiled_model.rs` (the `CompiledTimeFuncKind` field types + the ~16-site
  build loop + the whitelist validation + spline-knot rejection),
  `propensity.rs`/`resolved_expr.rs` (extract pure per-kind math, add
  `eval_forcing` + the memo), the ~30 pure-math test refactors, the Rust
  post-resolve guard, and the new tests. **No IR/schema change and no golden
  re-bless** — a forward `simulate` rebuilds the model per invocation, so its
  output is unchanged; only inference (build-once, vary-slice) changes behavior,
  covered by new tests. **No `inference/*` edits.** Independent of RC1/#191.
- **Gradient half (§4): larger lift, crosses the language boundary — but the
  _easy_ world.** `autodiff.ml` (per-kind derivatives + the `deriv` ADT across
  ~72 arms; thread `model.time_functions` in — data already in scope at
  `compiler.ml:354`), **and an atomic golden re-bless** of the 4 affected
  `ocaml/golden/*.ir.json` (forcing params gain `rate_grad` entries; via
  `make update-golden`). `sin`/`cos` already exist (`ir.ml:9`) so **no schema
  change / no `ir/VERSION` bump / no all-golden re-bless**, and the emitted
  derivative rides the existing emitted-`rate_grad` → `eval_resolved` path (no
  `ResolvedExpr::TimeFunc` threading). Forward-sim TSVs are unaffected
  (`rate_grad` is gradient-only). Decide whether to also fix `eval_expr_deriv`'s
  `TimeFunc→0` (secondary path) or document it value-only.
- **`default_params` rename: re-baseline before deferring.** The readiness
  review could not reproduce the `worktree-param-values` collision in this clone
  (that branch is 0 commits ahead of `main` — merged or absent). Two
  consequences: (i) confirm against _current_ `main` whether
  `default_params`/`effects.rs` were already touched — if the param-value work
  merged, the DEFER is **stale** and the rename can ride with the value half;
  (ii) note that the value half rewrites the **same** `compiled_model.rs` build
  loop the rename targets, so "rename collides, value half doesn't" is not a
  real separation — they edit the same lines. Resolve the branch-state question
  (maintainer knows where the other agent's work lives) before sequencing on a
  collision.

**Rough estimate (sharpened by the 3 readiness reviewers, in engineer-days):**

- **Value half ≈ 2–4 d.** ~2 if the `fold_resolved` constant-fold and the
  `(forcing,t)` memo are deferred (both perf, not correctness); ~3–4 for full §3
  including the build-loop reorder (the real risk), the new whitelist + guard
  walk, and the test migration. No schema/golden churn — the gating claim, and
  it holds.
- **Gradient half ≈ 3–4 d** (lower end of "several days" — `sin`/`cos` already
  paid for by gh#58, so it's the easy world): the per-kind derivatives + the
  `deriv` ADT + threading `time_functions` + the atomic 4-golden re-bless + FD
  validation.
- **Total ≈ 1–1.5 engineer-weeks, staged.** Value-first delivers the IF2/PF fix
  and the loud NUTS guard quickly (no goldens move); the gradient half then
  makes NUTS-estimation work and is the only part that re-blesses goldens.

## Open questions for the maintainer

1. **Scope: v1 (value + NUTS guard) then v2 (gradient), or both at once?**
   Recommend staged.
2. **Advisory wording & placement** for the four affected goldens (§7).
3. **Estimable Fourier/PeriodicSpline _coefs_** (linear, hence feasible) in the
   first gradient pass, or defer with spline _knots_? (Coefs are differentiable;
   knots are not.)

## Changelog

**v3 readiness pass (3 implementation-reviewers vs the code — no blockers):**
gradient half is the _easy_ world — `sin`/`cos` already in the IR (gh#58), so no
schema change, and the emitted-`rate_grad` path means no `TimeFunc`
identity-threading (v3's §4 over-scoped both — corrected). `Fourier::period_inv`
is a derived coefficient, not structural (corrected). The value-half top risk is
the `CompiledModel::new` build-loop reorder (`ResolveCtx` built after the
coefficient loops); the whitelist, `fold_resolved`, the memo, and the §6 guard
walk are all new code (fold/memo perf-deferrable). The `worktree-param-values`
collision could not be reproduced (branch 0-ahead of `main`) — re-baseline
before deferring the rename. `schema.json` already drifted (`amplitude:"number"`
vs emitted expr) — fold the correction in. Sharpened lift: value ≈ 2–4 d,
gradient ≈ 3–4 d, ≈ 1–1.5 eng-weeks staged.

**v2 → v3 (design panel, 3 reviewers, unanimous for Design B):**

- **Dropped the `CompiledCoeff = Const|Live` enum and its build-time
  classifier.** Coefficients are `ResolvedExpr`, evaluated live — uniform with
  rates and the obs path (which already does exactly this). Removes the
  classifier that is the v1 bug's mechanism; the freeze becomes unrepresentable.
- **Reframed §3/§4 as one root cause** (coefficient-as-data vs
  coefficient-as-expression) — §0.
- **Perf:** the win is a per-`(forcing,t)` memo (collapses N→1), not a baked
  `f64`; constant-fold gives the common-case fast path for free.
- **Kept v2's correct guardrails:** structural data precomputed, reject
  estimated spline knots, preserve the coefficient-grammar whitelist, pure
  `eval_time_func` math + caller-materialized coefficients (preserving the ~30
  tests).
- **Issue mapping corrected:** headline #119+#186; #128 closed/orthogonal; #180
  out of scope.

**v1 → v2 (adversarial review):** keying corrected from `ParamValue::Estimated`
to a structural decision (Required→Fixed blocker); gradient half promoted to
core; splines can't be `Live`; whitelist; §2.3 fold premise corrected; ~30 test
callers; per-class test scoping; guard relocated to Rust; four-golden blast
radius; rename deferred; perf is ×N-transitions.
