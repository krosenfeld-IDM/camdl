# Unifying the scheduling surfaces

Status: Proposed. Option A (frontend `schedule_core`) is specified to
implementation level below and recommended; Option B (the *factored*
IR-type merge) is the long-term design, deferred. No code yet.

## Problem

"When does this happen?" is asked in four places, each with its own
grammar rule and IR type:

| surface | IR type | shared core | surface-specific extension |
|---|---|---|---|
| observations | `observation_schedule` | `every = E`, `at = [...]` | `ObsFromData` (read times from the data file) |
| output | `output_schedule` | `every = E`, `at = [...]` | `OutMatchObservations` (align to obs times) |
| interventions | `intervention_schedule` | `at = [...]`, `every = E from..to` | `AtTimesExpr` (parametric `at [t_seed]`, gh#69), `at_day` |
| events | (recurring/`SAtTimes`) | `at [...]`, `every … at_day` | `at_day` |

Plus two byte-identical records: `regular_obs_schedule` and
`regular_output_schedule` are both `{ start; step; end_ }`.

The vocabulary is *already* consistent at the token level (`EVERY`/`AT_KW`
are shared), but the parsing rules and IR types are duplicated. The goal:
keep the UX consistent by construction and cut duplication, without
introducing bugs in inference-critical code.

## The catch: a shared core wrapped in divergent extensions

The four surfaces share a genuine common core — `every = E` and
`at = [...]` mean the same thing everywhere — but each has an extension
that does **not** generalize:

- `from_data` is meaningful only for observations (the data file *is* the
  source); an intervention has no data file.
- `match_observations` is meaningful only for output (align output to obs
  times); for observations it would be circular.
- `at_day` (fire on a day-of-period) and parametric `at [t_seed]`
  (`AtTimesExpr`, gh#69, resolved against the param vector at sim start)
  are intervention/event concepts.

So a single unified type is not "the same thing three times." It is
`{ every, at } + (from_data | match_observations | at_day | parametric)
+ a validity matrix` stating which extension is legal in which context.
That matrix is itself complexity and a bug surface; it may not be a net
simplification over three small, honest types.

And the *semantics* around the times differ, not just the times:
observations are **sampled**, output is **snapshotted**, interventions
**fire deterministically mid-substep and perturb propensities** — and the
paired-seed CRN coupling depends on RNG-consumption order at intervention
times. The schedule is a thin shared notion sitting on top of three
different evaluation models.

## Option A — unify the frontend surface (recommended)

One shared frontend type + grammar rule for the common `every`/`at`,
lowered per-surface in the expander to each construct's *existing* IR
variant. Today the observation and output schedule ASTs are literally the
same two-constructor type under different names:

```ocaml
(* before — duplicated *)
type obs_schedule         = ObsEvery of expr | ObsTimes of expr list
type output_schedule_spec = OtEvery  of expr | OtAt    of expr list

(* after — one shared type *)
type schedule_core =
  | SchedEvery of expr        (* every = E      *)
  | SchedAt    of expr list   (* at = [t1, ...] *)
```

Parsed once:

```
schedule_core:
  | EVERY EQ e = expr                                          { SchedEvery e }
  | AT_KW EQ LBRACKET ts = separated_list(COMMA, expr) RBRACKET { SchedAt ts }
```

Each construct reuses it and keeps its own extension rule; the expander
lowers `schedule_core` to the existing IR variant:

- **observations** use the full core (`SchedEvery`→`ObsRegular`,
  `SchedAt`→`ObsAtTimes`); `from_data` stays its own rule (it is a derived
  source, not a schedule).
- **output** use the full core (`→ OutRegular` / `OutAtTimes`); `format`
  stays its own field.
- **interventions / events** reuse only the `SchedAt` arm (explicit times).
  Their `every` is *windowed* (`every = E from F to T`) or day-of-period
  (`at_day`) — richer than the bare core, so it stays a separate variant.
  Do **not** let interventions carry the whole `schedule_core`, or
  `SchedEvery` (a windowless cadence they don't support) becomes a
  representable illegal state — the very smell Option B's naive form has.

Why no Rust/schema change: the expander emits the same IR variants it does
today, so the serialized IR is byte-identical. The Rust enums
(`ObservationSchedule` / `OutputSchedule` / `InterventionSchedule`) and
`schema.json` are untouched — their triplication is what *Option B* would
collapse, not A.

Migration order, asserting byte-identical golden IR at each step: output
(already `OtEvery`/`OtAt`) → observations (delete `obs_schedule`) → the
`at` arm of interventions/events. Behavior-preserving; gated by the
existing golden + integration suite.

Wins: `every`/`at` parse identically by construction (a fifth surface can't
drift), and two identical AST types plus their parsing collapse to one.
Cost: a frontend refactor, no contract exposure.

The adjacent `regular_obs_schedule` / `regular_output_schedule` record
merge is *not* part of A — it is an IR-type change, so it rides with B.

## Option B — merge the IR types (defer; the *factored* design, not a fat type)

The naive merge — one `Schedule` type carrying every variant with
per-context validation rejecting the illegal ones — is a worse design: it
widens the type so `output { from_data }` is representable, then leans on a
runtime validity matrix, violating "make illegal states unrepresentable."
Don't build that.

The *right* merge factors two axes the current types conflate — **specified
times** (`every`/`at`/parametric, genuinely shared) vs **derived source**
(`from_data`, `match_observations`, which are not schedules at all):

```ocaml
type schedule = Every of {…} | At of expr list | AtExpr of expr list  (* shared *)

(* observations *)  source = Specified of schedule | FromData
(* output *)        source = Specified of schedule | MatchObservations
(* interventions *) source = Specified of schedule   (* + at_day / windowing on the regular case *)
```

Now the shared `schedule` is clean (no validity matrix) and each surface's
one derived-source variant lives where it belongs. This is good ADT design
and probably the right long-term shape.

The boundary that keeps it honest: **share the *times*, never the
*evaluation*.** The schedule answers "when"; the surfaces differ in "what
happens then" — sampled vs snapshotted vs fired-with-propensity-effects,
where the paired-seed CRN coupling and PGAS conditioning live. Unify the
`schedule` type; do not push the unification down into shared evaluation.

Why defer it anyway — none of these is "it is a bad abstraction":
- **Execution risk.** A cross-language IR schema change (OCaml types + the
  three Rust enums + `schema.json` + bump + full golden regen) reaching
  `intervention.rs` / `pgas.rs` / `particle_filter.rs` — surfaces CLAUDE.md
  flags high-risk regardless of how mechanical the edit looks.
- **The factoring needs care.** The naive fat-type version is a real trap;
  separating the two axes is design work best done deliberately, not under
  refactor momentum.
- **No forcing function.** No incident on record is *caused* by the current
  duplication; nothing breaks if we wait. Electing this risk now buys
  long-term tidiness, not a fixed bug.

**Sizing (from the Rust consumer survey).** The type change is small; the
lift is breadth. Real *logic* consumers are few — `output_times`
(`output.rs`), intervention fire-times (`intervention.rs`), `AtTimesExpr`
resolution (`compiled_model.rs`), `resolve_output` (`resolve.rs`), and the
obs schedule→times path — a couple inference-adjacent (fire-times feed PGAS
event density; obs times set what PGAS conditions on). The bulk is
mechanical: dozens of inline construction sites (`OutputConfig { times: …
}` / `schedule: …`, mostly in tests) re-shape under the factored `source`
wrapper. The sharp edge is **run identity**: `runid/src/ir_hash.rs`
hand-hashes each schedule type into the `run_id` (a `ContentAddressed` impl
per type, `header` + fields). B must keep the emitted hash bytes
byte-identical or every `run_id`, CAS path, and golden `run_id` churns —
make it an explicit equivalence test, not an assumption. Plus a full golden
IR regen (schedule JSON tags change) reviewed to confirm only serialization
moved, not semantics.

If taken, B should also settle: do *all* surfaces gain parametric `at`
(gh#69) and `at_day`, or does the type carry per-context legality? Its own
review, not a default.

## Recommendation

1. Land **Option A** (frontend `schedule_core`) per the plan above — low
   risk, byte-identical IR, no Rust/schema/inference exposure, most of the
   UX win. It is a strict stepping-stone toward B: the shared frontend core
   is what B later promotes into the IR.
2. Hold **Option B** (the *factored* design, not the fat type) until a
   concrete problem justifies it — a real inconsistency bug, or a feature
   that needs one IR type. Then do it as its own schema change with
   byte-equivalence golden coverage, settling the parametric-`at` / `at_day`
   question first.
3. Treat "reduce bugs" as the test, not the slogan: the change that most
   reduces bug risk *right now* is the one that doesn't touch the live,
   tested, inference-critical IR schedule types.
