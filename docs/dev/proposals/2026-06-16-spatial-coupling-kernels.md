# Fittable spatial coupling kernels via restricted reductions

Status: proposed Issue: gh#185 IR impact: none — lowers to the existing `Reduce`
node; no `ir/schema.json` or `ir/VERSION` change. Autodiff already handles every
operator the kernel needs (the full power rule incl. variable exponent is in
`autodiff.ml:240-246`).

## The object: a coupling matrix bundles two separable things

A metapopulation force of infection `λ_p = Σ_q W[p,q] · (I_q/N_q)` hides two
logically distinct pieces of information inside one matrix `W`:

- **Support** — _which_ pairs `(p,q)` couple at all (`W[p,q] ≠ 0`): the
  adjacency / neighbour structure. It is geographic and **fixed**: which patches
  are neighbours does not change when you re-estimate the model.
- **Weight** — _how strongly_ a supported pair couples: the magnitude
  `W[p,q] = θ·N_p^{τ₁}N_q^{τ₂}/d_{pq}^{ρ}` (gravity), the radiation flux, etc.
  It is a **function of kernel parameters** `(θ, ρ, τ)`.

In a baked constant `W` table these are fused into one number per cell — either
`0` (no support) or a frozen weight. That fusion is the source of both problems
this proposal addresses.

## Two problems, one cause

**Problem 1 (gh#185, Daniel Klein, P=244 Sokoto cVDPV2): the slow form is the
natural one.** Indexing a transition by two levels of one dimension generates
one transition _per pair_ —

```camdl
imp[p,q] : S[p] --> I[p] @ kappa * w[p,q] * I[q]/N[q]  where p != q
```

— emitting P²−P transitions, each with its own stoichiometry and its own flow
accumulator (the `flow_*` blow-up). `where` is a compile-time filter on _which
index combinations become transitions_ (`expander.ml` `eval_guard`,
`expand_transitions_counted`); it prunes the diagonal but cannot collapse pairs
into a single summed rate. The fast form is one transition per `p` with a summed
rate; the on-by-default constant-fold then drops the zero-`w` terms, giving
O(P·k). It works, but nothing flags the slow form, and the author must
hand-write the foldable shape.

**Problem 2 (the deeper one): you can have sparse OR fittable, never both.** The
fold makes the sum O(P·k) by resolving each `w[p,q]` lookup to its literal and
dropping the `0`s — which requires the cells to be compile-time **constants**.
But fitting the kernel (estimating `ρ`, `θ`/`G`) requires `W[p,q]` to be a
**runtime expression of the parameters**, recomputed at each proposal so the
likelihood gradient flows through it. The moment a cell is `θ·dist^{-ρ}` instead
of a literal, no cell is statically `0`, the fold prunes nothing, and you are
back to dense O(P²) per likelihood evaluation. At national scale (P≈774) that is
the difference between a fit taking days and being intractable.

Both problems are the support/weight fusion. The fix is to **separate them**:
carve the support with a construct that is decidable at compile time (so it can
prune), and express the weight as an ordinary rate expression (so it can be
fitted).

## The construct: `sum(v in d where P, body)`

Add an optional compile-time `where` predicate to the `sum` reduction. The
expander iterates `v` over `d`, evaluates `P` per level, and emits a `Reduce`
term only for surviving levels — O(P·k) **by construction**, independent of the
fold.

```camdl
infection[p in patch] : S[p] --> I[p]
  @ beta * S[p] * ( I[p]/N[p]
      + G * sum(q in patch where dist[p,q] < 50,   dist[p,q]^(-rho) * I[q]/N[q]) )
                              └──── support (constant) ────┘ └──── weight (fitted) ────┘
```

- `where dist[p,q] < 50` — the **support**, carved from the constant `dist`
  table and a literal radius. Compile-time-decidable, so the sum is pruned to
  p's in-radius neighbours at expansion → O(P·k) with no fold reliance and no
  separate adjacency table (one `dist` table serves both the predicate and the
  body).
- `dist[p,q]^(-rho) · I[q]/N[q]` — the **weight**, a live rate expression:
  `dist` constant, `G`/`rho` fitted parameters. Autodiff differentiates it like
  any rate (the full power rule incl. the variable-exponent `ln(dist)` term is
  already implemented, `autodiff.ml:240-246`), so `G` (the coupling-strength
  estimand) and `rho` (distance decay) are **fittable**.

This is the single feature that gives sparse _and_ fittable at once — neither
the dense-sum-plus-fold path (frozen weights) nor a baked constant `W` (no
gradient) can.

### Three coupling regimes, unified

| Regime                       | How                                         | Cost                   | Fittable kernel? |
| ---------------------------- | ------------------------------------------- | ---------------------- | ---------------- |
| Forward, fixed kernel        | baked constant `w : p×p` table, dense `sum` | O(P·k) via fold        | no               |
| Forward, sparse              | `sum(q where dist<r, w[p,q]·…)`             | O(P·k) by construction | no               |
| **Inference, fitted kernel** | `sum(q where dist<r, kernel(dist;θ)·…)`     | O(P·k) by construction | **yes**          |

The third row is new and is the state-of-the-art spatial-inference target
(spatPomp's `G`; Xia et al. 2004 estimated `ρ`, `τ` for measles).

### Support forms

- **Radius (the clean default):** `where dist[p,q] < 50`. One table, literal
  radius, reads like the biology. (Distance is dimensionless — see Dimensional
  checking; `50` is a bare literal until a length dimension exists.)
- **Arbitrary adjacency:** `where mask[p,q] != 0`, where `mask` is a precomputed
  0/1 support table (top-k neighbours, empirical mobility edges). Use when the
  support isn't a simple distance threshold.
- **Self-term:** `where q != p` (index comparison; combinable:
  `where dist[p,q] < 50 and q != p`).

## Semantics

### Decidability (per-cell, hard constraint)

`P` must be decidable before simulation. It may reference: index/loop variables,
dimension levels, **compile-time-constant table cells**, and numeric/unit
literals. It may NOT reference parameters, compartment state (`Pop`),
`Time`/`Dt`.

The constant-cell rule is **per cell over the iterated index range**, not a
table-level property. A §6.6 table mixes `Const` and `Param` cells
(`[[0.0, beta_mf],[beta_fm,0.0]]`); a predicate is decidable exactly for the
`(p,q)` selecting `Const` cells. The check evaluates the predicate's index range
during expansion and errors on the first non-`Const` cell the range hits — it
does _not_ reuse the constant-fold's coarser table-level all-`Const` gate
(`constant_fold.ml:42-46`).

A parameter threshold (`where dist[p,q] < sparse_thresh`, `sparse_thresh` a
fitted param) is therefore rejected — and could not be sparse anyway, since the
survivor set would change with the parameter at runtime (a variable-length
reduction the bounded-time IR forbids). The truncation radius is a literal: it
is a computational truncation (kernel tails are negligible), not an estimand.

### Dimensional checking

Relational comparisons in `P` are dimension-checked like rate expressions: both
sides must share a dimension. camdl today has only two base dimensions, P
(population) and T (time) — there is **no length axis**, and `'km` does not lex
(`dimcheck.ml:3`; the unit-literal list is fixed). So a distance table is
**dimensionless** (`'ratio` or unitless), the radius is a bare literal (`< 50`),
and the kernel `dist[p,q]^(-rho)` type-checks precisely because the base is
dimensionless — `dimcheck.ml:394` rejects a non-constant exponent over a
_dimensioned_ base, so a dimensionless distance is exactly what makes a fitted
exponent legal (no `unchecked_dim` needed).

A length base dimension (`[P,T]` → `[P,T,L]`, plus `'km`/`'m` literals +
conversions) would let the predicate read `< 50 'km` and catch
distance-vs-time-vs-rate mistakes — a worthwhile but separable enhancement,
tracked separately, not required here.

### Pipeline ordering (resolved)

`expand_transitions_counted ctx` (`expander.ml:5859`) runs before
`expand_tables ctx` (`5886`), and the predicate is evaluated in the `ESum` arm
during transition expansion (`expander.ml:2053`) — so resolved table _values_ do
not yet exist there. Resolution: the predicate evaluator resolves only the
table(s) it references, on demand, from the table declarations in `ctx`,
**memoized per table** (a `read()` table does file I/O, and the predicate is
evaluated O(P) times per `p` → O(P²) total at expansion; without memoization a
`read()` would be re-parsed each time). No pipeline reorder.

### Lowering

`sum(v in d where P, body)` → `Reduce [body{v:=lvl} | lvl ∈ d, P(lvl)]` — the
existing `Reduce` node with surviving terms only. Empty survivor set →
`Reduce []` → `Const 0.0` (the `ESum` arm already returns `Const 0.0` for an
empty domain, `expander.ml:2055`; dimchecks as `Any`). Singleton → the bare body
(no degenerate `Reduce`). Autodiff/dimcheck/eval operate on the lowered `Reduce`
unchanged; the Rust runtime already evaluates `Reduce` across all backends and
inference methods (`compiled_model.rs`, `flat_eval.rs`, `pgas.rs`), so there is
no backend×method matrix gap.

### Bound-variable shadowing

`sum`'s bound variable colliding with an enclosing transition index variable
(e.g. `sum(p in patch …)` inside `infection[p in patch]`) must be a compile
error. This is a **pre-existing gap, not introduced by `where`**: it already
applies to plain `sum` today — `check_shadowing` (`expander.ml:4886`) covers
only let-names vs strata, and `ESum` resolution prepends to the env with
first-match-wins (`expander.ml:2057`), so `sum(p in patch, S[p])` inside
`infection[p in patch]` silently rebinds `p` to the inner binder (the body reads
the sum variable, iterating all patches, not the transition's stratum) — a
silent-wrong result with no diagnostic. The right fix is uniform: a single
"binder must not shadow an enclosing index/bound variable" check applied to all
index binders (transition indices, `sum`, indexed `let`, event/intervention
indices), landed as a companion to this work (the unify change already touches
the env/binder machinery). See the "wider scoping hygiene" note in the
implementation plan.

## Grammar

Extend the predicate (a restricted nonterminal `pred`, NOT `expr` — reusing
`expr` reintroduces a greedy-consume ambiguity since `w[p,q]!=0` is itself a
valid `expr`), and add an optional `where pred` to the `sum` rule:

```
sum_expr := SUM '(' v IN d ('where' pred)? ',' body ')'
pred     := atom | pred 'and' pred | pred 'or' pred | '(' pred ')'
atom     := ivar  ('=='|'!=')  ivar              # index comparison (existing guard)
          | tref  relop  lit                     # constant-table predicate (new)
          | lit   relop  tref
tref     := IDENT '[' idx (',' idx)* ']'         # lookup into a constant table
relop    := '==' | '!=' | '<' | '<=' | '>' | '>='
```

The relational tokens (`LT GT LE GE EQ2 NEQ`) already exist and are `%nonassoc`.
A menhir prototype of this grammar (unify path: `GTab` atom + optional sum
`where`) produced **no new conflict** vs the current grammar (byte-identical
`--explain` report, the pre-existing transition-`{` shift/reduce only). The
predicate ends at `COMMA`; the body is a separate `expr`; `WHERE` is a distinct
keyword — no predicate/body ambiguity.

### Unify the predicate language

The richer predicate becomes the single `guard` type used by both transition
`where` and sum `where` (the natural seam — one predicate language to hold in
the head). Mechanical cost: a `GTab` constructor breaks four exhaustive matches
(`eval_guard`, `check_guard_compile_time`, `guard_to_string`, `inspect.ml`), and
`eval_guard` gains `ctx` + memoized table values + dim sizes (for row-major
indexing) in its signature. A table-valued predicate on a _transition_ is legal
but the wrong tool for coupling (still per-pair transitions); the warning below
steers coupling to the `sum` form.

## The O(P²) warning

When a transition is indexed by two levels of the same dimension and the second
index appears **only in the rate**, not the source/destination stoichiometry
(the P² transitions share per-`p` stoichiometry), emit a warning with a real
catalog code:

> transition `imp` is indexed by `[p, q]` over `patch` but `q` is absent from
> its stoichiometry — this generates P²−P transitions and as many flow columns.
> Write one transition per `p` with a summed rate:
> `… @ … * sum(q in patch where dist[p,q] < r, …)`. See spec §8.

Warning, not error (per-pair flows are legal). New entry in
`docs/dev/warning-catalog.md`.

## Errors (error-quality bar)

- Predicate references a parameter / compartment / `Time` / a non-`Const` cell
  its index range hits →
  `error: the where-predicate in sum(...) must be
  decidable at compile time; it may reference index variables, constant tables,
  and literals, not '<name>' (<kind>). Move it into the rate body.`
- The common mistake — a **fitted threshold**
  (`where dist[p,q] < sparse_thresh`, `sparse_thresh` a parameter) — gets a
  tailored message rather than the generic one above:
  `error[E2xx]: the where-predicate compares against parameter 'sparse_thresh',
  but a coupling support must be fixed at compile time — a fitted radius would
  change which patches couple at runtime (an unbounded reduction the engine
  cannot evaluate). Use a literal radius (e.g. < 50) for the support and fit the
  kernel's shape/strength in the rate body instead (the radius is a truncation,
  not an estimand).`
- Dimension mismatch in a predicate comparison → standard dimension diagnostic
  naming both sides' dimensions.
- `sum`/`where` bound variable shadows an enclosing index variable → shadowing
  error naming both binders.
- A table appears in the predicate but not the body (the common
  `where w[p,q]!=0` but forgot-to-multiply-by-`w` error) → a hint, not an error.

## Spec changes (drafted; land with the implementation)

§8 (Indexed Let vs Sum) gains a "Restricted sums" subsection: the `where`
predicate semantics (compile-time, constant tables only, dimension-checked), the
radius and mask forms, and the support/weight separation framing — with the
fittable-kernel example as the worked case. Cross-reference from §9 (prefer the
summed-rate form to per-pair transitions for coupling) and §6 (constant-cell
requirement). Note the kernel's dimensional treatment: `dist^{-ρ}` for a fitted
`ρ` is legal **only** because distance is dimensionless (camdl has no length
dimension; `dimcheck.ml:394` rejects a non-constant exponent over a dimensioned
base). A future length dimension would require nondimensionalizing
(`(dist/d0)^{-ρ}`).

Fix §9.7 line 1763 ("use `a < b` in `where` guards") — currently false, made
true by this change; reword to match the actual predicate grammar.

## Test plan (TDD: red → green)

- Parser: `sum(q in d where dist[p,q] < 50 'km, …)`,
  `… where mask[p,q] != 0 and q != p, …` parse to the new AST.
- Expander term-count with the **constant-fold OFF**: a radius/mask predicate
  yields a `Reduce` of exactly the k survivors (not P) — proves sparsity is from
  `where`, not the fold.
- Decidability error: predicate over a parameter / compartment / parameterized
  §6.6 cell its range hits → the diagnostic, asserting it names the
  offender+kind.
- Dimension error: a predicate comparing mismatched dimensions (e.g. a
  dimensionless `dist` cell against a `rate`- or `count`-kinded table cell) →
  dimension diagnostic.
- Empty survivor set → `Const 0.0`; singleton → bare body.
- Equivalence: `sum(q where w[p,q]!=0, body)` (fold off) is byte-identical in
  simulation to `sum(q, w[p,q]*body)` (fold on) on a sparse model — ties the new
  construct to the already-gated fold result (`gate_constant_fold_ab.rs`).
- **Inference (the new capability):** PGAS/NUTS gradient through a
  where-restricted `Reduce` whose body carries a parameter (fit `G`; fit `rho`
  in `dist^{-rho}`) — a path the forward-only fold gate never exercised. Assert
  finite gradients and recovery on synthetic data.
- Shadowing: `sum(p in patch …)` inside `infection[p in patch]` → shadowing
  error.
- O(P²) warning fires on `imp[p,q]` with `q` absent from stoichiometry; does not
  fire on the summed-rate form or a genuine `[p,q]`-stoichiometry transition.
- Goldens (a small kernel-feature set, each with fixture data + reviewed IR and
  trajectory baseline):
  - `sir_spatial_where_radius` — radius support `where dist[p,q] < r`, fixed
    kernel, forward sim; proves sparse-by-construction with the fold OFF.
  - `sir_spatial_where_mask` — `where mask[p,q] != 0` over a precomputed 0/1
    adjacency table.
  - `sir_gravity_fitted` — fitted kernel (`G`, and `rho` in `dist^(-rho)`) over
    a radius support; the inference fixture (first-scenario param values + a
    small synthetic obs series for a fit smoke test). Shared fixture data:
    `data/spatial_dist.tsv` (sparse `src,dst,dist`, `read(..., default = 1e9)`),
    `data/spatial_pop.tsv` (`patch,pop`).

## Reproduction (measured, this design)

The fold collapse the construct must match, on a `read()`-loaded sparse 4-patch
chain (`w : patch × patch = read(..., default = 0.0)`, guarded FOI sum): fold
OFF → ON, `table_lookup` 96→0, coupling `cond` guards 144→36, IR 62 229→17 079
B. Repro: compile the model both ways (`CAMDL_NO_CONSTANT_FOLD=1` vs default)
and diff node counts / size.

## Non-goals

- **Building W** (gravity/radiation kernels, shapefile / mobility / population
  ingestion) — sibling follow-up. Building the geographic tables belongs in
  preprocessing (PySAL/`sf`/`exactextractr`); camdl ingests the long-format
  result via `read()` and expresses the kernel from it.
- **Runtime/state-dependent predicates** — forbidden by decidability.
- **Inferring the support itself** (which pairs couple) — a discrete
  model-selection problem; the support is fixed.
- **Ragged/neighbour-set index** `sum(q in neighbors[p], …)` — a larger
  construct; `where` over a constant table is the inline-predicate form and
  covers the common case (radius / precomputed adjacency).
