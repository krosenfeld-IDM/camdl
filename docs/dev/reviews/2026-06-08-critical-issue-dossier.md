# Critical-issue dossier — collision-free Criticals (2026-06-08)

Code-level briefing produced by a read-only subagent fan-out (one agent per
issue, each verified the defect against current `main`). Agent-produced —
spot-check the cited file:line before acting; the TDD red test is the real
verification.

All 10 confirmed live: [95, 97, 98, 111, 112, 114, 117, 119, 129, 147]. Stale:
none.

---

## #97 — profile-PMMH mle.toml: reported loglik may not match saved MLE params (same class as f52d1ecd)

**Effort:** S | **Confirmed live:** True

**What the code does today**

Defect is LIVE on main (HEAD cb5fdd0f). Issue cites profile.rs:1389-1406 — those
line numbers are stale; the real code is at
rust/crates/cli/src/profile.rs:1447-1481, inside the `ProfileAlgo::Pmmh` match
arm.

profile.rs:1447-1462:

```
let best_ll = result.steps.iter()
    .map(|s| s.log_likelihood)
    .fold(f64::NEG_INFINITY, f64::max);
// Report the MAP point's loglik for the rollup if it
// dominates the per-sample max ...
let final_ll = if result.map_loglik.is_finite() {
    result.map_loglik.max(best_ll)
} else if best_ll.is_finite() {
    best_ll
} else {
    f64::NEG_INFINITY
};
```

The arm returns the tuple `(final_ll, final_lp, result.map_params)` at
profile.rs:1481.

That tuple is bound at profile.rs:1204 as
`(final_loglik, final_log_posterior, mle_params)` and passed to render_mle_toml
at profile.rs:1586:
`render_mle_toml(..., &mle_params, final_loglik, final_log_posterior, &diag)`.

render_mle_toml (profile.rs:1747-1781) writes `final_loglik` as the labeled
loglik (line 1758:
`body.push_str(&format!("final_loglik = {}\n", final_loglik));`) and the `[mle]`
block from `mle` (= `result.map_params`, lines 1774-1777). So mle.toml's
`final_loglik` and `[mle]` params come from two independent sources.

PMMH engine couples map_params/map_loglik/map_log_posterior as a triple,
selected by POSTERIOR (pmmh.rs:478-483):

```
let log_posterior = current_ll + current_log_prior;
if log_posterior > map_log_posterior {
    map_log_posterior = log_posterior;
    map_params.copy_from_slice(&current_params);
    map_loglik = current_ll;          // PF loglik AT map_params
}
```

`best_ll` instead = max over all recorded steps of `s.log_likelihood` (the PF
loglik at whatever θ that step sat at). Under a non-flat prior the
argmax-posterior step ≠ argmax-likelihood step, so best_ll can exceed
map_loglik, and that best_ll came from a step whose params are NOT
result.map_params.

Contrast: the IF2 arm was fixed by f52d1ecd and now returns
`(true_ll, f64::NAN, r.mle)` where true_ll is a fresh clean bootstrap_filter
re-pass AT r.mle (profile.rs:1254-1257, 1295) — loglik and params coherent by
construction. The PMMH arm never got that coherence fix.

Priors are non-flat on this path routinely: profile.rs:1326-1331 resolves priors
via resolve_priors_with_precedence (--fit toml > model IR `~` syntax > Flat),
and gh#73 made model-IR `~` priors flow here. The arm's own comment
(profile.rs:1308-1312) describes map_params as "the highest-posterior sample" —
exactly the distinction that breaks the loglik label.

**Defect**

mle.toml's `final_loglik` field (labeled as the loglik at the saved MLE) is
computed as `result.map_loglik.max(best_ll)`. When `best_ll > result.map_loglik`
— which is legal whenever the prior is non-flat, because the maximum-likelihood
chain step is not the maximum-posterior (MAP) step — the reported loglik is the
PF loglik of a DIFFERENT chain step than the one whose θ was written into the
`[mle]` block (`result.map_params`). The file therefore pairs a loglik from θ_A
with parameters θ_B. The `.max(best_ll)` clause is the bug; under a flat prior
map_loglik==best_ll always (MAP==MLE) and the clause is a no-op, which is why it
survived until gh#73 made non-flat priors reachable.

**Trigger / repro**

Run a profile with the PMMH algorithm on a model carrying an informative `~`
prior in the IR (or supplied via --fit toml), e.g.
`camdl profile run model.camdl --algo pmmh --focal R0 --grid ...` where a
nuisance param such as `rho ~ beta(...)` or `phi ~ lognormal(...)` is sharply
informative and pulls the MAP θ away from the likelihood ridge. The per-cell
mle.toml then shows a `final_loglik` that does not reproduce when you
`camdl pfilter` the saved `[mle]` params. SILENT wrong number — no error is
raised; completed=final_ll.is_finite() stays true, the cell looks healthy, and
the profile-likelihood contour plotted from these final_loglik values is built
from mismatched loglik/θ pairs. A user only notices if they independently
pfilter the saved params and compare (the test the issue asks for).

**Blast radius**

Scientific output affected: the per-cell profile-likelihood (and
profile-posterior) surface produced by `camdl profile run --algo pmmh`. Each
cell's reported loglik is biased UPWARD by (best_ll - map_loglik) ≥ 0 whenever
the prior pulls MAP off the likelihood ridge; the bias is heterogeneous across
cells (depends on how far each cell's posterior peak sits from its likelihood
peak), so it distorts the SHAPE of the profile curve, not just an additive
constant. That moves the apparent MLE location, the curvature-based
confidence-interval width (the Δ-loglik=1.92 crossing), and any
model-comparison/likelihood-ratio read off the surface. Model class: any model
fit on the PMMH profile path with a non-flat prior — i.e. exactly the production
Bayesian-profile use after gh#73. cVDPV2 polio fits that use informative priors
on reporting/observation params are in scope. The map_log_posterior column
(final_lp) is NOT affected by this clause; only the loglik/MLE-param coherence
is.

**Fix shape**

Single-arm change in rust/crates/cli/src/profile.rs, ProfileAlgo::Pmmh branch
(the final_ll computation at lines 1447-1462). Mirror f52d1ecd's intent: drop
the `.max(best_ll)` clause and the `best_ll` fallback so final_ll ==
result.map_loglik (coherent with result.map_params by construction,
pmmh.rs:478-483), writing f64::NEG_INFINITY honestly when map_loglik is
non-finite. Minimal form:
`let final_ll = if result.map_loglik.is_finite() { result.map_loglik } else { f64::NEG_INFINITY };`.
The `best_ll` binding (1447-1449) becomes dead and should be deleted
(delete-on-sight). Then add the integration test the issue specifies. STRONGER
option (truer to f52d1ecd, recommended — see risk): do a fresh clean
bootstrap_filter re-pass at result.map_params, exactly as the IF2 arm does at
profile.rs:1254-1257 (pf_process/pf_obs_model/smc_cfg are already in scope at
1348-1360), capped at min(n_particles,500), and report that. This is a localized
fix, no design proposal needed.

**Risk**

Subtlety the issue's prescribed fix glosses: `result.map_loglik` is the IN-CHAIN
PF loglik at map_params from one specific PF seed (pmmh.rs:371,482 —
`map_loglik = current_ll`), NOT a fresh clean re-pass. So "report map_loglik"
gives a value COHERENT with map_params (same θ that produced it) but still a
single-seed PF estimate; a reviewer's independent `camdl pfilter` at the saved
params with a different seed will differ by PF Monte-Carlo SE. That is exactly
why the issue's test must allow "agree to within PF SE," and it's why the
stronger fix (fresh clean re-pass, mirroring IF2 at profile.rs:1254-1257) is
more defensible — it makes the reported number reproducible the same way
fit-run's run_quick_pfilter does. TDD red test: profile-PMMH with a sharply
informative nuisance prior chosen so MAP θ is off the likelihood ridge; assert
parse-then-pfilter of mle.toml's [mle] params ≈ final_loglik. Must confirm it
FAILS on current main (best_ll > map_loglik path) before the fix. Reviewer must
check: (1) the deleted best_ll doesn't break diag.loglik_trace / diag.completed
(completed becomes map_loglik.is_finite() — semantically correct, may flip some
cells from "completed" to not, which is honest); (2) final_lp
(map_log_posterior + focal offset, lines 1475-1480) is untouched.

**Sequencing**

No upstream dependency; standalone localized fix, no proposal required.
Conceptually follows f52d1ecd (already landed) which fixed the identical class
on the IF2 arm. Independent of the other open profile gh items (gh#109
log_posterior, gh#118 focal-prior offset) — those touch final_lp, not final_ll.
Should land with its own integration test in the same commit (red-then-green
proof in the message per CLAUDE.md TDD rule).

**Collision / files touched**

Fix touches ONLY rust/crates/cli/src/profile.rs (the Pmmh match arm at lines
1447-1462, plus deleting the now-dead best_ll binding) and adds a test, most
naturally extending rust/crates/cli/tests/profile_pmmh.rs. NO files in
rust/crates/sim/src/inference/* are edited — pmmh.rs is READ-ONLY for this fix
(it is referenced only to confirm map_params/map_loglik coupling at
pmmh.rs:478-483). NOT touching effects.rs or lifecycle.rs. The stronger re-pass
variant still stays inside profile.rs (reuses pf_process/pf_obs_model/smc_cfg
already constructed at profile.rs:1348-1360 and calls
sim::inference::bootstrap_filter, an existing public API — no inference-crate
edit). No collision with the other agent's owned files.

---

## #112 — [compiler] Table lookup arity not validated; under-indexing silently selects wrong cell

**Effort:** S | **Confirmed live:** True

**What the code does today**

Two unguarded paths in ocaml/lib/compiler/expander.ml (line numbers shifted from
the audit's 1507-1523/2788-2837; logic identical).

(1) Lookup lowering — expander.ml:1719-1735 (EIndex arm), the audit's
"1507-1523":

```
let tdims = table_dims ctx base_name in
if tdims <> [] then
  let per_dim = List.mapi (fun i item ->
    let dim      = List.nth tdims i in
    let val_name = index_item_to_str env item in
    (int_of_float (dim_value_index ctx dim val_name),
     List.length (dim_values ctx dim))
  ) items in                      (* <-- iterates `items`, not `tdims` *)
  let n = List.length per_dim in  (* n = #items, NOT table rank *)
  let linear = List.fold_left (fun (acc, pos) (idx, _) ->
    let stride = List.fold_left (fun s j -> s * snd (List.nth per_dim j)) 1
      (List.init (n - pos - 1) (fun k -> pos + 1 + k)) in
    (acc + idx * stride, pos + 1)) (0, 0) per_dim |> fst in
  Ir.TableLookup (base_name, [Ir.Const (float_of_int linear)])
```

`List.mapi (...) items` ranges over the user-supplied index items; the stride
math (lines 1728-1734) is computed from
`n = List.length per_dim = List.length items`, so it is internally
self-consistent with the SUPPLIED items but never reconciled against the table's
true rank `List.length tdims`. There is no
`List.length items = List.length tdims` guard (contrast shape_index for
shaped-lets at expander.ml:1075, which DOES guard `List.length items <> n` and
emits E273 — the M20 fix from the 2026-04-19 review).

(2) Inline table emission — expander.ml:3148-3164 (single-value path in
expand_tables) → table_source_of_expr (3085-3093) → flatten_expr_list
(3069-3082): the inline literal is flattened to `Ir.Inline vals` with the ONLY
check being `Ir.Inline [] -> []` (empty skipped, 3162). No check that
`List.length vals = product (map dim_size tdims)`.

No table-shape/arity validation in ocaml/lib/ir/validate.ml (read lines 1-135):
the error variants (line 11-24) and check_expr_refs (83-101) only verify the
table NAME exists (UnknownTable, line 94-95); rank/shape are not checked. Spec
error codes E202 ("Wrong number of indices for compartment") and E203 ("Index
belongs to wrong dimension") are declared in
docs/camdl-language-spec.md:4426-4427 but
`rg '"E202"|"E203"' ocaml/lib/compiler/*.ml` → ZERO matches; they are never
emitted.

Note: the `read(...)` external path (load_table_data, expander.ml:316-...) DOES
enforce shape — it allocates `Array.make total` over the full dim-size product
(3.30) and dense-checks via a nan sentinel. Only the INLINE literal path and the
LOOKUP path are unguarded.

**Defect**

Table index arity is never validated against the declared table rank, in either
direction:

- UNDER-INDEX (silent wrong cell): `C_age[child]` against `C_age : age × age`
  builds per_dim from the single supplied item, n=1, linear = index_of(child)
  (e.g. 0). That is the row-major cell C_age[child][child] (or worse, cell 0 =
  C_age[child0][child0]), returned silently as if it were the intended value. It
  is NOT an arity error. For a symmetric-dimension table (age × age — the
  canonical contact matrix) under-indexing is ALWAYS silently accepted because
  the supplied level (`child`) is a valid level of the prefix dimension, so the
  E263 not-a-level guard (dim_value_index, expander.ml:950-968) never fires.
- OVER-INDEX (compiler crash, not a diagnostic): `C_age[a,b,c]` against rank-2
  evaluates `List.nth tdims 2` with i=2 ≥ length 2, raising `Failure "nth"`.
  That propagates as an uncaught exception → camdlc prints
  `Error: Failure("nth")` (a stack-trace-style crash), violating the project
  rule against failwith/exceptions for user-facing errors. (The same class of
  bug was already fixed for shaped-lets — see the M20 note at
  expander.ml:1068-1074.)
- INLINE SHAPE MISMATCH: a too-short inline literal (fewer cells than product of
  dim sizes) emits a short `Ir.Inline` array; lookups into the missing tail hit
  the Rust OOB policy late (Ir.Error) or read a wrong in-bounds cell. A too-long
  literal silently carries unused trailing cells with no diagnostic.

Root cause: the dimension/rank vector (`tdims`) is known only at expander time
and is ERASED at IR emission — the IR `table` type (ocaml/lib/ir/ir.ml:169)
carries only name/source/out_of_bounds/cell_kind, and `Ir.TableLookup` is
`string * expr list` reduced to a single `Const linear`. So neither the Rust
validator nor the runtime can recover rank to check arity; the linear index is
fully decoupled from shape once it leaves the expander. The only place rank
exists to be checked is the expander, and the expander does not check it.

**Trigger / repro**

Repro A (silent wrong number, the critical case): a model with
`C_age : age × age = [[12.0,4.0],[4.0,8.0]]` and a force-of-infection rate that
writes `C_age[a]` (one index) instead of `C_age[a,b]`. Compiles clean, runs
clean; the rate uses C_age[a][a] (the diagonal first-cell prefix) instead of the
intended off-diagonal mixing term. No error, no warning — a wrong transmission
rate flows straight into trajectories/likelihood. A user would NOT notice;
output is plausible.

Repro B (crash): same table, write `C_age[a,b,c]`. `camdl`/`camdlc` aborts with
`Error: Failure("nth")` and no source location — user sees a mystery crash, not
a diagnostic.

Repro C (inline too-short): `C_age : age × age = [[12.0,4.0]]` (only 2 cells,
needs 4). Compiles; lookups into rows >=1 either read a wrong in-bounds cell or
trip the Rust runtime OOB error late at simulation time, far from the
declaration.

**Blast radius**

Scientific outputs affected: any model whose dynamics read a multi-dimensional
table with an under-applied or mis-shaped index. The canonical surface is the
contact matrix C_age (age × age) in force-of-infection, plus spatial mixing
kernels (mig[p,q]), age/fertility/mortality demography tables. A
silently-substituted cell changes the per-source transmission rate, so: forward
trajectories shift (wrong epidemic curve / peak timing / attack rate); and under
inference, the posterior for beta (and any parameter that co-varies with FoI:
reporting rho, importation, immunity-waning) shifts to COMPENSATE for the wrong
contact-matrix entry, i.e. a biased point estimate with no diagnostic. Direction
is model-specific (depends on which cell is wrongly substituted — typically the
prefix/diagonal cell, which for a contact matrix is usually larger within-group
mixing, biasing FoI and inferred beta). Most affected model class: age- or
space-stratified transmission models (exactly the cVDPV2 / polio stratified
setting). Single-dimension or scalar-table models are unaffected. The crash case
(over-index) is a hard failure, not a wrong number — less dangerous because it
surfaces.

**Fix shape**

Localized fix in ocaml/lib/compiler/expander.ml, two sites, plus
reuse-the-pattern from shaped-lets:

1. In the EIndex table arm (expander.ml:1719-1735), before building per_dim, add
   an arity guard mirroring shape_index's E273 check (expander.ml:1075): if
   `List.length items <> List.length tdims`, emit a hard diagnostic naming the
   table, declared rank, and supplied count, with a hint listing dimension names
   — use the spec-reserved code E202 ("Wrong number of indices for compartment";
   extend its description to tables, or mint a table-specific E2xx). This both
   kills the silent under-index AND replaces the `List.nth tdims i`
   Failure-crash over-index path with a clean error. (E203/wrong-dimension
   membership is partly covered today by E263 from dim_value_index, but full
   named-index resolution belongs to the broader resolver in audit finding #1.)

2. Add `validate_table_decl` for the inline path. In expand_tables (the
   single-value inline branch, expander.ml:3148-3164), after
   `table_source_of_expr` yields `Ir.Inline vals`, compute
   `expected = List.fold_left ( * ) 1 (List.map (fun d -> List.length (dim_values ctx (dim_name_of_entry d))) dim_entries)`
   and compare to `List.length vals`; emit a hard diagnostic (new E2xx) when
   they differ, before emitting the IR table.
   dim_value_index/dim_values/dim_name_of_entry (expander.ml:806, 880, 950)
   already provide the size lookups.

Both are localized expander changes — NOT a design proposal. The arity check is
genuinely standalone. Caveat: fully spec-compliant
named/positional/omitted-dimension table indexing (order-independent named
indices, omitted-dim summation) is audit finding #1's resolver and is a larger
lift; this issue should land the arity + inline-shape guards now and defer named
resolution to #1.

**Risk**

Delicate points a reviewer must check: (1) Omitted-dimension summation. The spec
(§ "Omitting a dimension sums over it", docs/camdl-language-spec.md:766-773)
says omitting a table dimension in a rate expression is VALID and means a sum
over it — so a blanket `items length must equal rank` rule could over-reject.
BUT: the current code does NOT implement omitted-dim summation for tables at all
(it just under-indexes and produces a wrong scalar), so today there is no valid
omission behavior to break. The safe fix is: reject under-indexing with E202 NOW
(matching the audit's "require exact arity"); implement omitted-dim-as-sum only
as part of audit #1's resolver, with its own tests. The reviewer must confirm
the fix does not silently bless omission as the wrong-cell behavior. (2) Confirm
no golden fixture currently relies on under-indexed table access (grep fixtures
for `C_age[` / single-index table reads; the spec examples all use full
`C_age[a,b]`). TDD red test: a fixture with `C_age : age × age` and rate
`... C_age[a] ...` — assert camdlc EXITS NON-ZERO with E202 (today it compiles
green and the simulate output uses the diagonal cell; the red test is "compile
should fail but doesn't"). Add a second red test for the inline-too-short case
and a third asserting the over-index emits E202 not `Failure "nth"`. (3) The
E202 description in the spec is phrased for compartments; either broaden it or
mint a sibling code — keep error-code/spec table in sync.

**Sequencing**

Independent of, but adjacent to, audit finding #1 (gh issue for "indexed
references lowered by string concat / no dimension-aware resolver"). The arity +
inline-shape guards here can and should land FIRST and standalone — they are
pure additions of hard diagnostics and do not need #1's resolver. The
forward-looking part (named-index resolution by declared dimension,
omitted-dimension summation for tables) is #1's territory; do NOT implement it
here. No proposal needed for the localized guards. No IR/schema change (rank
stays expander-only). If both are scheduled, land #112's arity guard, then #1's
resolver subsumes the named/positional resolution and omitted-dim summation on
top.

**Collision / files touched**

Fix touches ONLY: ocaml/lib/compiler/expander.ml (EIndex table arm ~1719-1735;
expand_tables inline branch ~3148-3164). Possibly a new error-code row in
docs/camdl-language-spec.md (E202/sibling) and ocaml diagnostics registration if
codes are enumerated. New OCaml test fixture(s) under tests/fixtures/ + golden
regen. NO Rust files. NONE of rust/crates/sim/src/inference/*, effects.rs, or
lifecycle.rs are touched — no collision with the other agent's surface. (Rust
runtime resolved_expr.rs:476 OOB-checks the flat index but is not modified by
this fix.)

---

## #98 — Typed-time OCaml↔Rust unification: parse_iso_date drift, dead origin_rata_die field, missing golden table

**Effort:** M | **Confirmed live:** True

**What the code does today**

Bundles 4 sub-defects; all verified live on main (line numbers in the issue body
have drifted, real anchors below).

C6 — OCaml parse_iso_date does NO range/zone/whitespace validation.
ocaml/lib/compiler/expander.ml:123-128:

```
let parse_iso_date s =
  match String.split_on_char '-' s with
  | [ys; ms; ds] ->
    (try (int_of_string ys, int_of_string ms, int_of_string ds)
     with _ -> failwith (Printf.sprintf "invalid date literal '%s': components must be integers" s))
  | _ -> failwith (...) "date literal must be YYYY-MM-DD, got '%s'" ...
```

It only rejects non-integer components. `days_in_month` exists in OCaml
(expander.ml:164) but parse_iso_date never calls it. Contrast Rust
caltime.rs:101-143 which trims (`s.trim()`), accepts/discards zone designators
(Z, +HH:MM), rejects time-of-day (DatetimeUnsupported), and validates
`!(1..=12).contains(&m) || d < 1 || d > days_in_month(y, m)` →
CalError::OutOfRange (caltime.rs:139-141). So: `2020-13-01`, `2020-02-30`,
`2020-03-15Z`, `2020-01-01` all parse on OCaml, three of them error on Rust.

I reproduced the silent garbage with the actual OCaml days_of_date formula
(origin 2020-01-01): 2020-02-30 → delta 60 days (= rata 43888, identical to
2020-03-01) 2020-13-01 → delta 366 days (= identical to 2021-01-01) Both emitted
as EConst into the IR with NO diagnostic.

C7 — origin_rata_die is computed by OCaml (expander.ml:5267-5270:
`Some (days_of_date y m d)`), serialized (serde.ml:989), deserialized
(serde.ml:1023), and hashed into the IR fingerprint
(rust/crates/runid/src/ir_hash.rs:795). It is NEVER read for time conversion.
The runtime data-loader converts via caltime_load.rs:214
`ir::caltime::date_to_internal(origin, c.trim(), opts.time_unit)`, and
date_to_internal (caltime.rs:149-152) re-parses the origin string fresh:
`let (oy, om, od) = parse_iso_date(origin)?; ... rata_die(oy,om,od)`. Every
other origin_rata_die reference in runtime crates is a `None` struct-literal
init. The schema description is false — schema.json:33: "the runtime reads this
instead of re-parsing the origin string."

M13 — date_range absorbs malformed dates to 0.0. expander.ml:1370-1371:
`(try parse_date_to_float origin_str iso ctx.time_unit with Failure _ -> 0.0)`.
(Other date paths DO emit diagnostics on Failure: the date() literal at
1847-1851 emits E220, the table cell at 451-456 emits E209, add_calendar_* at
1916-1921 emits E328. But none of these catch OUT-OF-RANGE dates — those don't
raise Failure, they sail through to days_of_date.)

M14 — origin is stored verbatim with no validation. expander.ml:588:
`| DOrigin s -> ctx.origin <- Some s`. A malformed origin reaches both the IR
`origin` string and the origin_rata_die computation (expander.ml:5270) silently.

No golden cross-language table: `find … -name caltime.tsv` → no results.
ir/validate.rs has no origin/rata_die consistency check. The cited OCaml test
(test_compiler.ml:4982 test_origin_rata_die_emitted) asserts
`Some (Expander.days_of_date 2020 2 28) = m.Ir.origin_rata_die` —
OCaml-vs-OCaml, with a comment (4979-4981) claiming "the runtime can read it
without re-parsing" — which is false.

**Defect**

Two independent, un-pinned ISO-date parsers (OCaml expander.ml parse_iso_date;
Rust caltime.rs parse_iso_date) accept different string sets. The OCaml side
performs no calendar-range validation, so out-of-range date literals
(`2020-02-30`, `2020-13-01`) are silently aliased to a valid neighbor
(2020-03-01, 2021-01-01) and emitted into the IR as a wrong f64 time, with no
diagnostic on either side (Rust never re-checks a date() literal — it is already
a float by the time the IR reaches Rust). Separately, origin_rata_die is
dead-but-fingerprinted: emitted, serialized, hashed into the run-id, and
documented as the canonical conversion source, but never actually read for
conversion (both sides re-parse the origin string). The two parsers agree by
luck until a string lands in their divergent zone.

**Trigger / repro**

Silent wrong number (no error), worst-case footgun:
`origin = date("2020-01-01")` then `date("2020-02-30")` anywhere a date()
literal appears (transition rate guard, init, intervention time, event time,
instant/duration param) → OCaml emits EConst 60.0 (aliasing 2020-03-01); no
E-code. The same string passed through the --data loader on the RUST side (a
dated TSV column, caltime_load.rs:214) would error with CalError::OutOfRange. So
the literal and the data file silently disagree, or the literal is silently off
by ~1-3 days at a month boundary / by a year for a bad month. date_range with a
malformed start/end is even quieter — absorbed to 0.0 (expander.ml:1371),
collapsing a cadence start to t=0. Whitespace/zone variants (`2020-01-01`,
`2020-03-15Z`) fail-hard on OCaml (Failure → E220) while succeeding on Rust — an
inconsistency a user hits if they paste a date with a stray space.

**Blast radius**

Any model that uses a date() literal or date_range with an out-of-range or
boundary date the author typo'd. Output that moves: every internal-time value
derived from that literal — observation cutoff times, SIA/campaign intervention
firing times, cohort-entry event times, instant/duration parameter values,
forcing onset times. Direction: a typo'd day-of-month rolls forward into the
next month (delta off by up to ~2 days for Feb-30/31 style typos); a bad month
(>12) rolls into the next year (delta off by months). For the polio cVDPV2 use
case the issue itself flags: an SIA campaign timing or AFP-surveillance window
can shift across an incidence peak, moving estimated campaign impact — a
confidently wrong fit with no error. Affects all model classes equally (the date
path is backend-independent; it runs at compile time in OCaml). The
origin_rata_die deadness also means the run-id/IR-fingerprint includes a field
with no runtime meaning — harmless to numerics but a phantom in provenance.

**Fix shape**

One atomic commit, two acceptable shapes (issue offers both):

Shape A (validate + make rata_die canonical):

1. Rewrite ocaml/lib/compiler/expander.ml parse_iso_date (123-128) to return a
   result/raise a typed error that validates month∈1..12 and
   day∈1..days_in_month(y,m) (days_in_month already exists at expander.ml:164),
   and to handle/discard a trailing zone designator and trim whitespace —
   mirroring Rust caltime.rs:101-143 exactly. Pick ONE canonical grammar and
   port to the lossy (OCaml) side.
2. Replace the date_range `with Failure _ -> 0.0` absorb (expander.ml:1370-1371)
   with a proper diagnostic (E328, consistent with the other date_range errors).
3. Validate origin up front in to_ir / at DOrigin handling (expander.ml:588 and
   the emission at 5267-5270): emit an E-code on malformed origin instead of
   silently computing a garbage origin_rata_die.
4. Either (4a) make origin_rata_die the primary conversion path: thread it into
   date_to_internal/internal_to_date in rust/crates/ir/src/caltime.rs and add an
   ir/validate.rs check that
   `origin_rata_die == rata_die(parse_iso_date(origin))` on load; OR (4b) DELETE
   origin_rata_die entirely (cleaner per CLAUDE.md "delete dead code on sight" —
   it has no runtime consumer except the IR hash). 4b touches schema.json,
   ir/VERSION, model.rs:144, serde.ml:989/1023, ir_hash.rs:795, and ALL the
   `origin_rata_die: None` struct-literal initializers across the test/cli
   files, plus golden regeneration. 4a is smaller in surface but keeps a field
   that must now be defended by a validator.
5. Add ir/golden/caltime.tsv + rust/crates/ir/tests/caltime_golden.rs +
   ocaml/test/test_caltime_golden.ml, both reading the same TSV, covering leap
   rules, month boundaries, dates ≤1583, negative deltas, zoned strings.
6. Fix the schema.json:33 description (remove the "runtime reads this" claim)
   and the misleading test_compiler.ml:4979-4981 comment.

This needs the canonical-grammar decision documented (the issue says proposal
§6.4 mandated one, but the cited proposals 2026-05-22-typed-time/calendar-time
are no longer present under docs/dev/proposals/ — a short proposal or design
note should re-pin the grammar before implementing, especially the 4a-vs-4b
choice).

**Risk**

High-care surface: changing parse_iso_date semantics changes which models
COMPILE — TDD red test must assert (a) `date("2020-02-30")` now errors with a
named E-code, not EConst 60.0, and (b) OCaml and Rust accept/reject the
identical string set (this is the load-bearing equivalence the current
OCaml-vs-OCaml test fails to provide). A reviewer must check: the OCaml rata_die
formula days_of_date and Rust rata_die stay byte-identical (only deltas are
load-bearing, but the golden TSV pins absolute values too); the zone-discard and
whitespace-trim behavior matches exactly (subtle: Rust trims THEN takes first 10
chars); changing origin_rata_die (4b delete) must regenerate golden IR and bump
ir/VERSION atomically, and will change every IR fingerprint/run-id that
currently includes the field (ir_hash.rs:795) — verify that is intended and
re-bless run-id goldens. The date() literal path already emits E220 on Failure;
do not regress that. Surface the canonical-grammar choice explicitly — guessing
it is the §5.6 failure the issue warns against.

**Sequencing**

The 4 sub-items (C6/C7/M13/M14) are coupled and should land together per the
issue (atomic). The canonical-grammar pin and the 4a-vs-4b decision (thread
rata_die vs delete it) should be settled in a short proposal/note FIRST — the
originally-cited proposals (docs/dev/proposals/2026-05-22-typed-time…,
…-calendar-time) are no longer present at those paths, so the design rationale
must be re-established or located before implementation. No hard dependency on
other open issues found. If 4b (delete) is chosen, it is a schema change →
follow CLAUDE.md "Changing the IR schema" (bump ir/VERSION, update both
languages, regenerate all golden + expected, one commit).

**Collision / files touched**

Primary files: ocaml/lib/compiler/expander.ml (parse_iso_date 123-128,
date_range absorb 1370-1371, DOrigin 588, origin_rata_die emission 5267-5270);
rust/crates/ir/src/caltime.rs (date_to_internal / optional rata_die threading);
rust/crates/ir/src/validate.rs (new origin consistency check);
ir/schema.json:31-34 (description fix) + ir/VERSION if 4b; new
ir/golden/caltime.tsv, rust/crates/ir/tests/caltime_golden.rs,
ocaml/test/test_caltime_golden.ml; ocaml/test/test_compiler.ml comment fix
(4979-4981). If 4b (delete origin_rata_die): also
rust/crates/ir/src/model.rs:144, ocaml/lib/ir/ir.ml:423,
ocaml/lib/ir/serde.ml:989/1023, rust/crates/runid/src/ir_hash.rs:795, and the
many `origin_rata_die: None` initializers in cli/ and sim/ test files.

COLLISION FLAG: 4b would touch rust/crates/sim/src/effects.rs (lines 575, 808,
945 — `origin_rata_die: None` struct inits) which another agent owns. It also
touches several files under rust/crates/sim/tests/ (not the protected inference/
source dir). It does NOT touch rust/crates/sim/src/inference/* or lifecycle.rs.
Shape 4a avoids effects.rs entirely (no field removal). Prefer 4a, or coordinate
the effects.rs init edits if 4b is chosen.

---

## #114 — [compiler] Stratified initial conditions not checked against expanded compartments

**Effort:** M | **Confirmed live:** True

**What the code does today**

Issue line numbers have drifted (cited expander.ml:2883-2928 is now the
prior-classification code; the real site is `expand_init` at
expander.ml:3210-3255). The defect is live.

PARSER — `ocaml/lib/compiler/parser.mly:721-727` accepts any `IDENT` as an init
LHS with no compartment check:

```
init_entry:
  | comp = IDENT LBRACKET ibs = ... RBRACKET EQ v = expr   { { icomp = comp; iindices = []; ibindings = ibs; ... } }
  | comp = IDENT idxs = index_items_opt EQ v = expr         { { icomp = comp; iindices = idxs; ibindings = []; ... } }
```

EXPANDER — `expand_init` (expander.ml:3210-3255). The bare form emits `ie.icomp`
VERBATIM as the init key (line 3222-3231):

```
let concrete_name =
  if ie.iindices = [] then ie.icomp           (* <-- bare stratified S kept as "S" *)
  else ... String.concat "_" (ie.icomp :: idx_vals)
in
let resolved = normalize_expr (resolve_expr ctx [] ie.ivalue) in
add_entry concrete_name resolved
```

No `Hashtbl.mem ctx.expanded_comp_tbl concrete_name` check, and no
bare-stratified→all-cells expansion — even though `expanded_comp_tbl` is built
and ready (build_lookup_tables, expander.ml:872-876) and `resolve_ident_name`
(expander.ml:2122-2128) already expands a bare stratified compartment to a
`PopSum` in rate context. The sibling constructs DO check: interventions emit
E265 against `expanded_comp_tbl` (expander.ml:3932-3943); prevalence projections
expand bare-stratified to `CurrentPopSum` (expander.ml:4018-4031). `expand_init`
is the lone holdout.

OCAML VALIDATOR — `validate.ml:103-191`: `validate` checks stoichiometry, ODE
comps, observation projections/likelihoods, bindings. It never iterates
`m.initial_conditions` (confirmed:
`rg initial_conditions ocaml/lib/ir/validate.ml` → no matches).

RUST VALIDATOR — `validate.rs:55-174`: builds `comp_names` and checks
transitions/ODE/observations, never `model.initial_conditions` (confirmed: only
hit for `init` in validate.rs is unrelated `initial_value: None` at line 304).

RUNTIME — `compiled_model.rs:1049-1060` (`initial_state`):
`int_counts`/`real_values` are zero-initialized (lines 1042-1043);
`InitialConditions::Explicit` looks each key up in `self.comp_index` and
`.ok_or_else(|| SimError::UnknownCompartment(name.clone()))?`. So a key absent
from comp_index is a hard sim-time error; cells never named in the map stay at
their zero init silently.

**Defect**

Two distinct wrong behaviors, both stemming from the missing init-LHS
resolution/validation:

(1) LOUD-BUT-LATE: A bare init for a stratified compartment, e.g.
`init { S = N0 }` where `S` was stratified into `S_child_kano`, `S_adult_kano`,
... emits an IR init entry literally keyed `"S"`. Neither IR validator rejects
it. It surfaces only at sim time as `SimError::UnknownCompartment("S")` from
`initial_state` — past load/validate, deep in the run.

(2) SILENT-WRONG: Because no validator enforces that every expanded compartment
is initialized (or that init keys are real), any expanded cell not explicitly
named in `init {}` retains the zero default. The author who writes the bare `S`
intends "all the S strata get N0"; instead either the run aborts (case 1) or, in
mixed/partial-coverage models, the named cells take values and the unnamed
strata silently start at 0 — a plausible-but-wrong initial population. The
expander asymmetry is the root cause: rate/prevalence context expands bare
stratified names to a sum-over-strata, but init context does not.

**Trigger / repro**

Concrete repro: a model with
`dimensions { age = [child, adult]; loc = [kano, lagos] }`,
`compartments { S; I; R }` stratified by age,loc, and
`init { S = 1000; I = 1 }`. Compile (camdlc) → IR contains
`initial_conditions: Explicit({"S": 1000, "I": 1})` with NO `S_child_kano`/etc.
keys. `camdl simulate model.ir.json` → either errors at sim start with
`UnknownCompartment("S")` (bare name not in comp_index), or in a
partially-indexed model some cells are set and the rest silently start at 0.

User visibility: case (1) is a runtime error but with a misleading message (says
"unknown compartment S" at sim time, not "you initialized a stratified
compartment without indices" at compile time, and not pointed at the init block
source location). Case (2) is fully silent — no error, just a wrong epidemic
that looks plausible (peaks lower / later because much of the population starts
empty). Critically, neither IR validator fires, so loading/validating the IR
passes clean.

**Blast radius**

Affects every STRATIFIED model class (age/space/risk groups) — the polio cVDPV2
spatial-by-patch and age-stratified models are squarely in scope; unstratified
SIR is unaffected. Scientific output that moves: the entire forward trajectory
and, downstream, any posterior fit to it. Direction: initial
susceptible/infectious pools are under-counted (unnamed strata sit at 0), so
simulated incidence/prevalence is biased LOW and the epidemic onset is
delayed/attenuated; an inference run conditioning on real data would compensate
by inflating transmission parameters (beta/R0) to fit observed cases against an
artificially small starting population — a biased posterior that informs
eradication strategy. Because it is silent in the partial-coverage case, it can
pass review and reach a published trajectory.

**Fix shape**

Two coordinated changes, matching the issue's prescription:

(1) EXPANDER — `ocaml/lib/compiler/expander.ml`, `expand_init` (lines
3210-3255). For the bare form (iindices = [] and ibindings = []): branch on the
tables exactly as `resolve_ident_name`/`prevalence_projection` do — if
`Hashtbl.mem ctx.expanded_comp_tbl ie.icomp` (already a concrete cell) accept
as-is; else if `Hashtbl.mem ctx.comp_tbl ie.icomp` (declared base name that
stratifies), HARD ERROR with a new E-code (issue says require explicit
expansion/indexed binding) — name old→new in hint; else fall through to
validator. For fully-indexed and indexed-binding forms, validate each computed
`concrete_name` against `expanded_comp_tbl` and error if absent. The
indexed-binding (`S[p in patch] = ...`) loop already expands to concrete cells
(lines 3235-3247) — just add the membership check. Reuse
`expand_compartment_name` if a decision is made to auto-expand bare-stratified
to all cells instead of erroring (the issue chooses ERROR; follow it).

(2) BOTH IR VALIDATORS — add an initial-condition reference pass:
`ocaml/lib/ir/validate.ml` (new error variant + iterate `m.initial_conditions`
Explicit/Parameterized keys against `comp_names`) and
`rust/crates/ir/src/validate.rs` (new `ValidationError` variant + iterate
`model.initial_conditions` keys against `comp_names`). This enforces the
constraint at the contract boundary, not just the frontend, and converts the
late `SimError::UnknownCompartment` into a clean validate-time diagnostic.

Localized fix, no design proposal needed (the resolver pattern and tables
already exist). One judgment call worth surfacing to the maintainer:
ERROR-on-bare-stratified (issue's choice) vs. EXPAND-to-all-cells (symmetric
with rate/prevalence context). The issue picks error; honor that unless the
maintainer prefers symmetry. New E-code should be added to
docs/language-changes.md if it tightens accepted syntax.

**Risk**

Delicate points: (a) TDD red test must distinguish the two cases — one camdl
fixture with bare stratified init asserting a compile-time error (currently
passes through to a sim-time UnknownCompartment, so the red test should assert
the NEW E-code from the expander, or the new validator error), and one asserting
indexed-binding init still expands to all cells correctly. (b) Must not break
legitimate bare init of an UNSTRATIFIED compartment (`init { S = N0 }` in a flat
SIR) — that path must stay allowed (expanded_comp_tbl contains the bare name).
(c) `Parameterized` init (non-const RHS) goes through the same key path —
validator must cover both Explicit and Parameterized arms. (d) Choosing ERROR
over auto-expand is a breaking surface change: any existing golden fixture or
camdl-book model that relies on bare stratified init will now fail to compile —
grep golden fixtures (`ir/golden/*`, `tests/fixtures/*.camdl`) for bare
stratified inits before landing, and `make update-golden`/`update-expected` will
need regeneration. (e) Reviewer must confirm the new diagnostic carries source
location (iloc is captured in init_entry, parser.mly:724) and hint text per the
error-quality bar.

**Sequencing**

Issue body and audit both say "reuse the indexed-reference resolver (#111)".
Verify #111 is landed (the `expanded_comp_tbl`-based resolution at
expander.ml:2122-2128 and the indexed prevalence/intervention paths already
exist on main, so the machinery is present regardless of #111's status — this
fix can proceed without waiting). No proposal required. Should land atomically
as one commit touching the OCaml expander + both validators + regenerated
golden/expected files (per the "Changing the IR schema" / golden-file discipline
— though no schema change here, the accepted-syntax tightening forces golden
regen if any fixture uses bare stratified init). If a new IR ValidationError
variant is added on the Rust side, confirm it does not require a schema/VERSION
bump (validation errors are runtime, not serialized — no bump needed).

**Collision / files touched**

Files the fix touches: `ocaml/lib/compiler/expander.ml` (expand_init, ~lines
3210-3255), `ocaml/lib/ir/validate.ml` (new error variant + init pass),
`rust/crates/ir/src/validate.rs` (new ValidationError variant + init pass), plus
possibly `docs/language-changes.md` and regenerated `ir/golden/*` /
`ir/expected/*` fixtures. NO COLLISION with the other agent's surface: none of
these are under `rust/crates/sim/src/inference/*`, nor `effects.rs`, nor
`lifecycle.rs`. Note `compiled_model.rs:initial_state` is the runtime consumer
that currently throws the late error — the fix does NOT need to edit it (the
validator catches it earlier), and it is not in the other agent's owned set
either, but leave it untouched to keep the diff minimal.

---

## #117 — [compiler] Duplicate and cross-namespace names silently overwritten (Hashtbl.replace) + wrong resolution order

**Effort:** M | **Confirmed live:** True

**What the code does today**

Issue line numbers are stale (file shifted) but the defect is live. Two distinct
defects in /Users/vsb/projects/work/camdl/ocaml/lib/compiler/expander.ml:

DEFECT 1 — last-wins, no diagnostic. `build_lookup_tables` (line 840) builds
every namespace table with `Hashtbl.replace`: 842 let lt = Hashtbl.create
(List.length ctx.let_bindings) in 843 List.iter (fun lb -> Hashtbl.replace lt
lb.lname lb) ctx.let_bindings; 847 ...Hashtbl.replace ct cd.cname cd)
ctx.comp_decls; (* compartments _) 852 | PScalar p -> Hashtbl.replace spt
p.pname () (_ scalar params _) 863 ...Hashtbl.replace ept (pname ^ "_" ^ v) ()
(_ expanded indexed params _) 869 ...Hashtbl.replace ft fd.fname fd)
ctx.func_decls; (_ forcing/funcs _) 874 ...Hashtbl.replace ec n ()) expanded; (_
expanded compartments *) Duplicate declarations within a namespace (two
`let beta`, two `parameter beta`, two compartments `S`, two forcings) silently
collapse to the last one. `collect_declarations` (line 582) only runs
`check_reserved` per decl — no within-namespace or cross-namespace uniqueness
check. Grep for duplicate-name diagnostics in expander.ml finds only unrelated
cases (table rows, prior kwargs). There is NO validation pass anywhere in
`expand_detail` (line 5231) that rejects duplicate or ambiguous declaration
names; the `caught downstream by Validate` comment at line 2199 is about
_unknown_ compartments, not duplicates.

DEFECT 2 — wrong resolution order. `resolve_ident_name` (line 2107) checks let
bindings FIRST: 2109 match Hashtbl.find_opt ctx.let_tbl name with 2110 | Some lb
-> ... (inline the let) 2121 | None -> (* 2. compartment? _) ... (_ then
scalar_param_tbl at 2129 *) So order is lets → compartments → parameters → funcs
→ reserved keywords (`t`,`dt`,`projected`,`pi`,`e`,`origin`). Spec §9.7
(line 1844) and §26.10 (line 4552) both mandate: compartments → parameters → let
bindings → forcing → tables, AND
`The compiler reports an error if a name exists in multiple namespaces`. Code
does the opposite for lets and never errors on ambiguity.

`check_reserved` (line 572) only reserves `t,t_start,t_end,dt`
(reserved_time_names) and `pi,e` (reserved_math_names) — NOT `origin`,
`projected`, `sum`, `consecutive`, `compartments`, so a `let origin = ...`
silently shadows the `origin` keyword via the lets-first path. `check_shadowing`
(line 4291) only emits W103 for let names matching stratum values.

**Defect**

Two coupled defects: (1) every namespace lookup table is built with
Hashtbl.replace, so duplicate declarations within a namespace are silently
overwritten last-wins with no diagnostic; there is no pass that rejects
duplicates or cross-namespace ambiguity. (2) resolve_ident_name resolves let
bindings before compartments and parameters — the inverse of the spec's mandated
order (compartments → parameters → lets → forcing → tables) — and never errors
on a name living in two namespaces. Net: a name colliding across kinds resolves
to whichever the code happens to check first (a let, or for typed-const lets an
Ir.Param), not the intended declaration, and the source still reads as
plausible.

**Trigger / repro**

Two concrete repros, both compile clean (no error, no warning) and silently
change equations:

1. Cross-namespace (param vs let): declare `parameter N : count` AND
   `let N = S + I + R`. A rate `beta * I / N` resolves N to the let (inlined
   PopSum S+I+R), not the parameter. If the user intended the fixed-population
   param, every rate is now divided by the live population sum instead of a
   constant — different dynamics, no diagnostic. Also: `let origin = 5` shadows
   the `origin` calendar keyword silently.

2. Within-namespace duplicate (two lets): two `let beta = ...` lines (e.g. a
   copy-paste/merge artifact) — the second silently wins, the first vanishes
   with no diagnostic.

A user would see NO error and NO warning — a silent wrong number in the
simulated/inferred trajectory. This is exactly the class CLAUDE.md
`no loose semantics` exists to forbid.

**Blast radius**

Affects every model that has a name collision — forward simulation AND inference
(IF2/PGAS/PF) since they all consume the same expanded rate IR. Direction is
model-specific: in the param-vs-let `N` case the rate denominator silently
switches from a constant to a state-dependent PopSum, shifting peak
timing/height of the trajectory and therefore every downstream posterior over
transmission params (beta, R0) and any reporting/observation fit keyed off those
rates. For polio cVDPV2 work where compartment/param names carry epidemiological
meaning, a copy-paste duplicate `let` or a param/let name clash produces a
plausible-looking model whose equations differ from what the author reads — a
correctness defect in inference inputs, not just ergonomics. No model class is
exempt; risk scales with name reuse, highest in large stratified models
authored/edited by agents.

**Fix shape**

Two localized changes, both in ocaml/lib/compiler/expander.ml; no proposal
needed (spec is already normative and unambiguous). (1) Add a
`check_declaration_names ctx` pass called in `expand_detail` (line 5234ff) AFTER
`resolve_dimensions` (so expanded indexed-param/compartment names exist) but
BEFORE `build_lookup_tables` (line 5238). It must: reject duplicates within each
namespace (lets, scalar params, indexed params, compartments,
forcing/func_decls, tables) via fold-with-prior-existence-check; reject any name
present in >1 of {compartments, parameters, lets, forcing, tables}; and extend
the reserved set so `origin`, `projected`, `sum`, `consecutive`, `compartments`
are rejected as declaration names consistently with `t/dt/pi/e` (extend
`reserved_time_names`/`check_reserved` at line 569-580, or add a
`reserved_keyword_names` list). Emit proper E-coded diagnostics with declaration
loc + hint (reuse `compartment_loc`/`param_loc` at 549-562 for second-occurrence
locations). (2) Reorder `resolve_ident_name` (line 2107) to spec order:
compartments (expanded_comp_tbl then comp_tbl) → parameters (scalar then
expanded-indexed) → lets → funcs → reserved keywords. Once the validation pass
guarantees no cross-namespace collision, the reorder is safe and ambiguity is
impossible by construction. Note the typed-const-let → Ir.Param branch
(line 2111) must move with the let case. Follow TDD: add red tests asserting
E-on-duplicate-let, E-on-param/let-collision, and correct resolution-order,
confirm they fail on current code, then fix.

**Risk**

Delicate points a reviewer must check: (a) Reordering resolve_ident_name is the
riskiest part — it changes which IR node a name lowers to, so it MUST be paired
with the validation pass that makes collisions impossible; otherwise existing
models that _rely_ on the current lets-first behaviour (intentionally or via the
typed-const-let→Param branch at line 2111) would silently change output. Re-run
the full golden suite (`make update-golden` diff must be empty for non-colliding
models) and the expected TSVs. (b) The validation pass must run after
`resolve_dimensions` so `expanded_param_tbl`/`expanded_comp_tbl` keys (e.g.
`R0_urban`) participate in cross-namespace/duplicate checks — an expanded
indexed param could collide with an expanded compartment name. (c) Choose error
codes per CLAUDE.md `Error messages are a feature` (point to the two conflicting
decl locs; prefer a cross-namespace-ambiguity code over a generic one). (d)
Reserving `compartments`/`sum`/`consecutive` is a breaking language change — per
CLAUDE.md it needs a migration-grade diagnostic and a `docs/language-changes.md`
entry; check no golden/fixture model currently uses those as names. Required
red→green TDD proof: a test declaring two `let beta` (and one declaring
`parameter N` + `let N`) must FAIL to error on current main, then error after
the fix.

**Sequencing**

Independent — no dependency on other issues and nothing must land first. The fix
is self-contained in the OCaml expander. It does interact in spirit with audit
finding #9 (partial-stratification stoich) since both touch
resolution/expansion, but they edit different functions and can land in either
order. Recommend the validation pass and the resolution-order reorder land in
ONE atomic commit (the reorder is unsafe without the validation guard). A
`docs/language-changes.md` entry is required because newly reserving
`compartments`/`sum`/`consecutive` is a breaking surface change.

**Collision / files touched**

Touches ONLY ocaml/lib/compiler/expander.ml (new `check_declaration_names`
pass + call site in `expand_detail`; reorder `resolve_ident_name`; extend
reserved-name list in `check_reserved`). Plus docs: docs/camdl-language-spec.md
is already correct (no edit needed) and a new entry in docs/language-changes.md
for the breaking reserved-word additions. Possibly the warning catalog / new
E-code registration. NO Rust files. NONE of the touched files are in
rust/crates/sim/src/inference/*, effects.rs, or lifecycle.rs — no collision with
the other agent's surface. New OCaml tests under ocaml/test/.

---

## #129 — survey_top_k ranks chain inits by likelihood, not posterior — silent bias under non-flat priors

**Effort:** M | **Confirmed live:** True

**What the code does today**

VERIFIED on current main (HEAD cb5fdd0f). The defect is live; only a loud
unconditional warning (commit fe42851) mitigates it.

(1) The rank step in `rust/crates/cli/src/fit/init.rs:794-798` sorts surveyed
points by raw loglik descending and takes top-K:

```
let mut ranked: Vec<&LandscapeRow> = filtered;
ranked.sort_by(|a, b| {
    b.loglik.partial_cmp(&a.loglik).unwrap_or(std::cmp::Ordering::Equal)
});
let selected: &[&LandscapeRow] = &ranked[..top_k];
```

No prior term enters the comparison. The chains assembled from `selected`
(init.rs:807-814) become the PGAS/PMMH chain starts.

(2) The survey landscape parser only knows `loglik`/`loglik_se` columns —
`LandscapeRow` at init.rs:843-848 has exactly `params, loglik, loglik_se`;
`parse_landscape_tsv` (init.rs:855-866) hard-requires `loglik` and `loglik_se`
headers and reads nothing else. So even an opt-in posterior rank is impossible:
the data isn't there.

(3) The survey writer never emits a prior column. `write_landscape` header build
at `rust/crates/cli/src/survey.rs:1085-1093` is
`<params...> loglik loglik_se [mean_ess] n_replicates point_id`; the row body
(survey.rs:1094-1104) writes only those. The per-point eval
(`eval_point_pfilter` survey.rs:947-1014, `eval_point_simulate`
survey.rs:1021-1054) computes loglik only — pfilter via `bootstrap_filter`
log_likelihood, simulate via `compute_ode_loglik`. No `log_density` call
anywhere in survey.rs (`rg prior rust/crates/cli/src/survey.rs` → only comment
mentions, no prior evaluation).

(4) `ResolvedSurveyInputs` (survey.rs:56-85) carries
`compiled, model_ir_json, base_params, estimated, obs_models, per_stream_obs, data_hashes, fixed, scenario, estimate_starts`
— NO prior field. So the audit's claim that "the [estimate] priors are already
in scope at the survey site" is only half-true: priors are reachable from
`config.estimate[name].prior` inside `resolve_survey_inputs` (fit-aware mode,
survey.rs:589-597 already iterate `config.estimate`), but they are NOT currently
resolved into `Prior` objects nor threaded into the writer/eval loop.

(5) The mitigation: `emit_rank_by_likelihood_bias_warning()` fires
unconditionally at init.rs:801, defined at init.rs:977-998. It is a warning only
— it does not change the rank or refuse.

(6) The cross-check infra the v2 fix mirrors exists: `SurveyCrossCheck`
(init.rs:826-840) reads `model_hash`/`data_hashes`/`fixed`/`estimated` from
survey `run.json` `inputs`; writer side records them at survey.rs:526-537
(`model_hash = crate::hashing::model_hash(...)`). There is NO `prior_hash`
today.

The fit side already has the machinery to compute a prior:
`resolve_prior(name, estimate, model) -> (Prior, &str)` at
`rust/crates/cli/src/fit/runner.rs:2141-2180` (fit.toml override → model IR →
flat fallback), and `Prior::log_density(natural, transformed)` at
`rust/crates/sim/src/inference/prior.rs:89-145`.

**Defect**

`build_chain_starts_from_survey` ranks surveyed parameter points by marginal
likelihood p(y|θ) and seeds PGAS/PMMH chains at the top-K likelihood points. But
PGAS and PMMH target the posterior p(θ|y) ∝ p(y|θ)·p(θ). When any estimated
parameter has a non-flat prior, argmax p(y|θ) ≠ argmax p(θ|y); the rank is
computed on the wrong objective. The chain inits are therefore systematically
pulled toward the MLE/likelihood ridge irrespective of prior mass. The mechanism
is a missing additive `log_prior` term in the sort key (loglik vs
loglik+log_prior); the prior contribution is deterministic per point, so adding
it would re-order ties and tails but the data to do so is never produced
upstream by the survey writer.

**Trigger / repro**

Concrete repro: a fit.toml with at least one non-flat prior, e.g.
`[estimate.beta] prior = { log_normal = { mu = -0.3, sigma = 0.5 } }`, run as:
camdl survey --fit fit.toml --out survey_dir/ camdl fit run fit.toml
--survey-path survey_dir/ --init survey_top_k The PGAS/PMMH stage seeds its K
chains at the top-K loglik rows of `survey_dir/landscape.tsv`. The user sees the
loud `emit_rank_by_likelihood_bias_warning` text on stderr (so it is not fully
silent post-fe42851), but the NUMBERS in `chain_starts.tsv` are silently
MLE-ranked — no error, the fit proceeds. A user who suppresses/skims the warning
gets biased inits with no further signal. Strong observable signature (cited in
gh#110): on `wa_low_rho` seed-timing model, `chain_starts.tsv` for
`--seed 1`/`--seed 2` shows 1-2 of 6 chain inits pinned at parameter-space
bounds — a classic bound-pinned-MLE tell. Flat-prior fits are mathematically
unaffected (loglik == log_posterior up to a constant), but the warning still
fires (decorative noise).

**Blast radius**

Affects Bayesian fits (PGAS production method, PMMH experimental) seeded with
`--init survey_top_k` on any model with at least one non-flat prior — i.e.
essentially all real cVDPV2/policy fits, since the point of going Bayesian is
the prior encoding R₀ band / generation interval / vaccine efficacy. Direction
of harm: chain inits biased toward the likelihood ridge, away from prior mass.
Downstream effects on the posterior estimate: (a) long burn-in as chains
random-walk back into the prior's high-density region (wasted compute, possible
under-burned chains kept); (b) failure to mix / pseudo-convergence when the
surveyed MLE sits in a prior tail; (c) for bound-pinned MLEs, PMMH/PGAS may not
escape the bound (no proposal direction at a hard bound), so the reported
posterior can be a degenerate spike at the bound rather than the true posterior.
The final posterior mean/CI for affected parameters can be biased toward the
data-only optimum and away from the prior-informed value the modeler intended.
IF2 (MLE) stages are NOT harmed — they legitimately target the likelihood, so
survey_top_k is correct for them.

**Fix shape**

Two-step, per audit C1 / issue body. STEP A (producer, survey.rs): in
`resolve_survey_inputs` resolve each estimated param's prior via
`crate::fit::runner::resolve_prior(name, &config.estimate, &model)` and store
the `Vec<Prior>` (aligned to `resolved.estimated`) on `ResolvedSurveyInputs`
(add a field). In the eval loop / row build (`eval_point_pfilter` survey.rs:947,
`eval_point_simulate` survey.rs:1021, and `LandscapeRow` survey.rs:935-944),
compute `log_prior = Σ_i Prior::log_density(natural_i, transformed_i)` for the
drawn point and store it; add `log_prior` (and derived
`log_posterior = loglik + log_prior`) columns to the header/body in
`write_landscape` (survey.rs:1085-1104). Inline mode (no fit.toml) has no priors
→ emit `log_prior = 0`. STEP B (consumer, init.rs): extend
`LandscapeRow`/`parse_landscape_tsv` (init.rs:843-899) to read `log_posterior`;
change the sort key at init.rs:794-797 from `b.loglik` to `b.log_posterior`;
update `emit_top_k_se_warning` (init.rs:1000-1022) to aggregate spread on
log_posterior (note: SE on log_posterior == SE on loglik since the prior term is
deterministic — document this). Add a `prior_hash` to `SurveyCrossCheck`
(init.rs:826-840) + survey writer `inputs_json` (survey.rs:526-537), computed
over the resolved `[estimate]` priors (canonical serialization →
`crate::hashing::sha256_hex`), and check it in `cross_check_survey`; refuse with
a named error on mismatch. Thread `prior_hash` into `SurveyFitContext`
(init.rs:619-637). Drop `emit_rank_by_likelihood_bias_warning` (init.rs:801,
977-998) once B lands. This needs a small schema/doc note for landscape.tsv
columns and a golden re-bless of any committed survey landscape fixtures;
recommend a short proposal section (it bumps the survey artifact's column
contract and adds a cross-check), but the change itself is localized to
survey.rs + fit/init.rs. CHEAPER FALLBACK (correct-by-construction): in
`build_chain_starts_from_survey`, refuse `survey_top_k` when any estimated param
has a non-flat prior (requires threading the fit's resolved priors into
`SurveyFitContext`); flat-prior runs still benefit. This avoids the
producer-side change but disables the feature for the exact case (non-flat
priors) where users most want good starts.

**Risk**

Delicate points a reviewer must check: (1) Scale consistency —
`Prior::log_density(natural, transformed)` needs BOTH the natural value and the
unconstrained-scale z; the survey eval draws natural-scale `param_values`, so
the transform must be applied identically to the fit (`Transform::Log`/`Logit`),
or the TransformedNormal/Beta densities (prior.rs:104-127) will be miscomputed.
Reuse the fit's transform resolution, don't re-derive. (2) The fix must NOT
silently change behavior for flat-prior or IF2 fits — log_posterior==loglik
there, so ranks are unchanged; assert this in a test. (3) `prior_hash`
canonicalization must be stable (sort param order, round floats consistently) or
it will spuriously refuse valid survey↔fit pairs — mirror exactly how
`model_hash` is canonicalized. (4) Inline-mode surveys have no priors →
`log_prior` must be 0 and `prior_hash` must encode "flat for all," so an inline
survey can only seed a flat-prior fit (any non-flat fit must refuse). REQUIRED
TDD red test: profile/fit a model with a non-flat prior whose MLE sits ≥2σ from
the prior mode; assert that on current code the top-K inits cluster at the MLE
(RED), and after the fix at least one top-K init lies within ±1σ of the joint
prior mode (GREEN) — exactly the acceptance criterion in the issue. Also add a
regression asserting flat-prior ranks are byte-identical pre/post fix.

**Sequencing**

Independent of inference-math work; no upstream dependency. Recommend a short
proposal/RFC section before implementing because it (a) extends the committed
`landscape.tsv` column contract and (b) adds a new `prior_hash` cross-check to
the survey `run.json` `inputs` schema — both are durable artifact-format changes
(gh#51 survey-top-k design doc at
`docs/dev/proposals/2026-05-07-survey-top-k-init.md` should be amended).
Sequence within the fix: STEP A (writer emits log_prior/log_posterior +
prior_hash) must land before STEP B (consumer ranks on it and checks the hash) —
B reading a column A doesn't yet write would break parsing. Drop the
unconditional warning only in the same commit B lands. Related: gh#110 (PF
watchdog) handles the downstream symptom (bad init handed to PMMH); this issue
is the upstream cause — fixing #129 reduces but does not replace #110's
defenses. Golden/expected survey fixtures and their fingerprints will need
re-blessing (the recent commits 626b24ae/2450fb0a show landscape/manifest
digests are pinned).

**Collision / files touched**

Files the fix touches: `rust/crates/cli/src/survey.rs` (add prior resolution +
log_prior/log_posterior columns + prior_hash to inputs_json),
`rust/crates/cli/src/fit/init.rs` (LandscapeRow/parse_landscape_tsv, sort key,
SE warning, SurveyCrossCheck/SurveyFitContext prior_hash, drop bias warning).
Likely light touch / reuse only (no edit needed):
`rust/crates/cli/src/fit/runner.rs` (call existing `resolve_prior`),
`rust/crates/cli/src/hashing.rs` (reuse `sha256_hex`/`model_hash` pattern for
`prior_hash`), `rust/crates/sim/src/inference/prior.rs` (call existing
`Prior::log_density` — read-only). NO COLLISION with the other agent's
territory: the fix does NOT touch `rust/crates/sim/src/inference/*` (prior.rs is
only called, not modified), and does NOT touch `effects.rs` or `lifecycle.rs`.
Also requires re-blessing any committed survey golden/expected fixtures
(column-set change) and amending the gh#51 proposal doc.

---

## #147 — CAS cache-key soundness: model_hash omits output schedule / origin / time_unit; fit stale-reuse

**Effort:** M | **Confirmed live:** True

**What the code does today**

HEAD cb5fdd0f. The issue is a 3-claim bug filed before the content-addressed
run-identity refactor; that refactor has LARGELY LANDED, closing claims 1 and 2
(and 3) on every path EXCEPT one. State per path:

SOUND PATHS (refactor landed):

- Single-run `simulate` and main `batch run` (sweep/scenario) both go through
  `CasSink` -> `resolve::resolve_trajectory` (resolve.rs:161-191). Identity =
  whole-IR `model_digest` (resolve.rs:92-99, only `output.format` +
  `simulation.time_semantics` normalized out as pure presentation,
  resolve.rs:84-89) folded with a `config` level that DOES include
  t_end/output/dt/allow_degenerate_rates (resolve.rs:163-173). Cache-hit
  decision is `store.lookup(...)` which gates on `status==Completed` and a
  checksum exact-set: store.rs:215
  `if record.status != RunStatus::Completed { return Lookup::Stale(Incomplete) }`,
  store.rs:218 `check_exact_set`. Writes are atomic stage-then-rename + fsync
  (store.rs:1-18, commit_atomic). batch.rs:899-914 `CasSink::should_run` uses
  this lookup, not traj.tsv existence.
- Fit: `fit::cas::resolve_fit_stage` (cas.rs:343-379) keys on whole-IR
  `model_digest` (cas.rs:309) + `fit_config_blob_hash` (include-by-default,
  cas.rs:266-277) + a `deps` DAG. `StartsFrom::Stage` folds the upstream's
  run_id + consumed `fit_state.toml` content digest into deps via `cas_dep_ref`
  (cas.rs:424-432) wired at fit/mod.rs:660-664. So claim 2's stale-reuse is
  closed.

THE ONE LIVE PATH — `[design.*]` in `camdl batch run`: `run_design_experiment`
(batch.rs:1115-1266), self-labelled \"standalone and pre-date the v2 run-system
types\" (batch.rs:10), is NOT migrated. It calls the legacy `plan_runs`
(batch.rs:1201) whose cache decision is bare existence on the OLD key:
batch.rs:390-394:
`let traj_exists = Path::new(&format!(\"{}/traj.tsv\", run_dir)).exists(); let decision = if !force && traj_exists { RunDecision::CacheHit } else { RunDecision::CacheMiss };`
The path is built from `shash = sim_hash(&mhash, ...)` (batch.rs:1439) where
`mhash = model_hash(&ir_json)` (batch.rs:1435), and `model_hash`
(hashing.rs:20-60) hashes ONLY the incomplete allowlist: hashing.rs:31-35:
`[\"compartments\",\"transitions\",\"parameters\",\"tables\",\"time_functions\",\"interventions\",\"observations\",\"ode_equations\",\"initial_conditions\"]` +
`version`. It omits `output`, `simulation` (t_end/t_start), `origin`,
`origin_rata_die`, `time_unit`. Execution honours that decision: batch.rs:1214
`if plan.decision == RunDecision::CacheHit { ...; return; }`. On a miss it
writes non-atomically: `write_traj_tsv(...traj.tsv...)` (batch.rs:1246) then a
separate hand-rolled `std::fs::write(.../run.json, ...)` (batch.rs:1252-1258)
with no status, no checksum manifest, no fsync, no rename.

**Defect**

In the `[design.*]` branch of `camdl batch run`, the cache key (`shash`, derived
from the incomplete-allowlist `model_hash`) does not capture all inputs that
determine the trajectory, and the cache-hit test is bare `traj.tsv` existence
with a non-atomic write. Concretely: (1) two models differing only in output
cadence / t_end / calendar origin / time_unit hash equal -> the second design
run is silently served the first's traj.tsv; (3) a partially-written or aborted
traj.tsv is read back as a valid CacheHit because nothing verifies
status/checksum and the write is not atomic. (Claim 2 / fit stale-reuse and the
sweep+single-run sim variants of claims 1 & 3 are already fixed by the landed
CAS refactor — only the pre-v2 design path is still on the broken machinery.)

**Trigger / repro**

Repro for claim 1 (silent wrong trajectory): a batch TOML with a `[design.NAME]`
block over model A; run `camdl batch run exp.toml`; edit ONLY the model's
`output { ... }` cadence (or `simulation.t_end`, or the calendar
`origin`/`time_unit`) leaving compartments/transitions/params untouched; re-run
with the same `output_dir`. Because `model_hash` omits those fields, `shash` is
unchanged, the design cell's `traj.tsv` already exists ->
`RunDecision::CacheHit` -> the user silently gets model A's trajectory at model
B's request. No error, no warning. Repro for claim 3: kill `camdl batch run`
(design block) mid-write so `traj.tsv` is truncated; re-run ->
`traj_exists==true` -> CacheHit on a partial file. NOTE: `[sweep]`/`[scenario]`
batch runs and single-run `simulate` do NOT exhibit this — they route through
CasSink. The trigger is specifically a `[design.*]` experiment.

**Blast radius**

Affects VOI / sensitivity-analysis outputs only (the `[design.*]` feature;
batch.rs:1107 \"design-based experiment (VOI/sensitivity analysis)\"). A user
iterating on observation cadence, simulation horizon, or calendar framing while
re-running a design over the same output_dir gets stale trajectories for every
design cell -> downstream sensitivity indices / value-of-information summaries
computed on the wrong dynamics, in an undetectable way. For cVDPV2 strategy work
this is the class where horizon/reporting-cadence sweeps are plausible.
Direction is unbounded (whatever the stale model produced). The
corrupt-partial-file variant (claim 3) yields parse errors or garbage rows in
the design's summarize step. Sound paths (single sim, sweep batch, all fits,
survey, profile, pfilter) are unaffected.

**Fix shape**

Two viable shapes. (A) Preferred / aligned with the refactor: migrate
`run_design_experiment` (batch.rs:1115-1266) onto the same engine+CasSink path
the sweep branch already uses (batch.rs:634-706) — build a `SimulateJob` with
`ParamSource::Sweep{points: design_result.points}` and run
`engine::run_job(&job, &mut CasSink{...})`. This deletes the legacy
`plan_runs`/`RunPlan`/`RunDecision`/`sim_run_rel`/hand-rolled-run.json machinery
(batch.rs:324-411, ~1212-1264) and the `model_hash`/`sim_hash`/`shash` call
sites for design, inheriting whole-IR identity + atomic checksummed commit for
free. The dry-run preview (batch.rs:577-583) and summary counts
(batch.rs:1608-1622) should then derive hit/miss from `CasSink`/`store.lookup`,
not `plan_runs`, so the preview agrees with execution. After migration,
`model_hash`/`sim_hash`/`plan_runs`/`canonical_params`(scen use stays) likely
become dead and should be deleted per the repo's delete-dead-code rule. (B)
Stopgap (NOT recommended given the maintainer's \"one cleanup, no interim
stopgap\" note in the issue): widen `model_hash` allowlist and replace the
existence check with a status/checksum gate — but this re-implements CasSink
badly and the issue explicitly says land it as the refactor. Recommend (A).

**Risk**

Delicate points: (1) The design path currently writes a bespoke minimal run.json
carrying `design_point_index`/`scenario`/`seed` (batch.rs:1252-1258) that the
design `summarize` step parses — migrating to CasSink changes the on-disk layout
(store_path factored dirs + full RunRecord), so the design summarize/reader code
must be updated in lockstep or it will not find points. Verify what reads
`designs/{design}/.../run.json` before changing the writer. (2) Output-path
bytes: the sweep path is documented as \"byte-identical to single-run --cas\";
the design path uses `sim_run_rel` + `designs/{name}/` subtree — confirm the
migrated layout is intentional, not an accidental break of existing design
output dirs. (3) RNG/coupling unaffected (no change to draw order). TDD red
tests (mirror the issue's list, scoped to a `[design.*]` exp): model differing
only in output{}/t_end/origin/time_unit over a design block -> distinct cache
key / forced re-run (must FAIL today: stale CacheHit); partial traj.tsv with no
Completed run.json -> not a hit (must FAIL today). A pure-presentation `--dates`
change -> same key (guards over-invalidation). These belong as integration tests
over `batch run` with a design block, since the unit `plan_runs` tests
(batch.rs:1688-1774) currently bake in the broken existence semantics and would
need replacing.

**Sequencing**

No dependency on other issues; the CAS refactor it relied on has already landed
(this is the last unmigrated consumer). No new proposal needed — the existing
`docs/dev/proposals/2026-05-31-content-addressed-run-identity.md` is the
governing design and this is finishing it for the `[design.*]` path. Should land
as one cleanup commit (migrate + delete dead legacy helpers) per the maintainer
note in the issue body. Confirm the design `summarize`/reader code is updated
atomically in the same commit (see risk #1).

**Collision / files touched**

Files the fix touches: rust/crates/cli/src/batch.rs (run_design_experiment +
plan_runs/RunPlan/RunDecision deletion + dry-run/summary counts),
rust/crates/cli/src/hashing.rs (delete model_hash/sim_hash if they become dead),
rust/crates/cli/src/run_paths.rs (delete sim_run_rel if dead), and the design
summarize/reader (likely also in batch.rs or a sibling). NO files in
rust/crates/sim/src/inference/* are touched. NO effects.rs or lifecycle.rs
touched. NOTE: fit/mod.rs and fit/cas.rs are NOT touched by this fix (the
fit-side of claim 2 is already done) — flagging because the other agent may own
fit/inference files; this fix stays entirely in the cli batch/hashing/path
layer.

---

## #95 — Gillespie inhomogeneous-Poisson sampling still biased after 424b6a9a; bare-t fix only narrows the failure mode

**Effort:** L | **Confirmed live:** True

**What the code does today**

The SSA next-event time is drawn with lambda_total frozen at the value from the
previous event/boundary. rust/crates/sim/src/gillespie.rs:219-223:

// Draw time to next event ... let u1: f64 = stateful_rng.uniform(); let dt =
-(1.0 / lambda_total) * u1.ln(); let t_next = t + dt;

lambda_total is only updated (a) at output/intervention boundaries that CLIP an
event (gillespie.rs:264-272, re-evaluating model.time_dep_transitions), (b)
immediately after an event fires (gillespie.rs:365-385, recomputing
comp_to_transitions[fired] + time_dep_transitions), and (c) after interventions
/ periodic full recompute. There is NO thinning, NO upper-bound rejection, NO
fine re-draw within a single exponential waiting interval. Between two events
the propensity of a t-dependent transition is held constant at its left-endpoint
value.

time_dep_transitions is built at compiled_model.rs:622-631 from
expr_is_time_dependent (compiled_model.rs:195-224), which returns true only for
Expr::Time / Expr::TimeFunc (and transitively through
BinOp/UnOp/Cond/TableLookup/Reduce/BindingRef). It returns FALSE for Expr::Pop /
Expr::PopSum — so a rate that depends on a REAL compartment (ODE-evolved) is in
NEITHER comp_to_transitions (that's integer-only: collect_int_comp_deps gates on
global_to_int[global], compiled_model.rs:131-136) NOR time_dep_transitions.

On a fired event, gillespie.rs:307-311 advances real_s via rk4_step over the
whole interval dt, then sets t = t_next, but the post-event sparse update
(gillespie.rs:365-385) refreshes only integer-compartment-dependent and
time-dependent transitions — never real-compartment-dependent ones.
GillespieSim::capabilities() still advertises REAL_COMPARTMENTS
(gillespie.rs:43-45).

TODO markers acknowledging the gap: gillespie.rs:234 ("TODO(v0.2): replace with
PDMP thinning for real compartments") and gillespie.rs:306 ("TODO(v0.2): replace
with PDMP thinning"). The incident report
2026-05-20-...frozen-propensity.md:72-82 labels the boundary-only re-eval a
"piecewise-constant approximation on the output grid."

**Defect**

Gillespie samples an inhomogeneous Poisson process with a homogeneous sampler.
The waiting-time draw dt = -ln(u1)/lambda_total (gillespie.rs:222) is exact ONLY
if lambda_total is constant over [t, t+dt). For any rate that varies in t
between events — bare t, TimeFunc forcing, or a real compartment evolving by ODE
— lambda_total is held at its left-endpoint value, so both the inter-event
interval distribution and the transition-selection weights are systematically
wrong. 424b6a9a fixed only that t-dependent propensities are refreshed at
output/intervention boundaries (so they no longer freeze at t=0 for the entire
run); it did NOT make the within-interval sampling correct. Three nonhomogeneity
classes are mishandled, in descending coverage: (1) bare t — refreshed at
boundaries only, biased between them; (2) TimeFunc forcing — same boundary-only
treatment (it is in time_dep_transitions, but never re-sampled mid-interval);
(3) real-compartment-evolved rates — not tracked at all, frozen between events
with no boundary refresh and no dependency entry, yet the backend still claims
REAL_COMPARTMENTS.

**Trigger / repro**

Silent wrong numbers, no error. Concrete repros:

1. Bare-t seed pulse (the shipped seed-timing model): rate ~
   lambda/(1+exp(-(t-tau)/w)).
   `camdl simulate seed.ir.json --backend gillespie --dt 1 --seed 7 --param tau=30 --output g.tsv`
   vs `--backend chain_binomial`. Cross-backend total seed inflow disagrees; the
   test seed_timing_e2e.rs:145-146 only asserts rel < 0.30, so up to ~30% bias
   passes green. Coarsen the output grid (--dt larger / sparse output.times) and
   the bias grows because boundaries are the only refresh points.

2. Seasonal forcing: any SIR with beta(t)=beta0*(1+eps*cos(2*pi*t/365))
   (TimeFunc/spline). Gillespie holds beta at the previous-event value across
   each exponential interval; near forcing peaks/troughs the inter-event clock
   is biased.

3. Real-compartment rate: an ODE-evolved real compartment feeding a stochastic
   transition's rate. Between events the real comp moves (rk4_step) but the
   dependent propensity is never recomputed — worst case of the three, and
   capabilities() advertises support, so the model runs with no warning.

A user picks Gillespie precisely because it is documented as "exact" and gets
biased dynamics with no diagnostic.

**Blast radius**

Forward trajectories AND any inference that uses the Gillespie backend as its
proposal/likelihood model. For polio cVDPV2 seed-timing: the tau
(importation-time) posterior shifts — biased seed inflow propensity biases onset
timing and total inflow, which is exactly the quantity the seed-timing chapter
infers. Direction depends on rate convexity over the interval: for a rising
pulse/ramp (importation onset, vaccination ramp-down of immunity), the
left-endpoint rate UNDER-estimates the true integrated hazard, so events fire
too late / too few — biasing tau LATE and epidemic size DOWN. For a falling rate
the opposite. Affected model classes: anything time-inhomogeneous on Gillespie —
seasonal/Fourier/spline forcing, importation pulses, vaccination ramps,
time-varying contact, and (most severely) any real-compartment-coupled rate.
Chain-binomial / tau-leap / ODE are unaffected (they re-evaluate every substep).
Magnitude scales with how fast the rate moves relative to the inter-event
spacing and the output-grid coarseness.

**Fix shape**

Not a one-line fix; this is a sampler correctness change and warrants a design
proposal (the issue's fix part 3 + the upstream review both call for a thinning
/ modified-next-reaction implementation). Concretely, three landable pieces:

A) Correct sampler (the real fix): replace the frozen-rate exponential draw at
gillespie.rs:219-223 with thinning (Lewis/Ogata): maintain a per-interval upper
bound lambda_max >= sup over the candidate horizon, draw a proposal time with
lambda_max, re-evaluate true lambda_total at the proposal time, accept with prob
lambda(t')/lambda_max, else continue thinning. Requires a bound source per
nonhomogeneity class (analytic for monotone t-pulses; sup of TimeFunc over the
segment; for real compartments, bound via the RK4-advanced state or a
conservative envelope). Touches run_gillespie_with_observer in gillespie.rs only
(sampler loop + real-state advance at :236-240, :307-311).

B) Real-compartment dependency tracking (upstream review's structural rec): add
collect_real_comp_deps + real_comp_to_transitions: Vec<Vec<usize>> and
real_dep_transitions: Vec<usize> in compiled_model.rs (mirror
collect_int_comp_deps at :122-183 and the build loop at :622-631), then refresh
those in the gillespie post-event/boundary updates. UNTIL thinning lands, remove
REAL_COMPARTMENTS from GillespieSim::capabilities() (gillespie.rs:43-45) so
models with ODE-coupled stochastic rates are rejected rather than silently
biased.

C) Interim guardrail (issue fix parts 1-2): in the CLI backend dispatch,
hard-error (or W-warn) when time_dep_transitions is non-empty AND
backend==gillespie AND the output grid is coarser than ~generation_interval/10;
document the residual in the backend-picker UX, not just the incident report.
This is the cheap immediate mitigation while A/B are designed.

**Risk**

The thinning sampler reorders and adds RNG draws (acceptance/rejection), which
BREAKS paired-seed CRN coupling and changes every Gillespie golden trajectory —
update-expected must be regenerated and the determinism/coupling tests revisited
(CLAUDE.md: paired-seed CRN, not event-keyed RNG). A correct lambda_max is
load-bearing: too small a bound silently biases (thinning's correctness rests on
lambda_max >= true sup over the proposal horizon); a reviewer must verify the
bound is a true upper bound for each Expr class, especially real-compartment
envelopes. Removing REAL_COMPARTMENTS (interim) is a capability regression that
will reject currently-running models — acceptable per "no loose semantics" but
needs a clear E-code naming chain_binomial as the alternative. TDD red test: a
backend-agreement test on a sharply-rising bare-t pulse (or seasonal forcing)
with a TIGHT tolerance (e.g. rel < 0.03, well under the current 0.30) over the
seed_timing model — it must FAIL on current main and PASS after the thinning
fix. Tighten seed_timing_e2e.rs:145-146 from 0.30 to the post-fix tolerance as
the regression pin. Surface explicitly that this is inference-numerics: until
verified, mark "plausible, not verified."

**Sequencing**

Interim guardrail (C) can land immediately and independently — no proposal
needed (small gh-issue-scoped: CLI warning/error + doc). The correct sampler (A)
needs a docs/dev/proposals/ RFC first (thinning vs modified-next-reaction, bound
derivation per Expr class, RNG-stream/coupling impact, golden regeneration plan)
— it is a load-bearing inference-math change. Real-comp tracking (B) is a
prerequisite for A to handle class (3) correctly and can be split: ship the
capability removal (B-interim) with C now, defer the dependency-set + thinning
(A + B-full) to the proposal. The issue states the intended end-state is "one
Gillespie correctness commit covering all three nonhomogeneity classes" — but
that should be staged behind the proposal, with C/B-interim as the immediate
stop-the-bleed.

**Collision / files touched**

COLLISION FLAG: the primary fix sites are owned by another agent.
rust/crates/sim/src/gillespie.rs is NOT under inference/ but the issue is
labeled `engine`; effects.rs and lifecycle.rs are touched indirectly by the
boundary path (gillespie.rs:202-207, :253-258 call crate::effects::due_effects
and crate::lifecycle::apply_post_advance) — the sampler fix itself does NOT need
to edit effects.rs or lifecycle.rs, but be aware those modules are invoked.
Files the fix touches: rust/crates/sim/src/gillespie.rs (sampler loop +
real-state advance), rust/crates/sim/src/compiled_model.rs (new real-comp
dependency sets + build loop), rust/crates/sim/src/lib.rs (only if Capabilities
flags change semantics), rust/crates/cli/src/* (backend-picker warning/error —
locate dispatch), rust/crates/cli/tests/seed_timing_e2e.rs (tighten tolerance),
ir/expected/_.tsv (golden regeneration), docs/dev/incidents + docs/dev/proposals
(new RFC), docs backend-picker UX. None of the required edits are inside
rust/crates/sim/src/inference/_ — but confirm ownership of
gillespie.rs/compiled_model.rs with the other agent before editing, since they
own the adjacent effects.rs/lifecycle.rs seam this path calls into.

---

## #111 — [compiler] Indexed references lowered by string concat instead of dimension-aware resolver

**Effort:** L | **Confirmed live:** True

**What the code does today**

All EIndex/index handling routes through one helper that DISCARDS the dimension
label and treats every index positionally by list order.
`ocaml/lib/compiler/expander.ml:1051-1056`:

```
let index_item_to_str env item =
  match item with
  | IPosn (EIdent (s, _))     -> (match List.assoc_opt s env with Some v -> v | None -> s)
  | IPosn _                   -> "?"
  | INamed (_, EIdent (s, _)) -> (match List.assoc_opt s env with Some v -> v | None -> s)  (* label `_` dropped *)
  | INamed (_, _)             -> "?"
```

For INamed it binds the _value_ (`s`) and throws away the dimension _name_ — so
`S[patch = p1]` and `S[sex = female]` are indistinguishable from
`S[p1]`/`S[female]` and positioned by argument order, not by declared dimension.

The lowering sites then build concrete names by raw `String.concat "_"` and
never expand omitted dimensions to sums:

- Compartment refs in rate exprs, `expander.ml:1797-1800`:
  `let idx_vals = List.map (index_item_to_str env) items in let concrete = String.concat "_" (base_name :: idx_vals) in resolve_ident_name ctx concrete`.
- Table lookup `:1719-1735` — positional `List.nth tdims i` (also gh#-issue-2
  arity bug).
- Indexed let inline `:1740-1751`, shaped let `shape_index :1090-1096`.
- Indexed time/forcing func `:1782-1783`:
  `Ir.TimeFunc (String.concat "_" (base_name :: idx_vals))`.
- Indexed param `:1788-1795`.
- Stoichiometry `resolve_stoich_ref :2192-2194`.
- Observation projections `:4047-4054`
  (`ProjIncidence`→`Ir.CumulativeFlow concrete`,
  `ProjPrevalence`→`prevalence_projection`) and
  `incidence(EIndex)`/`prevalence(EIndex)` `:4058-4065`, `:4082-4090`.
- Interventions/events `ASet`/`AAdd` `:3922-3924`, `:3946-3948`.
- Init `:3226-3228` (positional-only patterns).

Resolution endpoint `resolve_ident_name :2107-2176`: a name not in
`expanded_comp_tbl` and not bare-in `comp_tbl` falls to E100 "undeclared name
'%s'" (`:2168-2174`). Crucially the _partial-index sum_ path used for BARE names
(`comp_tbl` → `PopSum expansions`, `:2124-2128`; `prevalence_projection` bare →
`CurrentPopSum`, `:4023-4028`; `incidence_projection` bare →
`CumulativeFlowSum`, `:4040-4044`) is NEVER reached for an indexed ref, because
the indexed paths pre-concatenate the supplied indices into one name before
resolving. Parser accepts the syntax:
`parser.mly:407 | name = IDENT EQ e = expr { INamed (name, e) }`, ast.ml:38-40.
No golden fixture exercises named indexing (rg over ocaml/golden +
tests/fixtures: 0 matches).

**Defect**

Two compounding mechanism failures vs `docs/camdl-language-spec.md` §5.1 (lines
752-770) and §12.1 (2155-2225), which mandate (a) named indices bind by
dimension NAME, order-independent, and (b) omitted dimensions SUM. (1)
Named-index label is discarded (`index_item_to_str` matches `INamed (_, ...)`),
so a named index is treated as positional-by-list-order — wrong dimension
binding when index order != declaration order, and a label
typo/dimension-mismatch is never validated. (2) No omitted-dimension summation
for indexed refs: the supplied indices are string-concatenated into one partial
name and looked up directly, so a partially-indexed ref produces a non-existent
partial name (e.g. `S_p1` / `infection_kano`) instead of the spec-required sum
over the omitted strata (`PopSum`/`CurrentPopSum`/`CumulativeFlowSum`). The
compiler is both too permissive (silent wrong binding) and too strict (rejects
valid partial indexing) at once.

**Trigger / repro**

Model with `dimensions { patch, age }`, `stratify(by = patch)`,
`stratify(by = age)` (expanded compartments `S_p1_child`, `S_p1_adult`, ...).
Failure modes, all reproducible by `camdl simulate model.camdl`:

- `... @ beta * S[patch = p1] / N` → `index_item_to_str` yields `"p1"`,
  `concrete = "S_p1"`, which is in neither `expanded_comp_tbl` nor `comp_tbl` →
  hard error E100 "undeclared name 'S_p1'" (a name the user never wrote). A
  valid spec model is REJECTED, post-expansion, with a confusing diagnostic.
- Observation `projected = incidence(infection[patch = p1])` in patch×age model
  → `expander.ml:4058-4059` emits `Ir.CumulativeFlow "infection_p1"`; that
  transition name doesn't exist (real names are `infection_p1_child`,...) →
  likelihood fails to compile (post-expansion E507) instead of summing the
  patch's age-strata flows.
- `S[sex = female, age = child]` when stratify order is `[age, sex]` → positions
  become `["female","child"]`, `concrete = "S_female_child"` ≠ real
  `S_child_female` → E100; OR, if level names collide across dimensions, binds a
  DIFFERENT real compartment SILENTLY (wrong number, no error). Single-dimension
  named indexing (`S[patch = p1]` with only `[patch]`) works by luck (value
  happens to land in the one slot). User-visibility: mix of late hard errors and
  silent wrong bindings — the silent case is the dangerous one.

**Blast radius**

Hits any multi-dimensional (spatial / age × sex / patch × age) model — the core
polio cVDPV2 use case (per-patch surveillance streams). Scientific outputs that
move: (1) Observation likelihoods attached to a wrong or non-existent flow →
per-patch incidence likelihood scores wrong, shifting the entire posterior in
PGAS/IF2/pfilter for `beta`, reporting rate, and importation parameters —
direction unbounded (could attach data to one stratum's flow, biasing
transmission high or low). (2) Force-of-infection rate terms reading the wrong
compartment sum → wrong trajectory amplitude/timing per stratum. (3) Initial
conditions and intervention/event targeting (`set`/`add`/`transfer` with named
index) hit wrong or partial compartments → wrong scenario impact estimates.
Single-dimension models and bare (un-indexed) references are unaffected (bare
names already sum correctly via the
`comp_tbl`/`CurrentPopSum`/`CumulativeFlowSum` paths). Net: the failure is
concentrated exactly in the stratified models that matter most for spatial polio
strategy, and frequently surfaces as a late compile error rather than a wrong
number — but the dimension-collision case is a silent wrong number.

**Fix shape**

Build the single dimension-aware resolver the audit and issue prescribe, all
inside `ocaml/lib/compiler/expander.ml` (no schema change; emits existing IR
variants
Pop/PopSum/TableLookup/TimeFunc/Param/CumulativeFlow/CumulativeFlowSum/CurrentPop/CurrentPopSum).
Concretely: add `resolve_indexed_ref ctx env ~namespace ~base items` that (a)
looks up the object's declared dimension vector
(`comp_dims`/`table_dims`/forcing+param dims), (b) maps each
`INamed (dim, EIdent v)` to its dimension by name and validates membership
(reuse `dim_values`/`dim_value_index`, emit E263-style on bad dim or level), (c)
maps `IPosn` by declaration order, (d) rejects unknown/duplicate dimension
labels and over-arity with a real diagnostic, (e) for omitted dimensions,
cartesian-expands them and emits the SUM variant appropriate to the namespace
(PopSum / CurrentPopSum / CumulativeFlowSum), single-element collapsing to the
non-sum variant. Then replace every `index_item_to_str` + `String.concat "_"`
call: EIndex case `:1711-1800`, `resolve_stoich_ref :2192-2194`,
`expand_observations` projections `:4047-4090`, `expand_init :3226-3228`,
intervention `ASet`/`AAdd` `:3922-3948`. Delete `index_item_to_str` once
unreferenced. NOTE: the issue/audit recommend a typed `resolved_ref` sum type;
that is a structural refactor and the audit explicitly folds Highs #9/#10/#12 +
parts of #13 into it — so this should be implemented against a short design
proposal (per CLAUDE.md "Bigger lifts → docs/dev/proposals/"), not a drive-by
edit. Table-arity validation (audit #2) can ride along but is logically
separable. TDD: add red goldens for `S[patch = p1]` (expect PopSum over age),
`incidence(infection[patch=p1])` (expect CumulativeFlowSum),
`S[sex=female, age=child]` order-independence, and a label-typo rejection.

**Risk**

High-blast, math-adjacent (rate expressions and observation likelihoods),
touched by a single helper that fans out to ~8 call sites — easy to miss one and
leave a string-concat path live. Delicate points a reviewer must check: (1) sum
ORDER must match the OCaml Add-chain / PopSum convention so the IR is
byte-stable and dimcheck/normalize_expr agree (CLAUDE.md notes Reduce/PopSum
left-fold ordering); the omitted-dimension cartesian expansion must match
`expand_compartment_name`'s row-major order (`:826-832`). (2) Hoisting
interaction: indexed-let path `:1757-1762` registers hoisted bindings keyed by
`String.concat "_"` concrete names — the resolver must produce identical keys or
break hoist dedup. (3) `prevalence_projection`/`incidence_projection` already
implement the bare-name sum correctly — fold them INTO the resolver rather than
leaving two truth sources. (4) Must not regress single-dim positional models
that currently work by luck. Red-test that proves the bug: compile a 2-dim
`[patch, age]` model with `beta * S[patch = p1]` and assert it currently errors
E100 'undeclared name S_p1' (today's behavior), then assert the fixed compiler
emits `PopSum [S_p1_child; S_p1_adult; ...]`. No tolerance/gate to weaken.

**Sequencing**

Needs a design proposal first: the audit's "Structural fix" section
(`docs/dev/reviews/2026-05-26-upstream-ocaml-compiler-review.md:526-555`)
explicitly makes this the umbrella change that subsumes upstream Highs #9
(`c in compartments` omitted-dim fill), #10, #12, and parts of #13 — i.e.
several other open upstream-audit issues are downstream of this resolver. Decide
the bundling (do #9/#10/#12 land together, or is this resolver landed first as
the substrate?) before implementation. The table-arity validation (audit #2 /
its own issue) shares the lowering surface and should be sequenced adjacent
(same PR or immediately after) to avoid two rewrites of the table path. No
dependency on inference-side changes. Per CLAUDE.md, gate with `make test` and
add golden fixtures (there are currently zero named-indexing goldens) as part of
the change.

**Collision / files touched**

Single file: `ocaml/lib/compiler/expander.ml` (functions `index_item_to_str`,
`resolve_expr`/EIndex case, `shape_index`, `resolve_stoich_ref`,
`resolve_ident_name` helpers `prevalence_projection`/`incidence_projection`,
`expand_observations`, `expand_init`, `expand_scheduled_actions`). Likely also
`ocaml/lib/compiler/diagnostics.ml` (new/extended E-codes) and new golden
fixtures under `tests/fixtures/` + `ir/golden/`. NO collision with the other
agent's surface: nothing in `rust/crates/sim/src/inference/*`, `effects.rs`, or
`lifecycle.rs` is touched — this is pure OCaml-frontend lowering, emitting
existing IR variants the Rust side already consumes. If the structural
`resolved_ref` type is added, it stays internal to expander.ml (not the
serialized IR), so no `ir/schema.json` change either.

---

## #119 — [engine] Parameterized tables and forcing functions are frozen at model construction

**Effort:** L | **Confirmed live:** True

**What the code does today**

Inline-table and time-function Expr fields are evaluated ONCE at
CompiledModel::new using `default_params` and stored as plain f64 caches. The
line numbers shifted from the issue body but the defect is identical.

compiled_model.rs:667-674 (tables):

```rust
let mut table_values_cache: Vec<Vec<f64>> = ...;
for table in &model.tables { match &table.source {
  Inline { values } => {
    let vals = values.iter()
      .map(|expr| eval_table_expr(expr, &param_index, &default_params)).collect();
    table_values_cache.push(vals?);
```

compiled_model.rs:698-702 (sinusoidal forcing — same for
Piecewise/Interpolated/Periodic/Fourier/PeriodicSpline at :704-781):

```rust
Sinusoidal(s) => CompiledTimeFuncKind::Sinusoidal {
  amplitude: eval_table_expr(&s.amplitude, &param_index, &default_params)?,
  period:    eval_table_expr(&s.period,    ...)?,
  phase:     eval_table_expr(&s.phase,     ...)?,
  baseline:  eval_table_expr(&s.baseline,  ...)?, },
```

`default_params` is built at :554-561 from each parameter's `value` field; a
param with `value: null` (every inference target) makes new() error there, so
the freeze bites only after `--params`/proposals supply values.

Read sites ignore `ctx.params`. Slow path propensity.rs:182-204:

```rust
Expr::TimeFunc(w) => Ok(eval_time_func(&ctx.model.time_func_cache[idx].kind, ctx.t)),
Expr::TableLookup(w) => { let cached = &ctx.model.table_values_cache[idx]; ... table_lookup(table, cached, table_idx) }
```

Fast path resolved_expr.rs:472-477 is identical
(`eval_time_func(&ctx.model.time_func_cache[*idx].kind, ctx.t)`;
`&ctx.model.table_values_cache[*table_idx]`). EvalCtx DOES carry
`pub params: &'a [f64]` (propensity.rs:17) — the proposed vector is present but
never consulted by these two arms.

Compounding bug — gradients are also zeroed. Rust eval_expr_deriv
(propensity.rs:253-255) and eval_resolved_deriv (resolved_expr.rs:619-620)
return 0.0 for TimeFunc/TableLookup. The OCaml source-to-source autodiff that
emits `rate_grad` does the same unconditionally: autodiff.ml:23-24
`TimeFunc _ -> Const 0.0 | TableLookup _ -> Const 0.0` (invariant comment at
:7-10 asserts they "are data").

**Defect**

A parameter referenced inside a forcing field
(amplitude/phase/baseline/period/harmonics/coefs) or an inline table entry is
baked into `time_func_cache`/`table_values_cache` at construction. When
inference (IF2/PGAS/PMMH) reuses the single CompiledModel and proposes new
parameter values, the propensity evaluator reads the frozen cache and the
autodiff `rate_grad` carries no derivative w.r.t. that param. Two failure modes
stack: (1) the likelihood is exactly flat along that parameter's axis (value
frozen), and (2) the NUTS gradient component is identically 0 (autodiff zeroes
TimeFunc/TableLookup). The mechanism is "data present in EvalCtx.params, never
read by the TimeFunc/TableLookup arms" plus "OCaml differentiate treats
forcing/table as a constant regardless of whether its body mentions an estimated
param."

**Trigger / repro**

Live golden fixture `ir/golden/seir_vaccine_seasonal.ir.json` reproduces it. Its
`seasonal` forcing has `amplitude: {param: alpha}` and
`phase: {param: phi_season}` (:176,178); both `alpha` and `phi_season` are
estimated (value:null). The `infection` transition rate is
`beta*seasonal*S*I/N`, yet its `rate_grad` map keys are ONLY `[beta]` — `alpha`
and `phi_season` are absent (verified by parsing the JSON).
`seir_seasonal_patch.ir.json` (amp_urban/amp_rural) and
`tests/fixtures/corner_cases/ir/seasonal_drift.ir.json` (amplitude=alpha) are
further instances. Repro: fit either model over alpha with PGAS+NUTS or IF2.
User sees NO error — a healthy-looking run with a silently wrong number: the
reported posterior/CI for alpha equals the prior (or the supplied default), and
chains mix, ESS is high, R-hat is good. The construction-time error only fires
if you forget to supply a value at all.

**Blast radius**

Scientific output: any posterior/MLE over a parameter that lives inside a
forcing function or a table entry is invalid. Directionally, the estimate
collapses to the prior mean / supplied default along that axis; signal that
should constrain it instead leaks into co-varying non-forcing parameters (e.g.
baseline transmission `beta` absorbs seasonal-amplitude misfit), biasing THOSE
estimates too. Affected model classes: seasonal-forcing SEIR/SIR (inferred
seasonal amplitude/phase — central to cVDPV2 transmission seasonality),
reporting-ramp / time-varying reporting models, spatial/metapopulation models
with inferred coupling or contact-matrix table entries (seir_seasonal_patch
patch amplitudes), age-structured models with parameterized contact tables.
Forward simulation with fixed params is unaffected (default_params == the params
you'd evaluate with). The damage is specific to inference and to any sweep that
changes a forcing/table param without rebuilding CompiledModel.

**Fix shape**

Two coupled changes; both required for correctness. (A) Runtime value: stop
storing parameter-dependent forcing/table fields as f64. In compiled_model.rs,
classify each field at new() as Constant(f64) when its Expr has no Param ref,
else Parametric(ResolvedExpr) (use the existing `expr_refs_param` predicate
already in this file at the bindings guard, and `resolve_expr`/ResolvedExpr from
resolved_expr.rs). Replace the `time_func_cache: Vec<CompiledTimeFunc>` and
`table_values_cache: Vec<Vec<f64>>` fields with enum-tagged variants (issue's
`CompiledTableValues`/`CompiledTimeFuncField`). In propensity.rs:182-204 and
resolved_expr.rs:472-477, evaluate Parametric fields against
`ctx.params`/`ctx.t` on the hot path (eval the ResolvedExpr children, then apply
eval_time_func / table_lookup); keep the Constant fast path. Sinusoidal etc.
need their scalar fields resolved per-call when parametric. (B) Gradient: fix
the autodiff. OCaml autodiff.ml:23-24 must, when the forcing/table body mentions
the diff param, differentiate through it (chain rule through amplitude*sin(...)
etc.) rather than returning Const 0.0 — the `mentions p` helper already inlined
at :95-106 detects the case. Mirror in Rust eval_expr_deriv
(propensity.rs:253-255) and eval_resolved_deriv (resolved_expr.rs:619-620). If
full symbolic differentiation of every forcing kind (Fourier/spline) is too
large, the minimum-safe interim is a HARD COMPILE ERROR (like the Mod guard at
autodiff.ml:108-114) rejecting an estimated param inside a forcing/table whose
derivative is not yet implemented — never silently emit a zero gradient. This is
large enough to warrant the structural proposal the audit names ("split
CompiledModel into structure + ResolvedModel + EvaluationContext"); a localized
enum-tag fix is viable for (A) but (B) touches the IR autodiff contract, so
write it up as a proposal first. Sibling finding #2 (chain-binomial real state)
lands in the same refactor.

**Risk**

Delicate: (1) The autodiff change touches inference math (rate_grad is the
contract NUTS consumes) — high-risk per CLAUDE.md; any error here silently
corrupts gradients rather than crashing. Must re-bless `rate_grad` in ALL golden
IR (seir_vaccine_seasonal, seir_seasonal_patch, seasonal_drift) and add
gradient_check coverage analogous to
gradient_check.rs/gradient_check_overdisp.rs comparing analytic vs
finite-difference d(rate)/d(alpha). (2) RNG-order coupling: making forcing/table
eval parametric must NOT change the number/order of RNG draws (paired-seed CRN,
per CLAUDE.md) — the Constant fast path must be byte-identical to today so
gate_trajectory_baseline / determinism tests stay green for non-parametric
models. (3) Bless the existing f64-cache golden expecteds only where values are
genuinely unchanged. TDD red-test (issue's Medium #17): build ONE CompiledModel
from seir_vaccine_seasonal, run eval_propensities/complete_data_loglik with two
params slices differing only in `alpha`, assert propensity AND loglik differ —
this FAILS today (frozen) and must pass after (A). Separately assert rate_grad
w.r.t. alpha is nonzero (fails today, passes after B). No such test exists: all
forcing tests (periodic_forcing.rs, fourier_oracle.rs) use `parameters: vec![]`.

**Sequencing**

Independent of other open issues for part (A). Part (B) shares the IR
`rate_grad` surface; the autodiff change should land with or after part (A) (a
correct gradient is useless if the value is still frozen). Recommend a short
proposal under docs/dev/proposals/ before implementing because it changes the
autodiff contract and the CompiledModel field layout, and because the audit
explicitly frames this as half of a structure/ResolvedModel/EvaluationContext
refactor co-landing with sibling finding #2 (chain-binomial real state, issue's
"#2"). Required follow-up named in the issue: audit every saved fit since
parametric forcing/tables were introduced and flag invalidated posteriors. No
upstream dependency must land first.

**Collision / files touched**

Fix touches: rust/crates/sim/src/compiled_model.rs (field types + new()
classification), rust/crates/sim/src/propensity.rs (TimeFunc/TableLookup eval +
eval_expr_deriv), rust/crates/sim/src/resolved_expr.rs (ResolvedExpr eval +
eval_resolved_deriv), ocaml/lib/ir/autodiff.ml (differentiate
TimeFunc/TableLookup), and regenerated goldens
(ir/golden/seir_vaccine_seasonal.ir.json, seir_seasonal_patch.ir.json,
tests/fixtures/corner_cases/ir/seasonal_drift.ir.json) plus new sim tests. NONE
of the primary fix files are in rust/crates/sim/src/inference/, effects.rs, or
lifecycle.rs. FLAG: validating the fix requires running/asserting through
inference call sites (pgas.rs build_obs_at_substep / complete_data_loglik at
:576, if2.rs process.step at :421, pgas_grad.rs rate_grads consumption at
:32/:66) — those files are inference/-owned by the other agent. Adding the
red/green regression test will likely add a new test file (not edit
inference/*), but if the gradient assertion reaches into pgas_grad helpers,
coordinate. effects.rs/lifecycle.rs are NOT touched.
