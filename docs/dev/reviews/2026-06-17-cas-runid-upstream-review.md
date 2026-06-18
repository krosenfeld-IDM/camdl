# Review response: CAS / run-identity upstream review (gh#241)

- Date: 2026-06-17
- Reviewing: gh#241 PRs B–F (`c607693f` retire legacy model/fit digests, back
  through `0bc04f8f` backend domain types)
- Tree state at review: `1367b7e7`

Notes on an upstream review of the run-identity refactor. Each finding was
checked against the code at `1367b7e7`; the verifying command/line is pasted
inline. Classification follows `CLAUDE.md` (doc-vs-doc / doc-vs-code /
code-vs-code).

## Summary

| # | Finding                                                                                      | Class        | Verified | Live?                      | Action              |
| - | -------------------------------------------------------------------------------------------- | ------------ | -------- | -------------------------- | ------------------- |
| 1 | Optional child outputs (`obs/`, `--event-log`) not CAS-safe                                  | code-vs-code | yes      | yes (acknowledged interim) | follow-up issue     |
| 2 | `model_identity_from_ir` returns `""` on parse failure, shared by a load-bearing cross-check | code design  | yes      | no (latent)                | recommend split     |
| 3 | Legacy-identity guard bans 2 of 5 retired names; stale `model_identity_hex` doc refs         | doc-vs-code  | yes      | no                         | **fixed this pass** |
| 4 | Run spec still documents `sim_hash`/`scen_hash`/`model_hash` as the identity scheme          | doc-vs-code  | yes      | no (misleads agents)       | follow-up doc pass  |

The "what looks solid" observations all check out (see bottom).

## Finding 1 — optional child outputs are not CAS-safe (confirmed, interim)

The skip decision keys on the parent `Sim` leaf alone:

```
batch.rs:1091  fn should_run(&mut self, spec) -> bool {
batch.rs:1099      store.lookup(&dir, &runid::LeafIdentity::new(rt.run_id)) ... Hit
```

`lookup` → `check_exact_set` → `scan_orphans` treats a declared child as a
boundary it does **not** validate:

```
store.rs:683   if dir == root && record.children.contains_key(&name) {
store.rs:684       continue; // declared child boundary — its own leaf
```

So a parent traj leaf returns `Hit` regardless of whether its `obs/` child is
present, partial, or absent. The obs child is declared in the parent record
(`batch.rs:1169`), the parent commits atomically (`batch.rs:1210`), and obs is
written _after_ commit with a non-fatal failure path:

```
batch.rs:1229  if has_obs {
batch.rs:1230      if let Err(e) = write_obs_into_cas(...) {
batch.rs:1231          self.errors.push(...);   // non-fatal — parent already committed
```

The obs "child" is not yet a real `RunRecord`-backed leaf — `batch.rs:1156-1158`
calls it "M2-interim … a full obs-child RunRecord identity is a follow-up" — so
it has no `run.json` to look up independently either. `--event-log` carries the
same limitation, documented at `main.rs:1249-1254`.

**Net effect:** a re-run can report `skip (cache hit)` while requested
`--obs`/`--event-log` output is absent or partially written. That is a silent
gap against camdl's "no silent gaps" bar, even though the code flags it as
interim. The reviewer's fix — make obs/event-log real child `ResolvedArtifact`s
keyed by parent `run_id` + child inputs, resolved/looked-up/written
independently on a parent cache hit — matches the existing in-code TODOs and is
the right shape.

**Action:** file as a tracked issue (store-protocol follow-up). Not a quick
cleanup; it is a deliberate extension of the child-artifact protocol.

## Finding 2 — `model_identity_from_ir` too forgiving for a cross-check (confirmed, latent)

```
resolve.rs:140   Err(_) => String::new(),
```

The same helper feeds both best-effort display sidecars and the survey→fit
warm-start safety cross-check:

- display: `survey.rs:563`, `profile.rs:843` (recorded-not-hashed mirror)
- cross-check: `fit/mod.rs:1012-1021` builds `SurveyFitContext.model_identity`
  from `model_identity_from_ir(...)`, consumed by `cross_check_survey`:

```
fit/init.rs:909   if meta.model_identity != ctx.model_identity {
```

If both the survey-side and fit-side IR fail to parse, both sides are `""`, and
`"" == ""` lets the cross-check pass — a silent degradation of a semantic guard.
Today every cross-check site passes freshly compiled, valid IR, so this is **not
a live bug**; the hazard is that the API makes the silent-degrade path one
refactor away, in exactly the high-risk warm-start surface.

**Action (recommend):** split per "parse, don't validate" —
`try_model_identity_from_ir(...) -> Result<String, _>` for the cross-check
(survey/fit), `best_effort_model_identity_from_ir(...) -> String` for
display-only sidecars. Mechanical but touches ~9 call sites of a load-bearing
helper, so it is a scoped change, not a drive-by. Deferred pending sign-off.

## Finding 3 — guard contract + stale doc refs (confirmed; **fixed this pass**)

Two sub-parts:

**(a) Guard bans fewer names than its comment claims.** The comment lists five
retired names; the ban list had two:

```
hashing.rs:71-72  // There is no model_hash / fit_content_hash / sim_hash /
                  //   scen_hash / canonical_params here anymore.
hashing.rs:90     let banned = ["model_hash(", "fit_content_hash("];
```

The other three are genuinely absent from production source today —
`rg 'sim_hash|scen_hash|canonical_params' rust/crates/cli/src rust/crates/runid/src`
(minus tests) hits only the `hashing.rs` comment, which the guard skips — so
there is **no live gap**. But the guard under-delivers on its stated "constrain
future agents" purpose. Extended the ban list to all five.

**(b) Stale `model_identity_hex` references.** No such function exists; the real
name is `model_identity_from_ir` (`resolve.rs:131`). The dead name appeared in
three doc comments:

```
hashing.rs:6, hashing.rs:75, fit/init.rs:621
```

`rg 'model_identity_hex'` → those three comment hits only. Repointed all three
to `model_identity_from_ir`.

Both are doc/test-only and risk nothing in the identity computation or stored
bytes, so applied directly.

## Finding 4 — run spec is stale (confirmed)

`docs/camdl-run-spec.md` still describes `sim_hash` / `scen_hash` / `model_hash`
as the CAS identity scheme: directory layout (`190-191`, `280-281`), code blocks
(`637-644`, `1775-1833`), and a `cli/src/hashing.rs::model_hash` contract
(`1793`). Those functions are retired; identity is now the factored `runid`
levels and the `resolve_*` paths (`resolve.rs::resolve_trajectory`,
`fit/cas.rs::fit_level_hash`). Not a runtime bug, but it actively misleads
agents — and "constrain agents toward the right solution" is the goal the gh#241
refactor was for.

**Action:** dedicated doc pass to rewrite the identity section against the
factored-levels reality, cross-linking `runid/src/lib.rs` and `resolve.rs`. Out
of scope for a code cleanup; left as a follow-up so the rewrite gets a proper
review rather than riding in here.

## What looks solid (confirmed)

- **Single write seam.** Production `commit_atomic` / `claim_streaming` calls
  appear only inside `begin_resolved_write` (`resolve.rs:389`,
  `resolve.rs:394`); every other match in `cli/src` is a comment.
  `rg 'commit_atomic|claim_streaming'` confirms.
- **Differential harness is a real net.** `resolve/tests.rs:301`
  (`differential_semantic_inputs_rekey_the_run_id`) drives 13 semantic mutations
  that must re-key, paired with a presentation-inert test (`:328`).
- **Batch dry-run/status predict through the same `cell_resolve` +
  `store.lookup` path as real writes** — no parallel prediction logic to drift.
- Legacy flat sim/scenario helpers gone from production; synthetic fit dirs
  route through `fit_level_hash`.

## Cleanup applied this pass

- `hashing.rs`: extended the legacy-identity guard ban list to all five retired
  names; fixed two `model_identity_hex` → `model_identity_from_ir` doc refs.
- `fit/init.rs`: fixed one `model_identity_hex` → `model_identity_from_ir` doc
  ref.

Findings 1, 2, 4 left as recommendations (see each section).
