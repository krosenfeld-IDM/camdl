# Proposal: Type the IR parameter & intervention surface

- **Status:** Implemented (IR 0.10 → 0.11). As-built deviations, all verified
  against code:
  - **`ParamKind` has 7 variants, not 5** — `instant`/`duration` are live
    (`parser.mly`, `Dimcheck.param_dim_of_kind`, calendar-date rendering), so a
    5-variant enum would fail to deserialise real models.
  - **`param_kind` stays `Option<_>`** — 21 committed goldens carry
    `param_kind: null`; an un-annotated parameter gets a fresh dimension var.
  - **Field names `param_kind`/`param_dim` kept** (not renamed to `kind`/`dim`):
    cosmetic-only, and the enum serialises to the same `"rate"` strings, so
    keeping them means zero golden-value churn.
  - **Mapping (the resolved open question):** the `parameters {}` productions
    never carry `= value`; a fixed value comes only from a typed-const `let`. So
    typed-const `let` → `Fixed`; a `parameters {}` declaration with bounds/prior
    → `Estimated`; a bare declaration → `Required`.
  - **`ParamValue::resolved_value()`** (Fixed→value, Estimated→`init`,
    Required→None) is the faithful drop-in for the former `value: Option<f64>`;
    **`with_value()`** sets a supplied value while KEEPING an `Estimated`
    parameter's bounds (so a supplied value is still bounds-checked — a bare
    `Fixed` would silently accept out-of-range input).
  - **gh#191 gate:** the capability-scan placeholder loop is retained (rewritten
    over the ADT) — deleting it would require giving `Estimated` a concrete
    value in `CompiledModel::new`, changing forward-sim error behaviour; the
    ADT's win here is making the illegal states (prior-on-fixed,
    prior+hierarchical) unrepresentable, which it does.
- **Issues:** gh#191 (parameter value conflation), gh#107 (`always_active`
  bool), adjacent gh#12 (param-TOML dimensions)
- **Discrepancy class:** code-vs-code (an IR/schema change) → per `CLAUDE.md`
  "Changing the IR schema", this needs a proposal before implementation.
- **Background:** `docs/dev/notes/2026-06-08-static-typing-as-bug-prevention.md`
  (worked example #1 — this is that example, implemented), and
  `docs/dev/reviews/2026-06-08-systemic-root-causes.md` (RC2 capability gating,
  RC5 forked identity).

## The problem: a flat struct of `Option`s permits states the semantics forbid

The current parameter declaration is a flat record of optionals on both sides of
the IR contract:

```rust
// rust/crates/ir/src/parameter.rs:111
pub struct Parameter {
    pub name:          String,
    pub value:         Option<f64>,                 // :115  None = "supplied at runtime", Some = "present"
    pub bounds:        Option<(f64, f64)>,          // :118  inference only
    pub prior:         Option<PriorDist>,           // :119
    pub hierarchical:  Option<HierarchicalPrior>,   // :124  comment: "mutually exclusive with prior"
    pub transform:     Option<Transform>,           // :125
    pub initial_value: Option<f64>,                 // :126
    pub param_kind:    Option<String>,              // :130  "rate"|"probability"|"positive"|"count"|"real"
    pub param_dim:     Option<(i32, i32)>,          // :136
}
```

```ocaml
(* ocaml/lib/ir/ir.ml:326 — identical shape *)
type parameter = {
  name:          string;
  value:         float option;   (* None = must be supplied at runtime *)
  bounds:        (float * float) option;
  prior:         prior_dist option;
  hierarchical:  hierarchical_prior option;  (* mutually exclusive with prior *)
  transform:     transform option;
  initial_value: float option;
  param_kind:    string option;  (* "rate"|"probability"|"positive"|"count"|"real" *)
  param_dim:     (int * int) option;
}
```

Four illegal states are representable — and one is a shipped bug:

1. **`value: Option<f64>` conflates three distinct meanings.** `None` means
   _either_ "estimated, to be filled by inference" _or_ "genuinely missing —
   author error." `Some(v)` means _either_ "fixed constant `v`" _or_ "estimated
   parameter whose current iterate is `v`." The type cannot tell a fixed `0.3`
   from an estimate currently at `0.3` from a bug.

   **This is shipped as gh#191.** The capability gate builds a `CompiledModel`
   from the raw IR _before_ estimated params resolve, so `compiled_model.rs:557`
   —

   ```rust
   let v = p.value.ok_or_else(|| SimError::Validation(
       format!("parameter '{}' has no value; supply it via --params or --param", p.name)
   ))?;
   ```

   — fires `"parameter 'beta' has no value"` for a perfectly valid estimate-only
   fit. We patched it this session by filling value-less params with a
   placeholder for the structural scan, but **the type still lies**: the
   placeholder is a workaround for a distinction the type refuses to make.

2. **`prior` + `hierarchical` are both `Option` with a comment saying they're
   mutually exclusive.** The type permits both-`Some` (incoherent: a leaf with
   two prior specs) and both-`None` (which _is_ valid — a flat prior — but is
   indistinguishable from "forgot to set one").

3. **`param_kind: Option<String>` is stringly-typed.** A typo (`"raet"`) is not
   rejected at the boundary; every consumer re-parses the string (the kind
   drives dimension defaults in `dimcheck.ml` via `param_dim_of_kind`, the
   transform default in the fit runner, and `table.cell_kind` shares the same
   vocabulary per `table.rs:50`). Adding a kind means finding every match site
   by hand.

4. **`bounds`/`transform`/`prior` are meaningful only for _estimated_
   parameters**, but the flat struct lets you attach a prior or bounds to a
   fixed constant — silently ignored, never flagged.

## Proposed design

Make the parameter an ADT keyed on _what kind of value it is_, so inference
config exists exactly when the parameter is estimated:

```rust
/// Was Option<String>. Rejected at IR deserialisation, not at use.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ParamKind { Rate, Probability, Positive, Count, Real }

/// Collapses `prior` + `hierarchical` — exclusive by construction.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PriorSpec {
    Flat,
    Dist(PriorDist),
    Hierarchical(HierarchicalPrior),
}

/// The three real meanings of the old `value: Option<f64>`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "mode", rename_all = "snake_case")]
pub enum ParamValue {
    /// Known at model-build time; carries no inference config.
    Fixed { value: f64 },
    /// Inference draws this. Config exists *only* here.
    Estimated {
        init:      Option<f64>,        // was initial_value
        bounds:    Option<(f64, f64)>,
        prior:     PriorSpec,
        transform: Transform,          // default Identity
    },
    /// Must be supplied at runtime (--params/--set); no default in the IR.
    Required,
}

pub struct Parameter {
    pub name:  String,
    pub value: ParamValue,
    pub kind:  ParamKind,
    pub dim:   Option<(i32, i32)>,     // unchanged (param_dim)
}
```

OCaml mirrors it with variant types
(`param_value = Fixed of float | Estimated
of {...} | Required`;
`param_kind = Rate | Probability | ...`;
`prior_spec =
Flat | Dist of prior_dist | Hierarchical of hierarchical_prior`).

## What this makes impossible (the payoff)

- **gh#191 disappears structurally.** The capability scan matches
  `ParamValue::Estimated { .. } | ParamValue::Required` without needing a value;
  the `"has no value"` error is reachable _only_ for `Required` — the one case
  it is correct for. The placeholder hack is deleted, not relocated.
- **Prior on a fixed constant:** unrepresentable (`Fixed` has no prior field).
- **Both prior and hierarchical:** unrepresentable (`PriorSpec` is one slot).
- **`param_kind` typo:** rejected at deserialisation (parse-don't-validate); and
  every consumer `match`es an exhaustive enum, so adding a kind is a compile
  error at every site instead of a silent miss.

## gh#107 — same pattern, intervention surface

```rust
// rust/crates/ir/src/intervention.rs:81
pub always_active: bool,   // true for events{}, false for interventions{}
```

`always_active` conflates _which DSL construct declared this_ with _does it
toggle under scenarios_. Replace with a named enum:

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InterventionKind {
    Scenario,   // interventions {} — toggled by enable/disable/set/scale
    Event,      // events {} — fires unconditionally every substep
}
```

This names the distinction and lets a future kind (e.g. reactive, gh#204) extend
exhaustively rather than bolting on a second bool.

## Migration (atomic — per `CLAUDE.md` "Changing the IR schema")

1. `ir/schema.json`: `value` → tagged union
   (`{mode: fixed|estimated|required,
   ...}`); `param_kind` → string enum;
   intervention `always_active` → `kind` enum. Bump `ir/VERSION` 0.9 → 0.10.
2. OCaml `ocaml/lib/ir/{ir.ml,serialize.ml,deserialize.ml}`.
3. Rust `rust/crates/ir/src/{parameter.rs,intervention.rs}` + every consumer
   (the gate at `compiled_model.rs:557`, `dimcheck.ml::param_dim_of_kind`, the
   fit transform-default logic, `validate.rs`).
4. `make test-unit` — fix type errors (the compiler enumerates the consumer
   sites for free; this is the point).
5. `make update-golden && make update-expected` — **17 golden files** reference
   `value`/`param_kind` (`rg -l '"param_kind"|"value"' ir/golden/` → 17). Note
   `ocaml/golden/` regenerates too.
6. One atomic commit: schema + both languages + golden.

> **Golden hygiene** (per `CLAUDE.md` "Goldens are an explicit, reviewed,
> human-loop change" + incident
> `docs/dev/incidents/2026-06-09-golden-format-reverted-by-autoformat.md`): the
> regen here is a legitimate _content_ change (the `value` field's JSON shape
> becomes a tagged `{mode: …}` object). It must stay in `bf5d13b`'s **compact**
> format — one element per line; `sir_basic.ir.json` is tens of lines, not
> hundreds. Do **not** run a broad `dprint fmt` / editor reformat over the tree,
> and do **not** `git add -A` / `commit -a` — that is exactly how 48 goldens got
> silently pretty-printed for 34 days. Stage the golden files explicitly and
> eyeball two to confirm compact shape + the new tagged `value`.

## Risks / open questions (resolve before implementing)

- **`initial_value` / `ivp` parameters — RESOLVED: ivp needs no variant**
  (read-only investigation 2026-06-08). The shared word "initial" conflates two
  orthogonal things:
  - `initial_value` is the **optimizer's starting point** for an estimated
    param: `cli/fit/mod.rs:315` fills a value-less param with
    `p.initial_value.or_else(|| bounds-midpoint).unwrap_or(1.0)`. → maps cleanly
    to `Estimated { init }`.
  - **ivp-ness is derived, not declared** — and not even structurally:
    `pgas.rs:1663` builds `ivp_mappings` by _perturbing_ each estimated param
    and seeing which compartment's initial count moves (`model.initial_state()`
    finite-difference probe). The token `ivp` appears **only** under
    `sim/src/inference/` — never in the IR types, the OCaml compiler, or
    `schema.json`.

  So an ivp parameter is simply an `Estimated` parameter; its ivp role is a
  _relationship to a compartment_ (it lives in `initial_conditions`), not a
  property of the parameter's own declaration. A dedicated `Ivp` variant would
  put that relationship in a **second** place that can disagree with
  `initial_conditions` — re-introducing exactly the multi-source-of-truth smell
  (RC1–RC6) this effort exists to remove; it would _add_ an illegal state, not
  remove one. **Decision: `initial_value` → `Estimated { init }`, no `Ivp`
  variant.**
- **Out of scope (future, separate proposal): the perturbation probe is a latent
  smell.** Detecting ivp-ness by FD-perturbing `initial_state` is empirical, not
  structural. If we want to delete the probe, the typed fix is to make the
  compartment↔param link in `initial_conditions`
  (`Parameterized`/`FromDistribution`) the source the inference layer _reads_ —
  an inference + initial-conditions change, **not** a `ParamValue` variant.
  Tracked separately; do not bundle here. (Inference agent's lane — coordinate.)
- **The compiler already has the distinction at parse time** — `parser.mly`
  productions separate fixed (`name : kind = expr`), estimated-with-prior
  (`... ~ prior`), and bounded (`... in [lo, hi]`) declarations. So the kind
  information _exists_ and is being flattened into `Option`s on the way to the
  IR; the ADT stops the flattening. **Verify** the exact productions and which
  one yields `value = None` vs `Some` before mapping them to variants.
- **Blast radius is wide but mechanical** (17 golden + every consumer). The
  `make test-unit` step turns "find the consumers" into a compile-error worklist
  — but it is a large single commit. Consider landing `ParamKind` and
  `InterventionKind` (the two enum-ifications, low-risk) in one commit and the
  `ParamValue` ADT (the structural change) in a second, each atomic with its own
  golden regen.
- **Backwards compatibility is a non-goal** (alpha): clean break, no alias
  fields, no fallback deserialisation. Old IR with a bare `"value": 0.3` fails
  to deserialise — acceptable, and the deserialiser should say so with a
  migration hint (`CLAUDE.md` "Breaking language changes must signpost").
