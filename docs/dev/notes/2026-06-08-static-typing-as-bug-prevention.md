# Static typing as bug prevention — worked from this codebase

Audience: colleagues who aren't sold on static typing. **Every "before" below is
real code from this repo (file:line), and several are bugs that actually shipped
or are open right now** — not contrived strawmen. Dual purpose: (1) a concrete
argument that the *type system*, not just more tests, would have made whole
classes of these unrepresentable; (2) the type-solvable slice of the open
backlog, with the fix design for each (see the work-list at the end).

## The thesis in one paragraph

A test checks *"did the output change?"* A type checks *"is this state even
constructible?"* The bugs below passed CI precisely because the **wrong state was
representable** and the suite happened to exercise only the path where it looked
fine. The recurring smell across this codebase is **one semantic implemented as N
hand-maintained copies the compiler doesn't force to agree** (a value fn and its
gradient; a production hash and its test reimplementation; an OCaml parser and its
Rust mirror) plus **loosely-typed boundaries** (`Option<f64>`, `f64`-for-everything,
string-concatenated references, `as i64` casts). Each example shows the loose type,
why the compiler stayed silent, and the tighter type that turns the bug into a
compile error or an unrepresentable state. The honest accounting — what typing does
*not* catch, and the cost — is at the end.

---

## 1. ParamValue ADT: "estimated" is a typed state, not a missing f64

**Technique:** ADT / make illegal states unrepresentable (Option<f64> → Fixed | Estimated)  
**Solves:** gh#191  ·  **M-class**

**The bug.** The gh#191 capability gate builds a CompiledModel from the raw IR to scan structural capabilities, but CompiledModel::new errors "parameter '<x>' has no value" for ESTIMATED parameters — which legitimately carry value=None until their start is resolved per-stage from [estimate].start. A purely structural check (transitions/compartments/balance) refused to run on any estimate-only fit, so the gate had to be papered over with fake placeholder values.

**Before (real code):**

```rust
// rust/crates/ir/src/parameter.rs:110-116
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Parameter {
    pub name:          String,
    /// `None` = must be supplied at runtime via --params / --set.
    /// `Some(v)` = value present (either from hand-crafted IR or applied override).
    pub value:         Option<f64>,
    ...
}

// rust/crates/sim/src/compiled_model.rs:555-561 — the resolution that conflates
// "estimated, resolved later" with "user forgot to supply a value":
for (i, p) in model.parameters.iter().enumerate() {
    param_index.insert(p.name.clone(), i);
    let v = p.value.ok_or_else(|| SimError::Validation(
        format!("parameter '{}' has no value; supply it via --params or --param", p.name)
    ))?;
    default_params.push(v);
}

// rust/crates/cli/src/fit/mod.rs:314-323 — the workaround the bad type forced:
// invent a placeholder for every value-less param so a STRUCTURAL scan can run.
let mut cap_model = model.clone();
for p in &mut cap_model.parameters {
    if p.value.is_none() {
        p.value = Some(
            p.initial_value
                .or_else(|| p.bounds.map(|(lo, hi)| 0.5 * (lo + hi)))
                .unwrap_or(1.0),
        );
    }
}
let compiled = sim::CompiledModel::new(cap_model).unwrap_or_else(...);
if let Err(msg) = gate_run_stages_against_model(&stages_to_run, &compiled) { ... }
```

**Why the compiler stayed silent.** `Option<f64>` collapses two semantically distinct states into one `None`: "the user forgot to supply a value" (a real error) and "this parameter is estimated, its value is resolved later from [estimate].start" (legal, expected). The type carries no evidence of which one a `None` is, so every consumer must guess from context. CompiledModel::new guessed "error" — correct for a forward simulate, wrong for a structural capability scan. The compiler can't flag the mismatch because the absence of a value is, at the type level, exactly the same thing in both call sites. The doc comment on the field even spells the conflation out ("must be supplied at runtime" vs "value present") without distinguishing the estimated case at all.

**After (the typed design):**

```rust
// rust/crates/ir/src/parameter.rs — make "estimated" a first-class state.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum ParamValue {
    /// Concrete value: hand-crafted IR, applied --param override, or fixed scenario.
    Fixed { value: f64 },
    /// To be inferred; resolved per-stage from [estimate].start. NOT an error.
    Estimated { bounds: (f64, f64) },
}

pub struct Parameter {
    pub name:  String,
    pub value: ParamValue,   // total — every parameter is in a known state
    ...
}

// compiled_model.rs: a structural scan asks ONLY what it needs and can never
// demand a number it doesn't use. Resolution becomes total — no .ok_or_else,
// no "has no value" path reachable from the gate at all:
let v = match &p.value {
    ParamValue::Fixed { value } => *value,
    // For the capability scan a representative point in-bounds is well-defined,
    // not a fabricated placeholder; for a forward run the dispatcher requires
    // a Fixed value by *construction* (resolve_estimates(&start) -> Fixed),
    // so the demand lives at the boundary, typed, instead of deep in `new`.
    ParamValue::Estimated { bounds } => 0.5 * (bounds.0 + bounds.1),
};

// fit/mod.rs: the clone-and-fill loop deletes entirely. The gate operates on a
// model whose estimated params are already a valid typed state; the capability
// check (structural) never touches a value, and there is no None to backfill.
```

**What this still doesn't catch (honest).** The ADT removes the "estimated == missing" conflation and the placeholder loop, but it does NOT by itself solve gh#191's substantive bug: real (ODE) reservoir state is frozen at init in the filter loops because ParticleState carries no real compartments — that's a missing-feature/dynamics bug, untouched by parameter typing (gh#191's real fix is the interim REAL_COMPARTMENTS gate plus carrying the reservoir in ParticleState). The type also can't enforce that Estimated.bounds is well-ordered (lo < hi) or that a Fixed value lies in any sane domain — those stay validation-time checks unless you go further with a refined/newtype bound. And serde will still happily deserialize an Estimated param in a context (plain forward `simulate`) that has no estimator to resolve it; making *that* unrepresentable needs a separate resolved-vs-unresolved model type (a "parse, don't validate" boundary), not just this enum.

---

## 2. One (value, grad) traversal beats two hand-synced density functions: the gamma-multiplier term that exists in the score but not the energy

**Technique:** AD type / single source: collapse two separately-maintained functions (one returns the log-density VALUE, one returns its GRADIENT) into a single traversal that returns `(value, grad)` together — or a dual-number `Ad` type — so a density term cannot be present in one and absent in the other.  
**Solves:** gh#197 (OPEN, the headline bug), gh#200 (OPEN, same value/grad-divergence class — gradient scores deterministic source-less transitions as Poisson while the value uses an exact-count check), gh#79 (OPEN, "Restructure shared gamma-density iterator in pgas value+gradient" — the exact dual-source refactor). Earlier instances of the same shape were patched one-off as gh#20 (gamma grad added) and gh#76 (obs grad added).  ·  **M-class**

**The bug.** In PGAS+NUTS, the log-likelihood VALUE (`complete_data_loglik`, pgas.rs) adds the gamma-multiplier density term to `transition_ll`, but the GRADIENT (`complete_data_loglik_grad`, pgas_grad.rs) adds the gamma term's derivative to `grad` while never adding the term's value to `log_p`. NUTS therefore integrates a Hamiltonian whose force (∂/∂σ²) does not match its energy (log_p), so the σ² (overdispersion) posterior is biased.

**Before (real code):**

```rust
VALUE side — rust/crates/sim/src/inference/pgas.rs:800-804 (inside complete_data_loglik):
    let log_gamma_density = (shape - 1.0) * g.max(LOG_PROB_FLOOR).ln()
        - g / scale
        - shape * scale.ln()
        - crate::inference::obs_loglik::lgamma(shape);
    transition_ll += log_gamma_density;        // <-- VALUE term accumulated

GRAD side — rust/crates/sim/src/inference/pgas_grad.rs:431-434 (inside complete_data_loglik_grad):
    let gamma_grad = log_gamma_density_grad_substep(
        model, counts_before, &rec.gammas, params, t, dt_s, estimated_to_model,
    )?;
    for i in 0..d { grad[i] += gamma_grad[i]; }   // <-- GRADIENT accumulated, but NO `log_p += <gamma value>` companion

Verification that the value site is missing on the grad side:
  rg -n "log_p \+=|gamma_grad|log_gamma_density" rust/crates/sim/src/inference/pgas_grad.rs
  → log_p += appears for binom (IVP, :385), transition td (:422), obs (:444); for gamma ONLY `grad[i] += gamma_grad[i]` (:434) — no log_p companion.
The grad fn returns `(log_p, grad)` at :460 with log_p missing the term whose gradient is in grad.
```

**Why the compiler stayed silent.** Both functions satisfy the type `-> Result<(f64, Vec<f64>), SimError>` (grad) and `-> Result<LogLikComponents, SimError>` (value). The contract that binds them — "every term contributing to `grad` must also contribute to `log_p`, with the SAME shape/scale/gamma_idx iteration" — is stated only in a doc comment ("Mirrors the gamma-density loop in `pgas::complete_data_loglik` exactly … same source_group iteration order, same gamma_idx accounting", pgas_grad.rs:265-268). `f64` is `f64`: the compiler cannot see that one `f64` is the integral of the other. Two independent loops over `model.source_groups` can drift (a term added to one, a guard tightened in only one — exactly gh#200) and still typecheck. Nothing forces the value and its derivative to be produced by the same code path.

**After (the typed design):**

```rust
Make the value and its gradient impossible to author separately by computing them in one traversal that returns both, or by carrying a dual number through a single expression:

    /// Forward-mode dual: a scalar value paired with its gradient w.r.t. the
    /// estimated parameters. Every arithmetic op updates both halves, so a term
    /// can never be in `val` without its contribution being in `grad`.
    #[derive(Clone)]
    struct Ad { val: f64, grad: Vec<f64> }   // grad.len() == d
    impl Ad {
        fn constant(v: f64, d: usize) -> Self { Ad { val: v, grad: vec![0.0; d] } }
        fn add_assign(&mut self, o: &Ad) {
            self.val += o.val;
            for i in 0..self.grad.len() { self.grad[i] += o.grad[i]; }
        }
    }

    // ONE gamma-density helper, returning an Ad — used by both call sites:
    fn log_gamma_density_ad(/* model, counts_before, gammas, params, t, dt, estimated_to_model */) -> Ad {
        // (shape-1)*ln(g) - g/scale - shape*ln(scale) - lgamma(shape)
        //   val  = the four scalar terms
        //   grad = dlg_dsq * eval_resolved_deriv(...)  (the existing chain rule)
        // Both filled in the SAME `if let Some(resolved_od)` branch, advancing
        // ONE gamma_idx — so the guards (rate > eps, !Deterministic, Some(od))
        // are shared, not duplicated.  This is the gh#79 "shared iterator".
    }

    // complete_data_loglik_grad now accumulates Ad:
    let mut acc = Ad::constant(0.0, d);
    acc.add_assign(&log_transition_density_ad(...));
    acc.add_assign(&log_gamma_density_ad(...));   // value AND grad, together — gh#197 impossible
    acc.add_assign(&obs_density_ad(...));
    // ...
    Ok((acc.val, acc.grad))   // val and grad are the same expression, by construction

The point: there is no longer a place to write a term's derivative without its value. The grad fn's return is literally `(acc.val, acc.grad)` from one accumulator; omitting the gamma value would require omitting its gradient too (and then the term simply isn't there on either side — which is at least *consistent*, the property NUTS actually needs).
```

**What this still doesn't catch (honest).** Honest limits a skeptic should weigh:
1. An `Ad` type guarantees val/grad consistency PER TERM, but does not guarantee the per-term math is correct: a wrong analytic derivative inside `log_gamma_density_ad` (e.g. a sign error in `dlg_dscale`) is consistent val-vs-grad and still typechecks. Catching that needs a finite-difference check test, not types. (camdl deliberately uses compiler-emitted symbolic derivatives, not runtime autodiff — so the `Ad` here is a discipline for the hand-written density terms, and the FD test stays mandatory.)
2. It does not by itself fix gh#200's other half: the divergence there is in the *iteration predicate* (deterministic source-less transitions scored as Poisson on one side, exact-count on the other). A shared `Ad`-returning iterator fixes it only if both sides are forced to call that one iterator; if the value path keeps its own loop, the predicate can still drift. The structural fix (gh#79) is "one iterator, two consumers," which the `Ad` return enables but a careless refactor could still bypass.
3. `Ad` carries a `Vec<f64>` of length d per scalar — allocation/perf cost in a hot PGAS inner loop. A real implementation would want a fixed-size or arena-backed gradient buffer, or to keep the single-traversal-returning-(value,grad) form (cheaper) rather than a per-scalar dual number. The teaching point (single source of truth for value+grad) holds for either; the dual-number sketch is the clearer illustration, the single-traversal form is the one you'd ship.

---

## 3. resolve-don't-stringly-type: indexed references lowered by positional String.concat drop the dimension label

**Technique:** resolve don't stringly-type (parse to a typed, dimension-checked reference instead of a positionally-concatenated string)  
**Solves:** gh#111 (OPEN — the resolver was never built), gh#112 (CLOSED, fixed in f61db93a as a worked example: table-lookup arity). Per the issue, #111's resolver subsumes upstream Highs #9/#10/#12 and parts of #13.  ·  **L-class**

**The bug.** An indexed reference like S[patch = p] or S[sex = female, age = child] is lowered by throwing away the dimension label and gluing the index values onto the base name positionally with String.concat "_". A named index whose label does not match the declaration order, an under-indexed reference, or an over-arity reference all produce a wrong/phantom concrete name rather than an error — silently binding the wrong compartment, flow, or table cell that downstream force-of-infection and likelihood depend on.

**Before (real code):**

```ocaml
ocaml/lib/compiler/expander.ml:1236-1241 — the label is matched as `_` and discarded:

    let index_item_to_str env item =
      match item with
      | IPosn (EIdent (s, _))     -> (match List.assoc_opt s env with Some v -> v | None -> s)
      | IPosn _                   -> "?"
      | INamed (_, EIdent (s, _)) -> (match List.assoc_opt s env with Some v -> v | None -> s)
      | INamed (_, _)             -> "?"

ocaml/lib/compiler/expander.ml:2007-2009 — the lowering site, positional concat, no arity/membership check:

    (* 4. Compartment with indices → concatenate to concrete name *)
    let idx_vals = List.map (index_item_to_str env) items in
    let concrete = String.concat "_" (base_name :: idx_vals) in

The same `index_item_to_str` + `String.concat "_"` shape repeats at expander.ml:1968, 1993, 2004, 4215, 4239, 4311, 4341, 4350, 4375, 3520, 3532, 4477 (compartments, transition flows, indexed time functions, indexed params, interventions ASet/AAdd, cumulative-flow projections, observations). The type of the input is `index_item = IPosn of expr | INamed of string * expr` (ast.ml:38-40) and `EIndex of string * index_item list` (ast.ml:52) — `base` is just a `string` and there is no carried declared-dimension vector, so nothing forces a consistency check.
```

**Why the compiler stayed silent.** The reference's *identity* is a bare `string` (`EIndex of string * index_item list`) and its lowered form is another bare `string` (`Ir.Param`/compartment name produced by `String.concat`). The dimension vector of the referenced object is never reified into a type: `index_item_to_str` takes one `index_item` in isolation, with no access to "what dimensions does `base` declare and in what order," so OCaml's exhaustive match on `INamed (label, _)` *can* bind `label` but the surrounding function has nothing to validate it against — discarding it with `_` typechecks fine. List length (arity) and label-to-dimension membership are runtime list facts, not type-level facts, so `List.map index_item_to_str items` over an items list of any length, in any order, is well-typed. The compiler enforces that you produced *a* string, never that the string names a real cell. (gh#112 showed the dual: `List.mapi` ran over `items` not `tdims`, so a too-short item list typechecked and produced a partial-prefix linear index.)

**After (the typed design):**

```ocaml
Resolve at the EIndex site to a typed reference, against the object's declared dimensions, so "wrong cell / phantom name / over-arity" is unrepresentable past resolution:

    (* A dimension binding is a (dimension, level), never a bare positional string. *)
    type dim_binding = { dim : dim_name; level : level }   (* both validated members *)

    type resolved_ref =
      | RCompartment of comp_id                 (* a fully-bound cell *)
      | RFlowSum     of comp_id list            (* omitted dims → explicit sum, per spec *)

    (* Single entry point; replaces index_item_to_str + every String.concat site. *)
    val resolve_indexed_ref :
      ctx -> env ->
      namespace:[`Compartment|`TransitionFlow|`Table|`Parameter|`Let|`Forcing] ->
      base:string -> index_item list ->
      (resolved_ref, Diagnostics.t) result

`resolve_indexed_ref` looks up the declared dimension vector for `base`, then for each `index_item`:
  - `INamed (label, v)` → find the dimension *named* `label` (order-independent); error E-code if `label` is not a declared dimension of `base`;
  - `IPosn v` → bind by declaration order;
checks each `v` is a real level of that dimension (membership), checks no dimension is bound twice, and decides omitted dimensions: error in a scalar position, expand to `RFlowSum` in an expression/projection position (spec's omitted-dimension summation). Only a fully-resolved `comp_id` can be turned into a name. Because the lowering now consumes a `resolved_ref` (a `comp_id` carries the proof it was built from a validated dim_binding per declared dimension), there is no code path that emits a concrete name from a label-dropped, under-arity, or out-of-order index list — the positional `String.concat` is deleted, and `S[patch = p]` can no longer alias `S_p` by position.
```

**What this still doesn't catch (honest).** Types here buy validity-by-construction of the *reference* (right arity, real dimension, real level, label honored), not correctness of the *model*. What this still does NOT catch: (1) the modeler binding the wrong-but-valid level — `S[age = child]` when they meant `adult` is two legal `comp_id`s and indistinguishable to any type; (2) whether expanding an omitted dimension to a *sum* (RFlowSum) is what the epidemiologist intended vs. an error they'd want flagged — that's a spec policy choice the type only encodes once you've decided it; (3) the dimension/level *names themselves* are still strings sourced from the DSL, so a typo in a `dimensions{}` declaration produces a valid-but-wrong universe of levels that resolution will faithfully honor; (4) it does nothing for the numeric/units correctness of the expression the reference sits inside (that's dimcheck's job). And the resolver only helps if every lowering site is actually migrated to consume `resolved_ref` — until the ~14 `index_item_to_str`/`String.concat` sites are all replaced, a single un-migrated site re-opens the stringly-typed hole, which is why gh#111 remains open after gh#112's localized table-only fix.

---

## 4. Derive the content hash from the type, not a hand-maintained allowlist

**Technique:** derive don't hand-list (make "enumerate every input" a property of the type, via #[derive(RunInput)] / a content-hash trait, instead of a string allowlist humans maintain — and route every consumer, incl. tests, through that one impl)  
**Solves:** gh#147 (closed — fixed by extending the allowlist, the worked example); gh#189 and gh#190 (both open, both M-class, both cite the derive design as the fix: "field-add re-keys automatically via #[derive(RunInput)]")  ·  **M-class**

**The bug.** `model_hash` keyed the simulation cache off a hand-written allowlist of IR field names. The list omitted `output` cadence, `simulation.t_end`, `origin`/`origin_rata_die`, and `time_unit`, so two models differing only in (e.g.) the run horizon or calendar origin hashed equal — the second run was silently served the first run's cached trajectory. A silent wrong answer in software that informs public-health decisions, not a crash. The same allowlist was hand-copied into 3 integration tests (`model_hash_for_test`), a forked source-of-truth free to drift from production.

**Before (real code):**

```rust
// rust/crates/cli/src/hashing.rs:42-51 (current HEAD f61db93a — gh#147
// "fixed" this by appending three more strings to the list, not by deriving)
    let mut h = Sha256::new();
    let structural_keys = [
        "compartments", "transitions", "parameters", "tables",
        "time_functions", "interventions", "observations",
        "ode_equations", "initial_conditions",
        // gh#147: calendar/time-axis context.
        "origin", "origin_rata_die", "time_unit",
    ];
    for key in &structural_keys {
        if let Some(val) = obj.get(*key) {
            h.update(key.as_bytes()); h.update(b"\x00");
            h.update(serde_json::to_string(val).unwrap().as_bytes());
            h.update(b"\x00");
        }
    }
    // ...then `output.times`, `simulation.{t_start,t_end}`, `version` are
    // hand-appended below as one-off special cases (hashing.rs:64-86).

// And the forked reimplementation, copied verbatim into the test:
// rust/crates/cli/tests/survey_top_k_pmmh.rs:59-73
fn model_hash_for_test(ir_json: &str) -> String {
    // ...
    let structural_keys = [
        "compartments", "transitions", "parameters", "tables",
        "time_functions", "interventions", "observations",
        "ode_equations", "initial_conditions",
        "origin", "origin_rata_die", "time_unit",   // gh#147 (re-typed by hand)
    ];
    // ... (identical loop) ...
}
// The same fn is copied again into survey_top_k_pgas.rs:58 and
// pmmh_bad_init_skip.rs:65 — three hand replicas of one algorithm.
```

**Why the compiler stayed silent.** The cache key is computed by iterating a `[&str]` literal and `obj.get(key)`-ing an untyped `serde_json::Value`. The IR's real field set is data the compiler never sees at the hash site: adding `time_unit` to the schema (OCaml + Rust IR types) does not change the `&str` array, and the type checker has no obligation linking "field on the model type" to "string in this list." So omitting a trajectory-determining field is well-typed — it compiles, runs, and returns a hash that happens to ignore that field. The test replicas are even worse: the only thing tying `model_hash_for_test` to `model_hash` is a doc-comment ("Replicate `crate::hashing::model_hash`"), which the compiler cannot enforce. Both can drift to green independently.

**After (the typed design):**

```rust
// The fix that actually shipped in the `runid` crate: hash the *type*, not a
// list of its field names. Adding a field to the type adds it to the hash —
// forgetting is impossible, because a non-hashable field is a *compile error*.
//
// rust/crates/runid-derive/src/lib.rs — the derive emits, for each field in
// declaration order, `<FieldTy as ContentAddressed>::hash_into(&self.f, h)`
// after a domain-separation type tag + schema version. Per its own header:
//   "A field whose type is not ContentAddressed is a compile error ... you
//    cannot forget to make an input hashable."

#[derive(RunInput)]                  // include-by-default over all fields
#[run_input(schema_version = 1)]     // bump to deliberately re-key this type
struct ModelInput {
    compartments:       Vec<Compartment>,
    transitions:        Vec<Transition>,
    parameters:         Vec<Parameter>,
    tables:             Vec<Table>,
    time_functions:     Vec<TimeFunc>,
    interventions:      Vec<Intervention>,
    observations:       Vec<Observation>,
    ode_equations:      Vec<OdeEq>,
    initial_conditions: InitialConditions,
    origin:             Option<CalendarOrigin>,
    time_unit:          TimeUnit,
    horizon:            Horizon,        // t_start/t_end — was a string special-case
    output_cadence:     OutputTimes,    // was the `output.times` one-off
    #[run_input(provenance)]            // recorded in run.json, intentionally NOT hashed
    output_format:      OutputFormat,   // presentation-only — explicit, not a silent omission
}
// Add a new schema field tomorrow → its `hash_into` is generated → the key
// changes automatically. There is no list to forget. Every consumer
// (batch.rs, survey.rs, fit/{pgas,pmmh}.rs, and the tests) calls the ONE
// `.content_hash()`; the derive's golden macro-equivalence test forbids a
// second hand impl — killing the `model_hash_for_test` fork by construction.
```

**What this still doesn't catch (honest).** Honest gaps a skeptic should hold onto: (1) The derive guarantees every *field* is hashed, but the *include/exclude* policy is still a human judgement — marking a genuinely trajectory-determining field `#[run_input(provenance)]` re-creates the exact gh#147 bug, just spelled differently. The win is that the omission is now an explicit, reviewable annotation at the field, not an invisible absence from a far-away string list. (2) It does not catch impurity — if `f` reads `dt`, the clock, or an env var the input type doesn't enumerate, the hash is still blind to it; the proposal flags this separately (the degeneracy watchdog reads wall-clock) and that is exactly the shape of open gh#189/#190 (`dt`/`obs_alignment`/holdout-bytes missing from a *different* identity payload). (3) `schema_version` is a manual escape hatch: a semantic meaning-change to a field that keeps its bytes (e.g. reinterpreting an existing number) still needs a human to bump the version — the type cannot see that. (4) As of HEAD the migration is incomplete: `runid` + the derive exist, but live `hashing.rs::model_hash` was *not* ported onto it (gh#147 was closed by appending strings), so the allowlist and the three test replicas are still in the tree — the type-level fix is built but not yet routed through here.

---

## 5. newtypes for time scalars: making the StepClock dt/grid_dt swap a compile error

**Technique:** newtype (one f64 field per semantic role) — distinct types for distinct quantities so a transposition fails to typecheck  
**Solves:** Review finding #11 (StepClock dual-dt invariant rides on adjacent bare f64). No matching open gh# issue found in-tree; this is the audit/teaching example. Same hazard class as time_to_step(t, dt) transposition.  ·  **L-class**

**The bug.** step_one takes the realized substep length and the nominal model grid as two adjacent bare f64 params (dt, grid_dt) with distinct meanings — dt drives the transition probability/overdispersion math, grid_dt keys intervention/event firing. Transposing them at a call site compiles, passes every on-grid golden (where dt == grid_dt), and silently corrupts only the off-grid inference path (PGAS/correlated-PF clipping to an off-grid observation) — i.e. exactly the runs that feed a posterior.

**Before (real code):**

```rust
// rust/crates/sim/src/chain_binomial.rs:313-325
pub fn step_one(
    model: &CompiledModel,
    counts: &mut [i64],
    flows: &mut [u64],
    real: &mut RealState,
    params: &[f64],
    t: f64,
    dt: f64,        // dt_actual: rate eval, 1-exp(-rate*dt), shape = dt/sigma^2
    grid_dt: f64,   // nominal model dt: keys fire_steps firing only
    rng: &mut StatefulRng,
    scratch: &mut StepScratch,
    fire_steps: &[std::collections::BTreeSet<i64>],
) -> Result<(), SimError> {

// the off-grid inference call site where dt != grid_dt
// rust/crates/sim/src/inference/pgas.rs:1140-1147
step_one(
    model, &mut counts[j], &mut substep_flows[j],
    &mut particle_reals[j],
    // `step_dt` is the realized substep (clipped under Exact); `dt` is
    // the nominal grid the `fire_steps` were built on -> keys firing.
    params, t, step_dt, dt, &mut rngs[j], &mut scratches[j],
    &fire_steps,
)?;

// and the conversion primitive with the same transposable shape
// rust/crates/sim/src/time.rs:30
pub fn time_to_step(t: f64, dt: f64) -> i64 {
    (t / dt).round() as i64
}
```

**Why the compiler stayed silent.** All three quantities are f64, so the type checker treats `step_dt` and `dt` (grid) as interchangeable in adjacent argument positions; `step_one(..., dt, step_dt, ...)` typechecks identically. The invariant "evaluate the kernel on dt_actual, key fire-steps on grid_dt" lives only in a doc comment and a parameter name, neither of which the compiler enforces. The same is true of `time_to_step(t, dt)`: `time_to_step(dt, t)` is well-typed and silently wrong. The on-grid Snap forward path and every single-dt golden have dt == grid_dt, so the swap is observationally identical there — the divergence only appears when an Exact inference substep is clipped to an off-grid observation, which is the one path with no golden coverage.

**After (the typed design):**

```rust
// One field per role; transposition becomes a type error.
#[derive(Clone, Copy, PartialEq, PartialOrd)] pub struct Time(pub f64);
#[derive(Clone, Copy, PartialEq, PartialOrd)] pub struct Dt(pub f64);      // dt_actual
#[derive(Clone, Copy, PartialEq, PartialOrd)] pub struct GridDt(pub f64);  // nominal model grid

pub fn step_one(
    model: &CompiledModel, counts: &mut [i64], flows: &mut [u64],
    real: &mut RealState, params: &[f64],
    t: Time, dt: Dt, grid_dt: GridDt,   // Dt and GridDt no longer unify
    rng: &mut StatefulRng, scratch: &mut StepScratch,
    fire_steps: &[BTreeSet<i64>],
) -> Result<(), SimError> { /* rate uses dt.0; firing uses grid_dt.0 */ }

// Keying conversion takes a GridDt, not any old f64:
pub fn time_to_step(t: Time, grid: GridDt) -> i64 { (t.0 / grid.0).round() as i64 }

// Now the pgas.rs swap is rejected:
//   step_one(.., t, grid_dt /*GridDt*/, step_dt /*Dt*/, ..)
//   ^ expected `Dt`, found `GridDt`  — compile error, not a silent posterior bug.

// Same move applies to the other physical/index roles flagged:
//   Count(i64) / Rate(f64) / Prob(f64) / LogDensity(f64)  — so a raw rate
//     can't be passed where a probability is expected (1-exp(-rate*dt));
//   ParticleIdx(usize) / CompartmentIdx(usize) — so indexing counts[] with a
//     particle index (or vice versa) fails to typecheck.
// Arithmetic stays cheap: impl Sub<Time> for Time -> Dt; impl Mul<f64> for Dt;
// #[repr(transparent)] keeps it zero-cost.
```

**What this still doesn't catch (honest).** Newtypes catch the transposition of two arguments of *different* roles, not two of the *same* role: swapping two `Time` args (e.g. `t0`/`t1` in `interval_steps(t0, t1, dt)`) still typechecks — only the `dt1 >= dt0` debug_assert guards that. They don't enforce the numeric invariant itself: nothing stops a caller constructing `GridDt(step_dt.0)` and re-introducing the bug, because the wrap is an explicit, unchecked coercion (the boundary where you read an f64 off the IR or out of cfg.dt is still a trust point). They add no protection against the values being *computed* wrong upstream (a mis-clipped `step_dt` is a valid `Dt`). And `PartialOrd`/arithmetic impls must be written deliberately — a careless `impl Add<Dt> for Time` that also accepts `GridDt` would re-open the hole. The off-grid path still needs the existing bit-exactness test (`substep_is_bit_exact_dt_min_not_t_to_minus_t`) — types pin *which* quantity flows where, not *that the float math is right*.

---

## 6. parse-don't-validate: a typed Count at the IR/compile boundary, not a raw f64 cast at simulation time

**Technique:** parse, don't validate (smart-constructor newtype `Count`) + resolve-don't-stringly-type (range-check the table index at compile time so the eval path is infallible / has no panic)  
**Solves:** gh#124 (CLOSED, used as worked example), gh#127 (OPEN, the OOB table panic in resolved_expr.rs). Couples to gh#123 (validator completeness).  ·  **L-class**

**The bug.** Compartment initial-condition values flow as raw `f64` all the way into the simulator, where they are cast `*val as i64` / `v.round() as i64` with no finiteness, nonnegativity, or integrality check — so `I0 = -3`, `NaN`, `0.6`, or `1e20` silently become a wrong i64 seed. Separately, a dynamic table-lookup index that goes out of range `panic!`s inside the per-substep, per-particle hot evaluator instead of being rejected at compile time.

**Before (real code):**

```rust
// rust/crates/sim/src/compiled_model.rs:1075 (explicit ICs) and :1092 (parameterized ICs)
if let Some(local) = self.global_to_int[global] {
    int_counts[local] = *val as i64;          // :1075  no finite/nonneg/integer check
} else if let Some(local) = self.global_to_real[global] {
    real_values[local] = *val;
}
// ...
let v = eval_expr(expr, &ctx)?;
if let Some(local) = self.global_to_int[global] {
    int_counts[local] = v.round() as i64;     // :1092  NaN/neg/1e20 silently coerced
} else if let Some(local) = self.global_to_real[global] {
    real_values[local] = v;
}

// rust/crates/sim/src/resolved_expr.rs:476-498 (the hot eval path)
ResolvedExpr::TableLookup { table_idx, oob, table_len, index } => {
    let cached = &ctx.model.table_values_cache[*table_idx];
    let raw = eval_resolved(index, ctx);
    let table_idx_val = raw.floor() as i64;
    let n = *table_len as i64;
    let i = match oob {
        OobPolicy::Error => {
            if table_idx_val < 0 || table_idx_val >= n {
                panic!(                                  // :489  hard panic in per-particle loop
                    "table lookup out of bounds: index {} not in [0, {}). \
                     Widen the table bounds or fix the index expression.",
                    table_idx_val, n
                );
            }
            table_idx_val
        }
    };
    cached[i as usize]
}
```

**Why the compiler stayed silent.** The IR's `InitialConditions::Explicit` carries `f64` and the int-compartment slot is `i64`. `as i64` is Rust's total, lossy, *infallible* primitive cast: `NaN as i64 == 0`, `-3.0 as i64 == -3`, `0.6 as i64 == 0`, `1e20 as i64` saturates. The compiler is *satisfied* — every f64 maps to some i64, so there is no type to make it reject. The type `f64` literally encodes "any float, including the invalid ones"; the validity predicate (finite ∧ ≥0 ∧ integral) lives only in the programmer's head, not in the type, so nothing forces a check before the cast. Likewise the table index is a runtime f64 with the in-range invariant unstated in any type; the only enforcement point is a runtime branch in the evaluator, and the chosen failure mode there is `panic!` — which the type system also can't see as "this function can fail," because a panic is not in the return type.

**After (the typed design):**

```rust
// A count is a *parsed* nonnegative integer, not a float you cast later.
// Constructed once at the IR→CompiledModel boundary; the field type then
// guarantees every downstream reader holds a valid seed.
pub struct Count(i64);

impl Count {
    pub fn new(name: &str, v: f64) -> Result<Count, SimError> {
        if !v.is_finite() {
            return Err(SimError::Validation(format!("init for '{name}' is not finite (got {v})")));
        }
        if v < 0.0 {
            return Err(SimError::Validation(format!("init for '{name}' must be nonnegative (got {v})")));
        }
        if (v - v.round()).abs() > 1e-9 {
            return Err(SimError::Validation(format!("init for '{name}' must be an integer (got {v})")));
        }
        Ok(Count(v.round() as i64))          // the ONLY `as i64` in the codebase, behind the proof
    }
    pub fn get(self) -> i64 { self.0 }
}

// compiled_model.rs init: the cast is now unrepresentable without going through Count::new
int_counts[local] = Count::new(name, *val)?.get();   // explicit
int_counts[local] = Count::new(name, eval_expr(expr,&ctx)?)?.get();  // parameterized

// Table index: resolve+range-check at construction so the variant carries a *checked* index kind,
// and the eval path is total — no panic, no Result needed on the hot path.
// In CompiledModel::new, for a constant-index lookup:
let checked = TableIdx::new(table_idx_val, table_len)
    .ok_or_else(|| SimError::TableLookup(format!("index {table_idx_val} not in [0,{table_len})")))?;
// ResolvedExpr::TableLookup { values: Vec<f64>, index: CheckedTableIndex }
// eval: cached[index.get()]   // infallible: TableIdx can only hold an in-bounds usize
// For a *dynamic* (state-dependent) index, the eval path returns Result<f64,SimError>
// (per gh#127's fix) — never panic! — converting the model assertion into a per-particle error.
```

**What this still doesn't catch (honest).** Honest limits a skeptic should hear: (1) `Count` only makes the *count* valid in isolation — it does not make the *sum* of counts equal N, nor catch a biologically wrong-but-valid seed like `I0 = 10^6` in a population of 1000; that's a cross-field invariant a single newtype can't express. (2) For a *dynamic* table index whose value depends on simulation state, no compile-time `TableIdx` can prove in-range — the index isn't known until eval, so the best types buy you is forcing the eval path to return `Result` (a controlled per-particle error) instead of `panic!`; the *possibility* of OOB at runtime is irreducible, types only relocate it from panic to a typed error. (3) The smart constructor's correctness still rests on a hand-written predicate (the `1e-9` integrality tolerance, the finite/nonneg checks) — if that predicate is wrong, the type faithfully enforces the wrong thing; types move the check to one auditable place but don't verify the check itself. (4) Nothing here stops a future contributor from writing a fresh `as i64` elsewhere — you'd need a lint (e.g. clippy `cast_possible_truncation`) or a grep gate to keep the boundary the *only* cast site.

---

## 7. Make a dropped derivative an explicit typed state, not a silent Const 0.0

**Technique:** make silent states explicit (ADT: Deriv = Known of expr | Unsupported of reason) — turn an implicit "I gave up and returned 0" into a value the caller is forced to handle  
**Solves:** #119 (parametric forcing/table params un-estimable — frozen on the Rust side AND zero-gradient on the OCaml side), #128 (unknown rate_grad keys; same silent-wrong-posterior class), #180 (open: parametric DerivedExpr projection drops a chain-rule term — same "dropped derivative term" shape)  ·  **M-class**

**The bug.** OCaml autodiff differentiates TimeFunc and TableLookup nodes to Const 0.0 unconditionally. A parameter that enters a transition rate ONLY through a forcing function (e.g. seasonal amplitude) or a table entry gets a silent zero gradient: it's omitted from rate_grad, the likelihood is flat along that axis, and NUTS/PGAS report a clean-looking posterior that is just the prior. No error, anywhere.

**Before (real code):**

```ocaml
ocaml/lib/ir/autodiff.ml:23-25 (verbatim, current main):

  | TimeFunc _    -> Const 0.0
  | TableLookup _ -> Const 0.0
  | Projected     -> Const 0.0

Propagation: differentiate_rate (autodiff.ml:282-287) drops any param whose
derivative simplifies to Const 0.0 —
    | Const 0.0 -> None
so the param never appears in the emitted rate_grad map. The Rust side
(compiled_model.rs:916-930, the gh#128 fix) only hard-errors on rate_grad keys
for UNKNOWN params; a declared param with NO entry is read as zero gradient. So
the #128 check structurally cannot catch this — there is no key to reject.

Contrast: the Mod arm (autodiff.ml:94-114) already does the right thing — it
runs `mentions param` over the operands and `failwith`s if the param is present
rather than returning a silent 0. TimeFunc/TableLookup predate that discipline.
```

**Why the compiler stayed silent.** `differentiate : expr -> string -> expr` has return type `expr`, and `Const 0.0` is a perfectly well-typed `expr`. "True zero derivative" (param absent) and "I cannot/did-not differentiate this, here's a zero placeholder" are the same inhabitant of the type, so the compiler has no way to distinguish them and no obligation it can impose on the caller. Worse, `TimeFunc of string` carries only the forcing's NAME — the parametric arguments (amplitude/period/phase) live in a separate `time_function` record (ir.ml:147-156), reachable via that string. So at the `TimeFunc _` node the function literally cannot see whether `param` is involved; returning 0 is locally type-correct and locally blind. Same for `TableLookup (_, args)`: the dependence is in `args`, but the arm ignores them.

**After (the typed design):**

```ocaml
Make "dropped derivative" a state the type forces the caller to handle:

  type deriv = Known of expr | Unsupported of { node : string; reason : string }

  let rec differentiate (e : expr) (param : string) : deriv =
    match e with
    | Const _ | Pop _ | PopSum _ | Time | Dt | Projected -> Known (Const 0.0)
    (* TableLookup: differentiate THROUGH the args when the param appears *)
    | TableLookup (name, args) ->
        if List.exists (mentions param) args then
          Unsupported { node = "table `" ^ name ^ "`";
            reason = "parameter '" ^ param ^ "' enters via a table index/entry; \
                      table-derivative not emitted" }
        else Known (Const 0.0)
    (* TimeFunc: look up the forcing record; if any field mentions param, refuse *)
    | TimeFunc fname ->
        if forcing_mentions param fname then
          Unsupported { node = "forcing `" ^ fname ^ "`";
            reason = "parameter '" ^ param ^ "' is a parametric forcing argument; \
                      forcing-derivative not emitted" }
        else Known (Const 0.0)
    | BinOp b -> map2_deriv b.op (differentiate b.left param)
                                 (differentiate b.right param)  (* Add/Mul/... propagate Unsupported *)
    ...

  (* differentiate_rate is now forced to choose — it CANNOT silently omit: *)
  let differentiate_rate rate params =
    List.map (fun p ->
      match simplify_deriv (differentiate rate p) with
      | Known (Const 0.0) -> Ok None                 (* genuinely absent → omit, fine *)
      | Known d           -> Ok (Some (p, d))
      | Unsupported u     -> Error (E_grad_dropped (p, u)))  (* compile-time E-code w/ hint *)

Now the *only* way to get an omitted gradient is a proven `Known (Const 0.0)`.
Every "I dropped this" is an `Unsupported` the caller must pattern-match, and
the chosen call site turns it into a hard diagnostic (name the param, the
forcing/table, and the fix) — exactly the bar in CLAUDE.md "Error messages are a
feature". Best case (the issue's intent) you differentiate THROUGH a Sinusoidal's
amplitude/baseline analytically and return Known; the ADT is the floor that
guarantees the fallback is never silent. (This pairs with #119's Rust fix:
un-freezing the forcing/table caches is necessary for the gradient to be
meaningful; this is the OCaml half that stops emitting a zero for it.)
```

**What this still doesn't catch (honest).** The ADT guarantees no SILENT zero — it does not guarantee a CORRECT non-zero derivative. (1) If you choose to emit `Unsupported` for parametric forcings rather than implementing the analytic ∂/∂amplitude etc., the param is still un-estimable — but now it's a loud compile error telling the user to reparameterize, not a fake posterior. (2) `mentions`/`forcing_mentions` is a syntactic check; it can over-refuse (param textually present but cancels out) — a false positive that errors a model that was actually fine, which is the safe direction but still a friction. (3) It does nothing for #180's gated DerivedExpr-projection chain-rule term, which lives in the Rust obs-gradient path (obs_model.rs), not this OCaml pass — same shape, different file. (4) The type does not force the differentiation RULES to be mathematically right (the quotient/power rules could be wrong and still typecheck as `Known`); only finite-difference validation tests cover that. The ADT moves "did we drop it" into the type system; "is the kept derivative correct" stays a test obligation.

---

## Dual-purpose work-list: which open issues each technique retires

These aren't hypothetical — each technique above maps to concrete open (or
just-closed-as-worked-example) issues. The M-class column is the part that doubles
as the next batch of backlog knockdown.

| technique | M-class | L-class |
| --- | --- | --- |
| ADT `ParamValue` (estimated ≠ missing) | #191 (gate half) | — |
| One `(value, grad)` traversal / AD type | — | #197, #200, #79 |
| resolve-don't-stringly-type (`CompartmentId`) | #112 (arity, done) | #111 (resolver) |
| derive-the-hash-from-the-type | #189, #190 | — (#147 done) |
| newtypes (`Time`/`Dt`/`GridDt`, `Count`, idx) | #101, #107, #177 | review #11 |
| parse-don't-validate (`Count` at the boundary) | #124 (done), #126, #127, #134 | — |
| make-the-dropped-derivative-explicit (`Deriv`) | #128 (done) | #119, #180 |
| checked casts / typed effect amounts | #198, #199, #122, #125 | — |

≈ **13 M-class + ~6 L-class** issues are type-addressable. The rest of the backlog
(~49) is genuinely isolated features / ergonomics / docs that no type collapses.

## Where types do NOT help (so skeptics trust the rest)

Static types make *illegal states unrepresentable*; they cannot tell you whether a
*representable* computation is mathematically correct. These remain pure
tests/oracles work, untouched by any amount of typing:

- **Numerical/algorithmic correctness** — the Gillespie inhomogeneous-Poisson
  sampler bias (#95), 100%-divergent NUTS on stratified models (#175), whether a
  gradient *formula* is right. A perfectly-typed `Ad` value still computes the wrong
  number if the math is wrong.
- **"Equals the true posterior"** — you can't type-check that a particle-filter
  marginal converges to ground truth (#201); that needs an analytic oracle.
- **Cross-language *value* agreement** — types/codegen can pin that both sides
  *parse the same shape*, but a finite-state oracle (the `caltime.tsv` golden) is
  what proves they compute the same *number*.

The bugs that "passed tests" this codebase actually split ~evenly: the
value/gradient divergence (#197) was **type-preventable**; "is the marginal
correct" (#201) was **not**. Types and oracles are orthogonal axes — you need both.

## The honest fraction

Of the *hard bugs* remaining (excluding features/ergonomics/docs, which aren't
defects), roughly **45–55% would have been unrepresentable or compile-caught** under
the type designs above — concentrated in IR construction/validation and the
value/gradient seam. Of the *whole* backlog it's only ~20%, because about half the
backlog isn't bugs at all. So "use more types" is **half** the answer; the other
half is "stop testing for *change*, start testing for *correctness*" (the oracle /
meta-test discipline).

## Cost, and where to deploy (this matters for the DSL)

Types aren't free: pervasive phantom-types / GADTs ossify the grammar and slow
iteration, and camdl explicitly values a **small, human-readable DSL** a
non-software-engineer epidemiologist can read at a glance. The ROI is steeply
**concentrated at the bug-dense boundaries** — deploy the techniques here and stop:

1. **The compiled IR** — parse-don't-validate: resolve references and range-check
   values *once* into a type that can only hold valid states (examples 1, 3, 6).
   This is the single biggest lever.
2. **The value/gradient seam** — one traversal / an `Ad` type so they can't diverge
   (example 2).
3. **Identity & the FFI contract** — derive the content hash from the type; codegen
   both IR sides from one schema (example 4).
4. **Scalars & indices** — cheap newtypes where confusing two is a wrong-vaccination-
   plan bug, not everywhere (example 5).

Beyond those four seams, the marginal type costs more than it saves. The point isn't
"more types" — it's *make the bug **class** unrepresentable at the seam where it
keeps biting.*
