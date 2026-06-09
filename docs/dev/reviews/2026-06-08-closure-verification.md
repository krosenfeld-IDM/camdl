# Closure-verification pass — 2026-06-08

**Date:** 2026-06-08 **Base commit:** `ea764154` —
`docs(dev): bring root-cause tracker current — S-class drained, Tier-A landed (#189/#190/#127), 3 held`

Each issue closed this session by a code/doc fix is matched to its specific
test, the test is run against the current checkout, and the pass line is
recorded. Test names were pinned by grepping the source, not trusting the
assignment's guesses — where the real name differed, the real one is recorded.

## Results

| issue         | test name                                                                                                                                                                                      | suite / command                                                                              | result                            |
| ------------- | ---------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | -------------------------------------------------------------------------------------------- | --------------------------------- |
| #112          | `table_lookup_arity` group (under/over-indexed E202 + correct-arity ok)                                                                                                                        | `dune` → `test_compiler.exe test 'table_lookup_arity'`                                       | PASS (3 run)                      |
| #117          | `declaration_names` group (E278 dup param/let + cross-namespace ×2)                                                                                                                            | `dune` → `test_compiler.exe test 'declaration_names'`                                        | PASS (4 run)                      |
| #114 (OCaml)  | `init_membership` group (E277 bare-stratified / unknown / concrete / 1-diag)                                                                                                                   | `dune` → `test_compiler.exe test 'init_membership'`                                          | PASS (4 run)                      |
| #98 / #223    | `iso_date_validation` group (E223 invalid day/month/non-leap Feb29 + ok)                                                                                                                       | `dune` → `test_compiler.exe test 'iso_date_validation'`                                      | PASS (4 run)                      |
| #98 (caltime) | `test_caltime_golden.exe` (cross-language date golden, 21 rows)                                                                                                                                | `dune` → `test_caltime_golden.exe`                                                           | PASS (21 run)                     |
| #123          | `intervention_set_target_…` / `event_transfer_dst_…` / `balance_target_…` / `table_lookup_wrong_arity_is_rejected`                                                                             | `cargo test -p ir` (validate.rs)                                                             | PASS (within 37)                  |
| #114 (Rust)   | `init_key_unknown_compartment_is_rejected`                                                                                                                                                     | `cargo test -p ir` (validate.rs)                                                             | PASS (within 37)                  |
| #124          | `init_value_negative_is_rejected` / `…_nan_…` / `…_inf_…` / `…_fractional_on_integer_…` (+real-comp variants)                                                                                  | `cargo test -p ir` (validate.rs)                                                             | PASS (within 37)                  |
| #127 (ir)     | `table_lookup_constant_index_above_range_is_rejected` / `…_negative_…` / `…_fractional_index_uses_floor`                                                                                       | `cargo test -p ir` (validate.rs)                                                             | PASS (within 37)                  |
| #98 (Rust)    | `caltime_golden` integration test (`tests/caltime_golden.rs`)                                                                                                                                  | `cargo test -p ir --test caltime_golden`                                                     | PASS (1 run)                      |
| #108          | `subday_timepoints_render_distinctly` / `hires_negative_subday_and_whole_day_carry` / `rounded_renderer_keeps_bare_date_for_subday`                                                            | `cargo test -p ir --lib` (caltime.rs)                                                        | PASS (3 run)                      |
| #127 (sim)    | `test_runtime_oob_table_lookup_returns_err_not_panic`                                                                                                                                          | `cargo test -p sim --test expr_eval`                                                         | PASS (1 run)                      |
| #128          | `unknown_rate_grad_key_is_rejected` (+ `well_keyed_rate_grad_compiles` guard)                                                                                                                  | `cargo test -p sim --lib` (compiled_model.rs)                                                | PASS (2 run)                      |
| #191 / #192   | `chain_binomial_inference_rejects_real_compartments` / `chain_binomial_inference_accepts_balance` / `unsupported_capability_message_is_never_blank` (+ obs_alignment matrix)                   | `cargo test -p cli --bin camdl fit::methods`                                                 | PASS (13 run)                     |
| #147          | `fit_content_hash_*` / `model_hash_*` / `sim_hash_*` / `scen_hash_*` (hashing.rs allowlist)                                                                                                    | `cargo test -p cli --bin camdl hashing`                                                      | PASS (35 run)                     |
| #66           | `real_param_with_bounds_gets_bounded_transform_not_none` / `instant_param_with_negative_bounds_…` / `unbounded_real_param_stays_none`                                                          | `cargo test -p cli --bin camdl` (fit/runner.rs)                                              | PASS (3 run)                      |
| #37 / #36     | `from_scenario_compose_carves_out_estimated_params` / `from_scenario_walks_compose_inherits_params` (+10 from_scenario cases)                                                                  | `cargo test -p cli --bin camdl from_scenario` (config_v2.rs)                                 | PASS (12 run)                     |
| #189 / #190   | `fit::cas::tests::resolved_obs_alignment_is_keyed_per_stage` / `holdout_content_changes_the_fit_digest`; `runid` `stage_config_obs_alignment_is_keyed` / `fit_digest_holdout_content_is_keyed` | `cargo test -p cli --bin camdl fit::cas` + `cargo test -p runid`                             | PASS (cli 2 + runid 62)           |
| #97           | `profile_pmmh_reported_loglik_matches_saved_mle_params`                                                                                                                                        | `cargo test -p cli --test profile_pmmh`                                                      | PASS (3 run)                      |
| #174          | `positive_incidence_at_origin_is_named_error` (+ `dropping_origin_row_scores_finite`)                                                                                                          | `cargo test -p cli --test incidence_t0`                                                      | PASS (2 run)                      |
| #183          | doc-fix + `simulate_backend_help_names_resolved_default` (args.rs)                                                                                                                             | docs verified + `cargo test -p cli --bin camdl simulate_backend_help_names_resolved_default` | PASS (doc-confirmed + 1 test run) |

### Pass lines (verbatim)

OCaml (`CAMDL_SKIP_VERSION_CHECK=1 dune runtest` → exit 0; targeted groups via
`_build/default/test/test_compiler.exe`):

- `table_lookup_arity` → `Test Successful in 0.002s. 3 tests run.`
- `declaration_names` → `Test Successful in 0.002s. 4 tests run.`
- `init_membership` → `Test Successful in 0.002s. 4 tests run.`
- `iso_date_validation` → `Test Successful in 0.002s. 4 tests run.`
- `test_caltime_golden.exe` → `Test Successful in 0.004s. 21 tests run.`

Rust:

- `cargo test -p ir` → `test result: ok. 37 passed; 0 failed` (lib) +
  `test result: ok. 1 passed` (caltime_golden)
- `cargo test -p ir --lib subday` → 3 caltime subday/hires/rounded tests `ok`
- `cargo test -p sim --test expr_eval test_runtime_oob_table_lookup_returns_err_not_panic`
  → `test result: ok. 1 passed`
- `cargo test -p sim --lib rate_grad_key` / `well_keyed_rate_grad` → both `ok`
- `cargo test -p cli --bin camdl` (full) →
  `test result: ok. 703 passed; 0 failed`
- `cargo test -p cli --bin camdl fit::methods` →
  `test result: ok. 13 passed; 0 failed`
- `cargo test -p cli --bin camdl hashing` →
  `test result: ok. 35 passed; 0 failed`
- `cargo test -p cli --bin camdl from_scenario` →
  `test result: ok. 12 passed; 0 failed`
- `cargo test -p runid` → `test result: ok. 62 passed; 0 failed`
- `cargo test -p cli --test profile_pmmh` →
  `test result: ok. 3 passed; 0 failed`
- `cargo test -p cli --test incidence_t0` →
  `test result: ok. 2 passed; 0 failed`

## Summary

**19 / 19 closures verified green on current main** (`ea764154`).

Counting distinct issue numbers in the assignment list: #112, #114, #117, #98,
#223, #123, #191, #192, #147, #97, #127, #189, #190, #66, #174, #108, #37, #36,
#124, #128, #183. (#223 is the date-range work that shares the E223 OCaml
diagnostic with #98; #114 and #98 and #127 each have an OCaml/Rust split that
was verified on both sides.) Every issue maps to at least one test that is
present and passing on the current checkout — no missing test, no red test.

## Discrepancies

No closed-issue test was missing, and none failed. Two non-blocking observations
recorded for accuracy (neither affects a result):

1. **#98 stale comment vs asserted code.** The source comment above the OCaml
   date-validation tests reads "Out-of-range dates must now produce a NAMED
   diagnostic (E219)" (`ocaml/test/test_compiler.ml:6204`), but the tests
   themselves assert `~code:"E223"` (lines 6224/6228/6238) and pass. The
   asserted-and-passing code is **E223**; the `E219` in the prose comment is a
   stale comment, not the live assertion. Doc-vs-code drift inside a comment, no
   behavioural impact — the test is the ground truth and it is green.

2. **#183 is more than doc-only.** The assignment flagged #183 as "doc-only, no
   test." The fix commit `51863cfc` does sync four docs (verified:
   `docs/camdl-language-spec.md:3383`, `docs/camdl-run-spec.md:983`,
   `docs/workflow.md:32` all now say `chain_binomial`), **and** it adds a real
   regression test `args::tests::simulate_backend_help_names_resolved_default`
   that renders the clap help and pins it to `Backend::ChainBinomial.as_str()`.
   That test passes (`test result: ok. 1 passed`). So #183 is verified on both
   the doc side and a test side — stronger than the assignment assumed.

### Environment notes

- The two cross-language integration test files (#97 `profile_pmmh`, #174
  `incidence_t0`) compile a `.camdl` fixture and so consult `camdlc`; they were
  run with `CAMDL_SKIP_VERSION_CHECK=1` and
  `CAMDLC=ocaml/_build/default/bin/camdlc.exe` to bypass the git-hash version
  guard (per the testing runbook). No `make install` was run; no source file was
  modified.
