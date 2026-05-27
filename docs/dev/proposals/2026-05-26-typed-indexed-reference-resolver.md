---
status: draft
date: 2026-05-26
title: Typed indexed-reference resolver for the OCaml frontend
author: internal (responding to 2026-05-26 upstream OCaml-compiler review)
scope: ocaml/lib/compiler/expander.ml + ocaml/lib/ir/validate.ml
closes:
  - "#111 — Indexed references lowered by string concat (upstream Critical #1)"
  - "#112 — Table lookup arity not validated (upstream Critical #2)"
  - "#114 — Stratified initial conditions not validated (upstream Critical #4)"
partial-closes:
  - upstream High #9 (`c in compartments` omitted dims)
  - upstream High #10 (bare stratified `transfer`)
  - upstream High #12 (inline table shape)
  - upstream High #13 (output projection expressions)
non-goals:
  - "#113 (block transition rate=0.0) — grammar fix, separate"
  - "#115 (scenario validation) — namespace fix, separate"
  - "#116 (likelihood dim-check) — dimcheck layer, separate"
  - "#117 (duplicate names) — namespace uniqueness, separate"
  - "#98 (calendar dates) — calendar surface, separate"
related-reviews:
  - 2026-05-26-upstream-ocaml-compiler-review.md
  - 2026-05-26-week-audit-findings.md
  - 2026-05-26-week-audit-comparison.md
---

# Typed indexed-reference resolver for the OCaml frontend

## Required-reading checkpoint (per CLAUDE.md)

Before drafting, read:
- `docs/camdl-language-spec.md` §6 (tables), §"Named and partial indexing"
  (~L670–L685, L1860–L1925), §4 (parameter kinds)
- `docs/dsl-cheatsheet.md` for the surface examples
- `ocaml/lib/compiler/lexer.mll` (index-syntax tokens)
- `ocaml/lib/compiler/parser.mly` (the `EIndex`, `IPosn`, `INamed`,
  `IBind`, `IConsec`, `IComp` productions)
- `ocaml/lib/compiler/expander.ml` (the resolver surface)
- `ocaml/lib/ir/dimcheck.ml` (so this proposal doesn't break its
  invariants)

The spec says:

> ```
> S[sex = female, age = child]        # order doesn't matter
> S[patch = p1]                       # sum over age, specific patch
> incidence(infection[patch = p])     # sum over age, specific patch
> ```
>
> "Positional and named indexing can be mixed: `S[child, sex = female]`
> is valid (first positional = age, second named = sex). But for clarity,
> use one style consistently."
> (`docs/camdl-language-spec.md:670–685`)

> "Use named indexing in any model with more than one dimension — it
> prevents the positional-binding failure mode."
> (`docs/camdl-language-spec.md:1920–1925`)

This is the contract. The current implementation does not honor it.

## Problem — the current implementation is stringly-typed and discards labels

### Receipt 1 — `index_item_to_str` drops the named-index label

```ocaml
(* ocaml/lib/compiler/expander.ml:926–931 *)
let index_item_to_str env item =
  match item with
  | IPosn (EIdent (s, _))     -> (match List.assoc_opt s env with
                                  | Some v -> v | None -> s)
  | IPosn _                   -> "?"
  | INamed (_, EIdent (s, _)) -> (match List.assoc_opt s env with
                                  | Some v -> v | None -> s)
  | INamed (_, _)             -> "?"
```

The `_` in `INamed (_, EIdent (s, _))` *is the dimension label*. The
function pattern-matches it and immediately throws it away. After this
call, `S[patch = p1]` and `S[p1]` produce the same downstream input.

### Receipt 2 — Stringly-typed lowering everywhere downstream

```ocaml
(* ocaml/lib/compiler/expander.ml:3729–3735 (resolve_comp_name) *)
| ProjDerived (EIndex (name, idxs)) ->
    let idx_vals = List.map (index_item_to_str env) idxs in
    let concrete = String.concat "_" (name :: idx_vals) in
    if Hashtbl.mem ctx.expanded_comp_tbl concrete then
      Ir.CurrentPop concrete
    else if Hashtbl.mem ctx.comp_tbl name then
      prevalence_projection name idx_vals
    else
      Ir.CumulativeFlow concrete
```

The "type" of an indexed reference is `string` — a concatenation of the
base name and whatever the user wrote, in user-supplied order. The
disambiguation between `CurrentPop` (compartment) and `CumulativeFlow`
(transition) is by hash-table membership of the *concatenated string*,
not by what the AST node actually denotes.

`incidence(infection[patch = p])` with a `[patch, age]`-stratified
`infection` produces `"infection_p"` — a name that does not exist in
either table — falling through to `Ir.CumulativeFlow "infection_p"`,
which the runtime then fails to find. The named-index semantics
"specific patch, sum over age" is not implemented anywhere.

### Receipt 3 — Table lookup uses `List.nth tdims i` with no arity check

```ocaml
(* ocaml/lib/compiler/expander.ml:1505–1525 *)
let tdims = table_dims ctx base_name in
if tdims <> [] then
  let per_dim = List.mapi (fun i item ->
    let dim      = List.nth tdims i in       (* <- iterates over items, not tdims *)
    let val_name = index_item_to_str env item in
    (int_of_float (dim_value_index ctx dim val_name),
     List.length (dim_values ctx dim))
  ) items in
  ...
  Ir.TableLookup (base_name, [Ir.Const (float_of_int linear)])
```

`List.mapi` iterates over user-supplied `items`. If `items` has fewer
entries than `tdims`, the loop terminates short and the stride math
produces a partial-prefix linear index — wrong row of a contact matrix,
no diagnostic.

### Receipt 4 — Init expansion uses the same string-mangling pattern

```ocaml
(* ocaml/lib/compiler/expander.ml:2893–2905 *)
if ie.iindices = [] then ie.icomp
else
  let idx_vals = List.map (function
    | IPosn (EIdent (s, _))     -> s
    | IPosn (EConst f)          -> string_of_float f
    | INamed (_, EIdent (s, _)) -> s   (* label discarded again *)
    | _                         -> "?"
  ) ie.iindices in
  String.concat "_" (ie.icomp :: idx_vals)
```

Bare stratified `init { S = N0 }` for stratified `S` is accepted; the
key emitted into the IR initial-conditions table is the literal string
`"S"`, which doesn't match any expanded compartment name.

### Receipt 5 — `IComp` iteration ignores partial stratification

```ocaml
(* ocaml/lib/compiler/expander.ml:2008–2014 *)
| IComp v ->
    let names = List.filter_map (fun cd ->
      match cd.ckind with
      | Integer -> Some cd.cname
      | Real    -> None
    ) ctx.comp_decls in
    if names = [] then None
    else Some (List.map (fun n -> [(v, n)]) names)
```

This substitutes only base compartment names. `resolve_stoich_ref`
downstream then concatenates whatever indices the user supplied. If
`R` has dimensions `[age, immunity]` and the model writes:

```camdl
death[c in compartments, a in age] : c[a] --> @ mu * c[a]
```

the expander yields stoich names like `"R_child"` — partial, invalid.
The omitted `immunity` dimension is silently dropped instead of being
cartesian-product expanded to `R_child_natural`, `R_child_vaccine`, ….

### Why this surface is wrong

Every stringly-typed path above shares one property: **the absence of a
type that records what an indexed reference actually denotes.** A
denotation is one of:

- a single expanded compartment cell (e.g. `S_kano_child`)
- a sum over compartment cells (e.g. `S_*_child` summed over patches)
- a single transition flow
- a sum over transition flows
- a specific table cell, identified by a row-major linear offset that
  was validated against the table's declared rank
- a parameter (scalar or expanded indexed)
- a let-binding expansion (an inlined expression with index vars bound)
- a time function (`exp`, `mod`, etc.)

The current implementation collapses all of these into `string`. Every
caller pays the cost of guessing the denotation by hash-table
membership of a guessed concatenation. The named-index *label* — the
single piece of information the user provided to make their intent
unambiguous — is dropped at line 930 and never recovered.

The bugs are not implementation errors on individual call sites. They
are *the same structural error replicated across every site that
mentions an indexed reference*. Six discrete code paths (compartment
ref, transition projection, table lookup, indexed let, indexed
parameter, transfer source/destination) each reinvent string-mangling
and each gets it slightly wrong in a different way.

## Proposal — a single typed resolver

### The ADT

```ocaml
(* ocaml/lib/compiler/resolver.ml — NEW *)

(** Which logical namespace the user's identifier inhabits. The
    resolver must be told this — the same string may live in
    multiple namespaces (compartment + parameter + let) and the
    spec's resolution order is per-namespace, not global. *)
type namespace =
  | NS_Compartment
  | NS_TransitionFlow
  | NS_Table
  | NS_Parameter
  | NS_Let
  | NS_Forcing

(** Result of resolving an indexed reference. Exhaustive — every
    legal denotation is one constructor; ambiguity or arity errors
    are not constructors, they are [Result.Error] in the resolver's
    return type. *)
type resolved_ref =
  | OnePop      of string
    (** A single fully-resolved expanded compartment cell. *)
  | PopSum      of string list
    (** A sum over expanded compartment cells (omitted dimensions
        or a bare reference to a stratified compartment). The list
        is the sum's elements; downstream lowers to [Ir.PopSum]. *)
  | OneFlow     of string
  | FlowSum     of string list
  | TableCell   of { name : string; linear_index : int; rank : int }
    (** Fully-validated table reference. [linear_index] was computed
        only after asserting [List.length items = rank], and the
        sub-indices were each validated against their declared
        dimension. *)
  | TableSum    of { name : string; bound : (dim_name * level) list;
                     summed_over : dim_name list }
    (** A partial table reference: pin some dimensions, sum over
        the others (the spec's named-index semantics applied to
        table lookups). *)
  | Param       of string
    (** Scalar parameter or fully-resolved indexed parameter. *)
  | LetExpansion of Ir.expr
    (** A let-binding inlined at this site with all index variables
        substituted. *)
  | TimeFunc    of { name : string; args : Ir.expr list }

(** What the resolver can refuse. Each constructor maps to a
    diagnostic E-code; the resolver never returns [Ok] with an
    invalid resolved_ref. *)
type resolve_error =
  | UnknownIdent       of { name : string; ns : namespace }
  | UnknownIndexLabel  of { base : string; bad_label : string;
                            known_dims : dim_name list }
  | UnknownIndexLevel  of { base : string; dim : dim_name;
                            bad_level : string; known_levels : level list }
  | ArityMismatch      of { base : string; expected : int; got : int }
  | AmbiguousNamespace of { name : string; in_ : namespace list }
  | DuplicateIndex     of { base : string; dim : dim_name }
  | MixedPositionalAfterNamed of { base : string; position : int }
    (** Defensive: [S[age = child, kano]] is rejected (named must
        precede or follow positional consistently). *)

val resolve : context -> env -> namespace -> base:string ->
  items:index_item list -> (resolved_ref, resolve_error) result
```

### How the spec's semantics fall out of the types

- **Order-independent named indexing.** The resolver builds a
  `(dim_name * level) list` from `items`, looking each label up in
  the declared dimension vector. The user's source order is lost
  after the first traversal; `S[sex = female, age = child]` and
  `S[age = child, sex = female]` produce the same map.

- **Omitted dimensions sum.** After collecting the user's bindings,
  the resolver compares them against the base's declared dimension
  vector. Bound dimensions are pinned to specific levels; unbound
  dimensions are enumerated and the result is `PopSum` /
  `FlowSum` / `TableSum`. The current "string-concat then
  hash-lookup" path becomes: "produce the cartesian product of the
  unbound dimensions × the bound levels, emit one element per
  product."

- **Arity errors are unrepresentable.** `TableCell { rank }` carries
  the rank that the linear index was computed against. The resolver
  cannot produce `TableCell` without first asserting
  `List.length pinned_items = rank`. Under-indexing and over-indexing
  are `Error ArityMismatch` before any IR is emitted.

- **Cross-namespace ambiguity is rejected.** `resolve` takes the
  namespace as a parameter, but the callers that previously did
  hash-table-pinball — "is `name` in `expanded_comp_tbl`? if not, is
  it in `comp_tbl`? if not, is it a flow?" — are replaced by **one**
  call site that passes the intended namespace. A name that is valid
  in multiple namespaces (today: `parameter N` and `let N = S+I+R`)
  becomes a `DuplicateNames` error during the new declaration-name
  pass (separate proposal, issue #117), not a silent let-wins-by-
  resolver-ordering bug.

### Refactored call sites — before / after

**Compartment projection.**

```ocaml
(* BEFORE — expander.ml:3727–3735 *)
| ProjDerived (EIndex (name, idxs)) ->
    let idx_vals = List.map (index_item_to_str env) idxs in
    let concrete = String.concat "_" (name :: idx_vals) in
    if Hashtbl.mem ctx.expanded_comp_tbl concrete then
      Ir.CurrentPop concrete
    else if Hashtbl.mem ctx.comp_tbl name then
      prevalence_projection name idx_vals
    else
      Ir.CumulativeFlow concrete

(* AFTER *)
| ProjDerived (EIndex (name, idxs)) ->
    match Resolver.resolve ctx env NS_Compartment ~base:name ~items:idxs with
    | Ok (OnePop n)        -> Ir.CurrentPop n
    | Ok (PopSum names)    -> prevalence_projection_of_names names
    | Ok _ | Error _ as e  -> Diagnostics.fail_resolve ctx.diags e
```

The `Ir.CumulativeFlow` fallback is gone — that was the bug where a
miss in the compartment namespace silently re-resolved as a flow.
Instead the caller declares its namespace and the resolver returns an
error if `name` is not a compartment.

**Table lookup.**

```ocaml
(* BEFORE — expander.ml:1505–1525, no arity check *)
let tdims = table_dims ctx base_name in
if tdims <> [] then
  let per_dim = List.mapi (fun i item ->
    let dim = List.nth tdims i in   (* under-indexes silently *)
    ...
  ) items in
  ...
  Ir.TableLookup (base_name, [Ir.Const (float_of_int linear)])

(* AFTER *)
match Resolver.resolve ctx env NS_Table ~base:base_name ~items with
| Ok (TableCell { name; linear_index; _ }) ->
    Ir.TableLookup (name, [Ir.Const (float_of_int linear_index)])
| Ok (TableSum { name; bound; summed_over }) ->
    (* Spec-named-indexing on tables: sum over omitted dims. *)
    expand_table_sum ctx name bound summed_over
| Ok _ | Error _ as e -> Diagnostics.fail_resolve ctx.diags e
```

`C_age[child]` against a `age × age` table is now
`Error ArityMismatch { base="C_age"; expected=2; got=1 }`. The
diagnostic names the table, the rank, and points at the index list.

**Bare stratified transfer (`upstream High #10`).**

```ocaml
(* BEFORE — expander.ml:2990–3017, resolve_comp_name requires Ir.Pop *)
let resolve_comp_name ctx env e =
  match resolve_expr ctx env e with
  | Ir.Pop n -> n
  | Ir.PopSum _ -> Diagnostics.fail "transfer needs a single compartment"
  | _ -> Diagnostics.fail "..."

(* AFTER *)
let resolve_transfer_endpoint ctx env e =
  match e with
  | EIdent (name, _) ->
      Resolver.resolve ctx env NS_Compartment ~base:name ~items:[]
  | EIndex (name, idxs) ->
      Resolver.resolve ctx env NS_Compartment ~base:name ~items:idxs
  | _ -> Error (...)
(* Then in the transfer expander: *)
match (resolve_transfer_endpoint ctx env from_e,
       resolve_transfer_endpoint ctx env to_e) with
| Ok (OnePop f, Ok (OnePop t)) ->
    [ build_one_transfer f t ]
| Ok (PopSum fs, Ok (PopSum ts)) when same_dim_signature fs ts ->
    List.map2 build_one_transfer fs ts
| Ok (PopSum _, Ok (PopSum _)) ->
    Diagnostics.fail
      "transfer source and destination have mismatched dimensions; ..."
| ...
```

Bare stratified `transfer(from = S, to = V)` is now legal — both
endpoints resolve to `PopSum`, and the expander zips them. Mismatched
dimensions are a hard error with hint text, not a `failwith`.

**`c in compartments` with partial stratification (`upstream High #9`).**

```ocaml
(* BEFORE — expander.ml:2008–2014, IComp substitutes only base names *)
| IComp v ->
    let names = List.filter_map (fun cd -> ... cd.cname) ctx.comp_decls in
    Some (List.map (fun n -> [(v, n)]) names)

(* AFTER *)
| IComp v ->
    (* Iterate over base names AND, for each, over the cartesian
       product of the compartment's unbound dimensions. The
       resolver consumes the (base, partial_binding) tuple. *)
    List.concat_map (fun cd ->
      let unbound_dims = dims_not_bound_by_outer_indices cd ibs in
      List.map (fun env_extension ->
        [(v, cd.cname)] @ env_extension
      ) (cartesian_of_dims ctx unbound_dims)
    ) ctx.comp_decls |> Some
```

The "model emits `death_R_child` instead of
`death_R_child_natural + death_R_child_vaccine`" bug becomes a
non-bug by construction.

### What the resolver does *not* do

This proposal intentionally does not touch:

- **The grammar** (`parser.mly`). The block-transition `rate = ref
  (EConst 0.0)` default (`#113`) is a separate AST/grammar fix.
- **Dimchecking** (`dimcheck.ml`). The blanket
  `permissive_dim <- true` in the observation pass (`#116`) is a
  separate layer.
- **Scenario / intervention validation** (`#115`). Scenario fields
  inhabit a different namespace and need closed-grammar typing.
- **Declaration-name uniqueness** (`#117`). The
  `Hashtbl.replace` problem is solved by a separate pre-resolver
  pass; the resolver depends on uniqueness as a precondition.
- **Calendar date validation** (`#98`). Different surface entirely.

Sequencing these together would be a single mega-commit. They are
independent enough to land sequentially; this proposal lands first
because the resolver is a prerequisite for `#114` (stratified-init
validation) and `#113`/`#117` are independent of it.

## Migration plan

### Phase 1 — Introduce the resolver, do not use it yet

1. Create `ocaml/lib/compiler/resolver.ml` and
   `ocaml/lib/compiler/resolver.mli` exporting the ADT and the
   single `resolve` entry point.
2. Write the resolver against the current `ctx` shape (no changes
   to `context`, `env`, or AST node types yet).
3. Comprehensive unit tests in `ocaml/test/test_resolver.ml`:
   - every constructor of `resolved_ref` exercised at least once
   - every constructor of `resolve_error` exercised at least once
   - parity check: for the *intersection* of inputs the current
     code handles correctly (single positional index, fully-indexed
     stratified compartment), the resolver returns the same
     denotation. Run side-by-side on every existing golden's index
     references.

No expander call site changes in Phase 1. The resolver is dead code
that the test suite exercises.

### Phase 2 — Migrate call sites one namespace at a time

Migration order (smallest blast-radius first):

1. **Table lookup** (closes `#112`). One call site
   (`expander.ml:1503-1525`). Adds arity errors. Update goldens that
   may have been emitting wrong-arity lookups silently — if any do,
   they were already broken and the golden update is the fix.
2. **Init expansion** (closes `#114`). Two call sites in
   `expand_init`. Adds the "bare stratified compartment is a hard
   error" rule. Update goldens that wrote `init { S = … }` with
   stratified `S` — none should exist if the spec was honored, but
   verify.
3. **Stoich refs** (closes `#111`, partly closes upstream #9). The
   biggest lift — every transition source and destination resolves
   through the new path. Drives the named-indexing semantics.
4. **Projection refs** (incidence, prevalence, cumulative) —
   completes `#111` for the observation surface.
5. **Transfer endpoints** (closes upstream #10). Enables bare
   stratified transfer.

After each phase, `make test-unit` + `make test-golden` must be
green. If a golden changes, that's a normative change — read the
diff before regenerating.

### Phase 3 — Validate that the old string-mangling paths are gone

Run:

```
rg "String.concat \"_\"" ocaml/lib/compiler/expander.ml
rg "index_item_to_str" ocaml/lib/compiler/expander.ml
```

Each surviving hit must have a justification in a code comment
(rare — e.g. emitting the *final* IR name for a fully-resolved
single-cell reference is still string-concat, and that's fine).

### Phase 4 — Negative golden tests

Per CLAUDE.md "Tests, but actually" and upstream finding #16, add
the following fixtures to `ocaml/golden/negative/` and assert each
produces the *expected* `Error` constructor:

- `block_transition_missing_rate.camdl` (covers `#113`, not closed
  by this proposal but the negative fixture is in scope)
- `table_under_indexed.camdl` (covers `#112`)
- `init_bare_stratified.camdl` (covers `#114`)
- `partial_compartment_iteration.camdl` (covers upstream #9)
- `bare_stratified_transfer.camdl` (covers upstream #10)
- `named_index_unknown_label.camdl` (`S[notadim = p]`)
- `named_index_unknown_level.camdl` (`S[patch = notalevel]`)
- `mixed_positional_after_named.camdl` (`S[age = child, kano]`)

Plus one positive golden:

- `named_indexing_basic.ir.json` (`S[sex = female, age = child]`
  resolves identically to `S[age = child, sex = female]`)
- `omitted_dim_sums.ir.json` (`incidence(infection[patch = p])`
  emits a `FlowSum` over age strata of `infection_p_*`)

## Issues closed by this proposal

### Fully closed
- **#111** (upstream Critical #1) — indexed references via string
  concat
- **#112** (upstream Critical #2) — table lookup arity
- **#114** (upstream Critical #4) — stratified init not validated

### Partially closed (the resolver enables; some additional work may
be needed at the consuming layer)
- Upstream High #9 (`c in compartments` omitted dims) — closed once
  Phase 2 step 3 lands the new `IComp` semantics
- Upstream High #10 (bare stratified `transfer`) — closed in Phase
  2 step 5
- Upstream High #12 (inline table shape validation) — the resolver
  validates *lookup* arity; declaration-time *inline-shape*
  validation is a separate small pass that uses the same dimension
  metadata (one helper, two callers)
- Upstream High #13 (output projection expressions discarded) — the
  resolver makes the projection legible; whether the IR carries
  these expressions is a separate IR-extension proposal

### Not closed (separately tracked)
- **#113** (block transition `rate=EConst 0.0`) — grammar fix
- **#115** (scenario validation) — different namespace
- **#116** (likelihood dim-check blanket-permissive) — dimcheck layer
- **#117** (duplicate names silently overwritten) — namespace
  uniqueness pre-pass; this proposal *depends on* uniqueness as a
  precondition
- **#98** (calendar date validation) — different surface
- Internal C1, C2, C3, C5 (inference-side findings) — Rust runtime,
  out of scope

## Risks and mitigations

- **Risk: golden churn.** Every existing model with an indexed
  reference re-emits through the new path. If the old path silently
  produced wrong but *consistent* output, goldens will diff.
  **Mitigation:** Phase 1's parity tests catch this; review each
  diff before regenerating. A diff is normative.
- **Risk: PopSum/FlowSum expansion blows up IR size.** Today, a
  silently-collapsed partial reference produces one entry; the new
  resolver produces N entries (one per omitted-dimension level).
  **Mitigation:** the spec already implies this — it's "fixing the
  bug" not "introducing inefficiency." If size becomes a real
  problem, add a downstream IR optimizer pass; the frontend stays
  honest.
- **Risk: the new error constructors produce diagnostics that don't
  match the existing E-code numbering scheme.**
  **Mitigation:** allocate a new E-code range (E270–E279 reserved
  for the resolver) and document it in
  `docs/dev/warning-catalog.md`.
- **Risk: subtle behavior changes break user models that compile
  today.**
  **Mitigation:** the only models that "compile today" through the
  buggy paths produce *wrong output*, which the user has no way to
  detect. Migration must be loud — print a one-line warning for
  every model that compiled before and now errors, citing this
  proposal. After one release cycle, the warning becomes a hard
  error.

## Test plan

Per CLAUDE.md "Fix bugs via TDD: red → green → refactor":

1. Write the negative goldens listed in Phase 4 *first*.
2. Run them against current `main`; confirm every one *fails to
   error*. That is the red.
3. Implement the resolver and migrate call sites.
4. Re-run the negative goldens; every one now produces the expected
   `Error` constructor.
5. Run the full positive golden suite; diffs must be reviewed
   normatively.
6. Mutation test: pick one `Result.Ok` constructor in the resolver,
   flip it to a sibling, verify a specific positive golden test
   fails. If no positive test catches the flip, the positive test
   suite is incomplete.

## Open questions for review

- **Q1.** Should `LetExpansion` carry the index environment used to
  substitute, or should substitution happen inside the resolver
  before returning? The resolver returning a fully-inlined `Ir.expr`
  hides one source of bugs; passing the env back gives the caller
  flexibility for an unusual semantics-preserving rewrite.
  **Recommendation:** fully inline. The resolver is the place
  that knows the binding context.
- **Q2.** Should `TableSum` be allowed at all, or should the spec be
  read as "table cells must be fully indexed"? Today the compiler
  has no table-sum semantics; the spec is ambiguous. Defer until
  the user-features doc is amended.
- **Q3.** Should the resolver be invoked during dimchecking, or
  only during expansion? Today dimchecking operates on the AST.
  Threading the resolver in would let dimcheck see `PopSum` vs
  `OnePop` and tighten some inferences. Defer — separate proposal.
