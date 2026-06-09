# A parameter used inside a forcing/table coefficient is frozen at construction

Date: 2026-06-09 Severity: high (silent wrong inference; no error, no warning)
Backends affected: **all** (the freeze is in the shared `CompiledModel`, not a
backend) — bites **inference**, where the model is built once and parameters
vary; a single forward `simulate` run is unaffected because it rebuilds the
model each invocation. Status: open — fix proposed (see Remediation), not yet
landed. Discrepancy class (per CLAUDE.md): **code-vs-code** — two internal
evaluation paths disagree about the same `Expr::Param`: rate eval reads it live,
forcing / table-coefficient eval freezes it. Fix the code; add a test pinning
agreement.

## What happened

When an **estimated** parameter is used inside a forcing coefficient (or an
inline-table value expression), its proposed values during a fit have **zero
effect on the dynamics**. The likelihood is flat in that parameter; the sampler
explores its prior, the dynamics never respond, and the posterior is garbage.
Nothing errors or warns.

### Reproduction (concrete input → wrong output)

The golden `ocaml/golden/seir_seasonal_patch.ir.json` has an estimated parameter
`amp` driving a sinusoidal forcing:

```
amp[patch]: positive in [0.0, 1.0]          # estimated (bounds, no value)
forcing { seasonal[p in patch] : sinusoidal 'per_day { amplitude = amp[p] ... } }
transitions { infection[p] : S[p] --> E[p] @ seasonal[p] * S[p] * I[p] / N[p] }
```

Build the model **once**, then vary a parameter **only in the live params
slice** — exactly what the inference inner loop does — with a fixed seed:

```rust
let compiled = CompiledModel::new(model).unwrap();   // amp_urban seeded at 0.4
let cfg = /* chain-binomial, t_end=60, dt=1 */;
let p_lo = compiled.default_params.clone();          // amp_urban = 0.4
let mut p_hi = compiled.default_params.clone();
p_hi[compiled.param_index["amp_urban"]] = 0.9;       // 2.25x larger amplitude
let a = ChainBinomialSim.run(&compiled, &p_lo, 7, &cfg).unwrap();
let b = ChainBinomialSim.run(&compiled, &p_hi, 7, &cfg).unwrap();
```

Final compartment counts
`[S_urban, S_rural, E_urban, E_rural, I_urban, I_rural,
R_urban, R_rural]`:

| run (one built model, varied live slice) | final counts                                                         |
| ---------------------------------------- | -------------------------------------------------------------------- |
| `amp_urban = 0.4`                        | `[40974, 50000, 23947, 0, 21302, 0, 13777, 0]`                       |
| `amp_urban = 0.9`                        | `[40974, 50000, 23947, 0, 21302, 0, 13777, 0]` **← byte-identical**  |
| `sigma = 0.6` (control)                  | `[1560, 50000, 1455, 0, 32269, 0, 64716, 0]` **← totally different** |

A 2.25× larger seasonal amplitude produced an **identical epidemic**. `amp` is
used inside the forcing → frozen. `sigma` is used directly in a rate
(`sigma * E`) → live → changes everything. This is the bug in one table:
`amp_lo == amp_hi` (the freeze) and `amp != sigma` (the control proving the test
discriminates).

## How it was detected

While auditing the const‖parametric-forcing issue cluster
(gh#119/#186/#128/#180) and the `default_params` naming, the maintainer pushed
for a reproduction rather than a code-trace. I compiled `seir_seasonal_patch`
and inspected the IR:

```
$ camdlc ocaml/golden/seir_seasonal_patch.camdl | jq '.model.time_functions[].kind'
{"sinusoidal": {"amplitude": {"param": "amp_urban"}, "period": {"const": 365.0}, ...}}
```

The coefficient survives compilation as a **live `Param` reference**, not a
folded `Const` (contrast `period: {"const": 365.0}`). The reproduction above
then showed the runtime ignores it.

## Root cause

The model is built **once** per fit and the parameter vector varies as a
separate borrowed slice — the PGAS / chain-binomial density and step functions
take `model: &CompiledModel, params: &[f64]`, with the comment _"to avoid
rebuilding each call."_ Two evaluation paths then treat the same `Expr::Param`
differently:

- **Rates are live.** A rate is stored as a pre-resolved tree and evaluated via
  `eval_resolved`, whose `ResolvedExpr::Param(idx) => ctx.params[idx]`
  (`rust/crates/sim/src/resolved_expr.rs:408`) reads the **live** slice. So a
  proposed `sigma` reaches the dynamics.
- **Forcing / table coefficients are frozen.** At construction,
  `CompiledModel::new` collapses each `Sinusoidal` field to a single `f64`:
  `amplitude: eval_table_expr(&s.amplitude, &param_index, &default_params)?`
  (`rust/crates/sim/src/compiled_model.rs:759`; the table cache is the same at
  `:731`). `eval_table_expr`'s `Expr::Param(p) => Ok(params[idx])`
  (`compiled_model.rs:277-281`) silently resolves the param against
  `default_params` — **no error** — and the result is cached in
  `time_func_cache` / `table_values_cache`.
- **The read path can't recover.** When a rate references the forcing, eval
  dispatches to `eval_time_func(&ctx.model.time_func_cache[idx].kind, ctx.t)`
  (`rust/crates/sim/src/propensity.rs:186`). `eval_time_func(kind, t)`
  (`propensity.rs:340`) takes **no `params` argument** — structurally, a forcing
  _cannot_ read the live slice. The cache baked at construction is all it ever
  sees.

So: `default_params[amp]` is baked into `time_func_cache` at build time; the
live slice's `amp` is read by nothing. Same `Expr::Param`, two paths — one live,
one dead.

### What `default_params` actually is (the "f64 from nothing" question)

It is not from nothing. `pub default_params: Vec<f64>`
(`compiled_model.rs:372`), doc-commented _"Default parameter values extracted
from model.parameters, in param_index order"_ — **the construction-time
parameter vector.** Every parameter must resolve to a concrete number to build
the model at all (you cannot bake a rate/forcing cache from a symbol). The build
loop (`compiled_model.rs:611-620`):

```rust
let v = p.value.resolved_value().ok_or_else(|| SimError::Validation(
    format!("parameter '{}' has no value; supply it via --params or --param", p.name)
))?;
default_params.push(v);
```

`resolved_value()` (`rust/crates/ir/src/parameter.rs:213-219`) is where the f64
comes from, per kind:

- `Fixed { value }` → the constant.
- `Estimated { init, .. }` → `init` — **the inference starting point**. For
  `seir_seasonal_patch`, `amp` has no DSL init, so the fit layer seeds one
  before construction (`rust/crates/cli/src/fit/runner.rs:164-180`:
  `if p.value.resolved_value().is_none() { p.value = p.value.with_value(v); }`),
  and the chain starts from `base_params = compiled.default_params.clone()`
  (`runner.rs:213`), perturbing the slice each iteration.
- `Required` → `None` → the "has no value" error fires (this is the only path
  that errors).

So the frozen value is **whatever the fit seeded `amp` at** — there is no
intrinsic constant. (An earlier hand-wave of "frozen 0.2" in chat was an
illustration wrongly stated as measured; corrected here.)

### Is `default_params.push(v)` itself the bug? No.

The maintainer asked whether the snippet above _"should be an error."_ It should
not. `default_params` legitimately needs a concrete value per parameter to
construct the model; `ok_or_else` errors only when a value is genuinely absent
(`Required` / unresolved), which is correct. The construction of
`default_params` is sound. **The bug is 100% downstream:** calling
`eval_table_expr(Param, default_params)` for a _param-referencing_ coefficient
at all. The construction value is correct as a _starting point_ (rates treat it
that way); the forcing/table cache is wrong to treat it as _permanent_.

### Why types didn't prevent it; what it was before last night's ADT

The maintainer asked whether the quoted `p.value.ok_or_else(...)` is "the new
code from last night." **No — that is the _pre-ADT_ form.** Last night's commit
`refactor(ir): ParamValue ADT — Fixed | Estimated | Required (gh#191)`
(2026-06-08) renamed the field access `p.value` → `p.value.resolved_value()`;
the older `value: Option<f64>` form `let v = p.value.ok_or_else(...)` is what
shipped before it (e.g. on `fix(runid): close lock-reclaim TOCTOU…`, 2026-06-08,
which predates the ADT — confirmed by `git merge-base --is-ancestor`). **Both
forms have the freeze**; the ADT neither introduced nor fixed it. The freeze
long predates the ADT — `eval_table_expr` and the cache build are present in the
pre-ADT tree.

Types did not catch it because the type system never modeled the distinction
that matters: an `Expr::Param` is the _same type_ whether it sits in a rate
(must stay live) or in a forcing coefficient (was collapsed to `f64`). The
escape from `f64`-land to a live tree (`ResolvedExpr`) was taken for rates and
**not** for forcing/table coefficients. The freeze is encoded in a function
signature — `eval_time_func(kind, t)` has no `params` — so as long as that
signature stands, no test of behavior downstream of it can see a live param,
because there isn't one to see.

## Detection: would per-parameter perturbation goldens have caught it?

**Yes — but only the right kind, and they are the weakest of three options.**

1. **A per-parameter perturbation golden would catch it _iff_ it perturbs the
   live slice without rebuilding** the model (i.e. mimics inference: build once,
   vary `params`). The reproduction above is exactly that. A naive version that
   _recompiles per parameter value_ would **mask** the bug — recompiling
   re-bakes the cache, so the change reappears. This subtlety is why a
   hand-authored golden is fragile.

2. **Better: a general "no dead estimated parameter" invariant test**, run
   across _all_ fittable goldens automatically rather than hand-written per
   model. Build each model once; for every estimated parameter, perturb only the
   live slice and assert the propensity vector (or the loglik) changes at some
   `t`. This catches the bug _class_ for every model, present and future —
   whereas a per-model golden only protects the one model someone remembered to
   write.

3. **Sharpest signal: a gradient-sensitivity gate.** The compiler emits
   `rate_grad`; a frozen parameter has _exactly_ zero gradient through the
   forcing — a noise-free check needing no tolerance. Assert `∂(loglik)/∂θ ≠ 0`
   for every estimated `θ` on a representative dataset, reusing the existing FD
   infrastructure (`gradient_check.rs`). This is also the natural home given the
   PGAS value/grad work (RC1).

   Note that **backend-agreement testing does not help here** — unlike the
   2026-05-20 frozen-propensity incident (below), all backends share the one
   frozen `CompiledModel` cache, so they agree _wrongly_. The detector has to be
   a parameter-sensitivity / identifiability invariant, not cross-backend
   consistency.

**Best of all: make the bug unrepresentable.** Options 1–3 detect; the fix
_prevents_. If forcing/table coefficients that reference a parameter stayed live
`ResolvedExpr` (like rates), and `eval_time_func` took the params slice, the
dead-parameter state could not be constructed. A bug that cannot be expressed
needs no test. (The existing `gate_constant_fold_ab.rs` does _not_ cover this —
it proves the OCaml `constant_fold.ml` pass is trajectory-preserving; the freeze
is a Rust runtime bake at a different layer.)

## Remediation (proposed — not yet landed)

Designed in `docs/dev/proposals/` (const‖parametric forcing; in draft). The
shape:

- **Fold only constants / `ParamValue::Fixed`.** Key the build-time collapse on
  the kind: a coefficient that is `Const` (or references only `Fixed` params)
  may bake to `f64`; a coefficient referencing an `Estimated`/`Required`
  parameter must stay a live `ResolvedExpr`, evaluated each step against
  `ctx.params` — identical to how rates already work. The `ParamValue` ADT (last
  night's work) is the enabler: the fold decision can now key on `Fixed` vs
  `Estimated`/`Required` instead of an opaque `Option<f64>`.
- **`eval_time_func` (and the table path) take the params slice** so a live
  coefficient can read it — closing the structural hole.
- **Add the identifiability invariant test (option 2)** and a gradient
  sensitivity gate (option 3); the reproduction above is the red test (today
  `assert_eq!(amp_lo, amp_hi)` passes — the freeze; the fix flips it to
  `assert_ne!`).

Until the fix lands, **estimating a parameter that appears only inside a forcing
or inline-table coefficient silently does not work.** A compile-time diagnostic
rejecting that construct (E-code with hint: "parameter `amp` is used in a
forcing coefficient, which is currently evaluated once at load; estimating it
has no effect — use it in a rate, or mark it fixed") would be a cheap interim
guard if the full fix is deferred.

## What it suggests

- **Cousin incident, opposite detector.**
  `docs/dev/incidents/2026-05-20-gillespie-bare-time-frozen-propensity.md` is
  the same failure _shape_ (a propensity frozen at its construction-time value,
  no error, plausible-but-wrong) but a different root (Gillespie omitting
  bare-`t` rates from re-evaluation) — and there backends _disagreed_, so
  backend agreement caught it. The recurring meta-pattern across both: a value
  that should be live is computed once and cached, and the type system does not
  distinguish "live" from "constant," so the freeze is invisible.
- **The smell is a type that erases a semantic distinction.** `f64` cannot say
  "this number is provisional (an inference start) vs final (a fixed constant)."
  The rate path escaped to `ResolvedExpr`; the forcing/table path did not. Where
  one sibling path is live and the other is frozen for the _same_ AST node,
  suspect the frozen one.
- **A predicate/seam named for one case hides the others.** As in 2026-05-20
  (`expr_has_time_func` missing `Expr::Time`), the forcing cache was built for
  the constant-coefficient case the author had in mind; the
  parameter-coefficient case was silently swept into the same `f64` collapse.
