(* Compile a camdl source string + optional model name to an Ir.model *)

type compile_detail = {
  model   : Ir.model;
  ctx     : Expander.context;
  summary : Expander.model_summary;
  source  : Source_cache.t;
}

(** Structured, non-raising compile outcome (gh#181 step 1).

    A value-typed surface over [collect_detail]: every diagnostic — errors,
    warnings, infos — is returned as a [diagnostic list] rather than rendered
    and raised. [value] is [Some] exactly when no [Error]-severity diagnostic
    was produced. Nothing here raises ([compile]'s [report_and_exit] /
    [Compile_error] path is bypassed entirely).

    This is the accumulating shape the gh#181 proposal targets — structurally
    [MaybeT (Writer (diagnostic list))]: the diagnostic log is always present;
    the value is present only on success. Step 1 deliberately carries the
    expanded [compile_detail] (not a fully-finished [Ir.model]) — promoting
    [value] to a gradient-attached, constant-folded model and routing
    [simulate]/[fit]/CLI through this one surface is steps 2–4 of the
    migration. Keeping it a pure addition here means no existing caller
    changes behaviour. *)
type 'a outcome = {
  value       : 'a option;
  diagnostics : Diagnostics.diagnostic list;
  source      : Source_cache.t;
}

(* ── Front-end core ───────────────────────────────────────────────────────

   The single, non-aborting lex/parse/expand front end. It runs the
   pipeline once, accumulating EVERY front-end diagnostic — the E001 on a
   lex/parse/expand failure, the W100 lex warnings, and the parser-action
   errors — into the returned [Diagnostics.t], and never renders or
   aborts. Both consumers build on it:

   - [compile_detail_result] (the production path) wraps it: on a
     front-end error it renders the diagnostics and returns [Error]; on
     a clean expand it returns [Ok].
   - [collect_diagnostics] (test/tooling) uses it directly and continues
     the downstream pipeline.

   Return shape: [(detail, diags, source)]. [detail] is [None] when
   lex/parse/expand structurally failed (then [diags] holds the E001);
   [Some d] when expansion produced a model (then [diags] is [d.ctx.diags],
   which may itself carry expansion-phase errors/warnings and the drained
   W100 / parser-action diagnostics — the caller decides whether to
   continue). [source] is the [Source_cache] for the input, returned even
   on the [None] path so a renderer can show the offending line. *)
let front_end_collect ?(name = "model") ?(filename = "<input>") (src : string)
    : compile_detail option * Diagnostics.t * Source_cache.t =
  let source = Source_cache.of_string ~filename src in
  (* Drain any stale lex-phase warnings from a previous compilation in the
     same process. pending_warnings is a mutable global ref; clearing it
     here ensures we never replay warnings from a prior run. *)
  Lexer.pending_warnings := [];
  Parser_errors.pending_errors := [];
  let parse_diags = Diagnostics.create () in
  match
    (try
       let lexbuf = Lexing.from_string src in
       Lexing.set_filename lexbuf filename;
       let t_parse = Sys.time () in
       let decls =
         (try Ok (Parser.file Lexer.token lexbuf)
          with
          | Lexer.LexError msg ->
            let pos = lexbuf.Lexing.lex_curr_p in
            Diagnostics.error parse_diags ~code:"E001"
              ~loc:(Diagnostics.loc_of_positions ~file:filename pos pos)
              ~message:(Printf.sprintf "lex error: %s" msg) ();
            Error ()
          | Parser.Error ->
            let pos = lexbuf.Lexing.lex_curr_p in
            Diagnostics.error parse_diags ~code:"E001"
              ~loc:(Diagnostics.loc_of_positions ~file:filename pos pos)
              ~message:"syntax error" ();
            Error ())
       in
       Passtime.record "parse" (Sys.time () -. t_parse);
       decls
     with
     | Failure msg ->
       Diagnostics.error parse_diags ~code:"E001" ~loc:Diagnostics.no_loc
         ~message:msg ();
       Error ()
     | exn ->
       Diagnostics.error parse_diags ~code:"E001" ~loc:Diagnostics.no_loc
         ~message:(Printexc.to_string exn) ();
       Error ())
  with
  | Error () -> (None, parse_diags, source)
  | Ok decls ->
    let source_dir =
      if filename = "<input>" then "" else Filename.dirname filename
    in
    (match
       (try Ok (Passtime.time "expand"
                  (fun () -> Expander.expand_detail ~source_dir ~filename name decls))
        with
        | Failure msg ->
          Diagnostics.error parse_diags ~code:"E001" ~loc:Diagnostics.no_loc
            ~message:msg ();
          Error ()
        | exn ->
          Diagnostics.error parse_diags ~code:"E001" ~loc:Diagnostics.no_loc
            ~message:(Printexc.to_string exn) ();
          Error ())
     with
     | Error () -> (None, parse_diags, source)
     | Ok (model, ctx, summary) ->
       (* Drain lex-phase warnings (e.g. inconsistent digit grouping)
          collected before the expander's ctx.diags was available. *)
       List.iter (fun (sp, ep, msg) ->
         Diagnostics.warning ctx.diags ~code:"W100"
           ~loc:(Diagnostics.loc_of_positions ~file:filename sp ep)
           ~message:msg ()
       ) (List.rev !Lexer.pending_warnings);
       Lexer.pending_warnings := [];
       (* Drain parser-action errors collected from semantic actions that
          used to `failwith` (n3 in the 2026-04-19 compiler review). *)
       List.iter (fun (sp, ep, code, msg, hint) ->
         Diagnostics.error ctx.diags ~code
           ~loc:(Diagnostics.loc_of_positions ~file:filename sp ep)
           ~message:msg ?hint ()
       ) (List.rev !Parser_errors.pending_errors);
       Parser_errors.pending_errors := [];
       (Some { model; ctx; summary; source }, ctx.diags, source))

(** Production front end: run [front_end_collect], and on any front-end
    error (lex/parse/expand failure, or a parser-action error drained
    into [ctx.diags]) render the diagnostics and return [Error]; on a
    clean expand return [Ok]. The [Error] payload is the rendered string
    from [Diagnostics.render] — the serialized JSON array under
    [--json-errors], else ["compilation failed"]. CLI entry points
    recognize the payload shape and exit without re-printing a redundant
    Error line (m5 in the 2026-04-19 compiler review). Warnings are NOT
    rendered here: callers render once at the end of their pipeline so
    expansion-phase warnings don't print twice when downstream passes
    (dimcheck) also emit diagnostics (M3). *)
let compile_detail_result ?(name = "model") ?(filename = "<input>") (src : string)
    : (compile_detail, string) result =
  let (detail, diags, source) = front_end_collect ~name ~filename src in
  match detail with
  | None ->
    (* Front-end failure (lex/parse/expand): [front_end_collect] has
       captured it as an E001 in [diags]. Render and return the payload,
       exactly as the old [report_and_exit]-then-catch path did. Note a
       deliberate consolidation: a [Failure] raised inside the expander
       (e.g. a malformed date literal) now surfaces as a rendered E001
       here, the same way it already did via [collect_diagnostics] — the
       old inlined [compile_detail_result] returned its bare message
       un-rendered through an outer handler. Routing both consumers
       through one core makes the diagnostic surface identical. *)
    Error (Diagnostics.render diags source)
  | Some d ->
    (* Expansion produced a model; [d.ctx.diags] may carry a drained
       parser-action error. Mirror the old [has_errors → report_and_exit]
       gate: render and return [Error] on any error, else [Ok]. *)
    if Diagnostics.has_errors d.ctx.diags then
      Error (Diagnostics.render d.ctx.diags d.source)
    else
      Ok d

let no_dim_check = ref false

(** Run the sparse-coupling constant-fold pass. On by default; the
    CAMDL_NO_CONSTANT_FOLD escape hatch forces it off (see the call site).
    Exposed as a ref so tests that assert on the *unfolded* IR shape (the
    expander's TableLookup-flattening contract) can disable it locally,
    mirroring [no_dim_check]. *)
let constant_fold = ref true

(** Translate a `Validate.error` into an E5xx Diagnostic and attach
    it to the given context. Codes are new (E500–E511) — the existing
    E2xx range covers parser/expansion-phase duplicates and unknowns,
    but `Validate.validate` runs post-expansion and can catch cases
    the parser/expander miss (e.g. unknown reference in a let-binding
    that expands into a rate, or a `Real` compartment with no ODE).
    A separate code range makes that distinction visible in output. *)
let diagnose_validate_error ctx (err : Validate.error) : unit =
  let open Validate in
  let (code, message, hint) = match err with
    | DuplicateCompartment s ->
      "E500",
      Printf.sprintf "duplicate compartment after expansion: '%s'" s,
      Some "stratification produced two compartments with the same name"
    | DuplicateTransition s ->
      "E501",
      Printf.sprintf "duplicate transition after expansion: '%s'" s,
      Some "stratification produced two transitions with the same name"
    | DuplicateParameter s ->
      "E502",
      Printf.sprintf "duplicate parameter: '%s'" s, None
    | UnknownCompartment s ->
      "E503",
      Printf.sprintf "unknown compartment referenced: '%s'" s,
      Some "check stratification / spelling against the compartments block"
    | UnknownParameter s ->
      "E504",
      Printf.sprintf "unknown parameter referenced: '%s'" s,
      Some "check the parameters block for a matching declaration"
    | UnknownTable s ->
      "E505",
      Printf.sprintf "unknown table referenced: '%s'" s, None
    | UnknownTimeFunction s ->
      "E506",
      Printf.sprintf "unknown time_function referenced: '%s'" s, None
    | UnknownTransition s ->
      "E507",
      Printf.sprintf "unknown transition referenced in observation: '%s'" s, None
    | RealCompartmentInStoichiometry (tr, c) ->
      "E508",
      Printf.sprintf "real-valued compartment '%s' cannot appear in \
                      stoichiometry of transition '%s'" c tr,
      Some "real compartments have continuous dynamics (ODE); mixing them \
            into transition stoichiometry is ill-defined"
    | MissingOdeEquation s ->
      "E509",
      Printf.sprintf "real-valued compartment '%s' has no ODE equation" s,
      Some "add an `ode { ... }` block with dX/dt for this compartment"
    | OdeForNonRealComp s ->
      "E510",
      Printf.sprintf "ODE equation for '%s', which is not a real-valued \
                      compartment" s,
      Some "only compartments declared `: real` can have ODE equations"
    | ZeroDelta (tr, c) ->
      "E511",
      Printf.sprintf "transition '%s' has zero delta for compartment '%s'" tr c,
      Some "a zero-delta stoichiometry entry has no effect; remove it"
  in
  Diagnostics.error ctx.Expander.diags
    ~code ~loc:Diagnostics.no_loc ~message ?hint ()

(** Run post-expansion structural validation.

    Wired in per M1 of the 2026-04-19 compiler review — previously
    `Validate.validate` existed in `lib/ir/validate.ml` but was never
    called from the compile pipeline, so its unknown-reference /
    missing-ODE / zero-delta checks ran nowhere. Without this pass
    the `ode_equations = []` hardcoding bug (C5) would have been
    invisible; now C5 is fixed AND the integrity net that would have
    caught it in the first place runs.

    Order: post-expansion, pre-dimcheck. Dimcheck ICEs on unknown
    params, so running Validate first gives the user a clean
    "unknown parameter 'foo'" error instead of a dimcheck trace. *)
let run_validate (d : compile_detail) : bool =
  match Validate.validate d.model with
  | Ok () -> false
  | Error errs ->
    List.iter (diagnose_validate_error d.ctx) errs;
    true

(** Run Dimcheck on a compiled model and route results into the diagnostic
    context. Exposed so `camdlc check` runs the same pass as `camdlc compile`;
    previously `check` skipped dimcheck entirely (GH #9). *)
let run_dimcheck (d : compile_detail) : unit =
  if not !no_dim_check then begin
    let dc_result = Dimcheck.check_model d.model in
    List.iter (fun (dc : Dimcheck.diagnostic) ->
      match dc.severity with
      | Dimcheck.Error ->
        Diagnostics.error d.ctx.diags
          ~code:dc.code ~loc:Diagnostics.no_loc
          ~message:dc.message ?detail:dc.detail ?hint:dc.hint ()
      | Dimcheck.Info ->
        Diagnostics.info d.ctx.diags
          ~code:dc.code ~loc:Diagnostics.no_loc
          ~message:dc.message ?detail:dc.detail ?hint:dc.hint ()
    ) dc_result.diagnostics
  end

(** Run the model linter on a compiled model and route its results into
    the diagnostic context as non-blocking warnings. Lints (L4xx) flag
    semantically valid but discouraged patterns (e.g. L402 dead
    compartment); they render with hint text but never set [has_errors],
    so the build does not fail on a lint. Called right after
    [run_dimcheck] so both `camdlc compile` and `camdlc check` run it. *)
let run_lint (d : compile_detail) : unit =
  let lint_result = Lint.check_model d.model in
  List.iter (fun (l : Lint.diagnostic) ->
    match l.severity with
    | Lint.Warning ->
      Diagnostics.warning d.ctx.diags
        ~code:l.code ~loc:Diagnostics.no_loc
        ~message:l.message ?detail:l.detail ?hint:l.hint ()
  ) lint_result.diagnostics

(** Autodiff pass: differentiate every transition rate w.r.t. all
    parameters, returning the transition list with [rate_grad] filled in.
    If a rate contains `mod` over a parameter, differentiation raises
    [Failure] — caught per-transition, emitting E600 (with source
    location) into [d.ctx.diags] and leaving that transition's
    [rate_grad] empty. Side effect is confined to the diagnostic context;
    this never renders or aborts, so it is shared verbatim by [compile]
    (which short-circuits on the resulting errors) and
    [collect_diagnostics] (which does not). *)
let differentiate_transitions (d : compile_detail) : Ir.transition list =
  let param_names = List.map (fun (p : Ir.parameter) -> p.name) d.model.Ir.parameters in
  let tr_loc name =
    (* Find the original (pre-expansion) transition declaration by prefix
       match: expanded name "infection_child" → base "infection". *)
    match List.find_opt (fun (td : Ast.transition_decl) ->
      let b = td.trname and bl = String.length td.trname in
      let el = String.length name in
      name = b || (el > bl && String.sub name 0 bl = b && name.[bl] = '_')
    ) d.ctx.orig_transitions with
    | Some td -> Expander.diag_loc_of_ast_ctx d.ctx td.trloc
    | None -> Diagnostics.no_loc
  in
  Passtime.time "autodiff" (fun () ->
    List.map (fun (t : Ir.transition) ->
      match (try Ok (Autodiff.differentiate_rate t.rate param_names)
             with Failure msg -> Error msg) with
      | Ok rate_grad -> { t with Ir.rate_grad }
      | Error msg ->
        Diagnostics.error d.ctx.diags
          ~code:"E600"
          ~loc:(tr_loc t.name)
          ~message:(Printf.sprintf "transition '%s': %s" t.name msg)
          ~hint:"mod is not differentiable; replace with a conditional guard"
          ();
        { t with Ir.rate_grad = [] }
    ) d.model.Ir.transitions)

(** Sparse-coupling constant-fold (on by default): resolves
    constant-indexed inline-table lookups and drops zero-W terms from FOI
    Reduce sums, collapsing the dense P-term spatial sum to its k nonzero
    terms. Proven byte-identical by the A/B gate (rust
    .../gate_constant_fold_ab). Set CAMDL_NO_CONSTANT_FOLD to emit the
    unfolded (dense) IR — an escape hatch for debugging the pass or
    inspecting the pre-fold shape. *)
let maybe_constant_fold (m : Ir.model) : Ir.model =
  let fold_on = !constant_fold && Sys.getenv_opt "CAMDL_NO_CONSTANT_FOLD" = None in
  if fold_on then Passtime.time "constant_fold" (fun () -> Constant_fold.fold_model m)
  else m

let compile ?(name = "model") ?(filename = "<input>") (src : string) : (Ir.model, string) result =
  match compile_detail_result ~name ~filename src with
  | Ok d ->
    (* Post-expansion structural validation (M1 / C5 in the
       2026-04-19 compiler review). *)
    if Passtime.time "validate" (fun () -> run_validate d) then
      Diagnostics.report_and_exit d.ctx.diags d.source;
    Passtime.time "dimcheck" (fun () -> run_dimcheck d);
    Passtime.time "lint" (fun () -> run_lint d);
    if Diagnostics.has_errors d.ctx.diags then
      Diagnostics.report_and_exit d.ctx.diags d.source;
    let transitions = differentiate_transitions d in
    if Diagnostics.has_errors d.ctx.diags then
      Diagnostics.report_and_exit d.ctx.diags d.source;
    (* Single render of any collected non-blocking diagnostics
       (expansion warnings + dimcheck infos + L4xx lints). This is the
       ONLY non-blocking emission, and it fires AFTER the final E600
       [has_errors] check, so it runs only when the compile is
       definitely succeeding — it can never co-fire with a
       [report_and_exit] above. (Were it placed before the autodiff
       check, an E600-with-warnings model would emit twice: once here,
       once from [report_and_exit] re-rendering everything — a cosmetic
       double-print in ANSI, two invalid JSON arrays under
       --json-errors. Routing through [Diagnostics.render] gives JSON
       under [--json-errors] and the ANSI box otherwise, matching the
       error path's shape exactly.) *)
    if Diagnostics.has_any d.ctx.diags then
      ignore (Diagnostics.render d.ctx.diags d.source);
    let m = { d.model with Ir.transitions = transitions } in
    Ok (maybe_constant_fold m)
  | Error e -> Error e

(* ── Severity-agnostic diagnostic collection ─────────────────────────────────

   [collect_detail] runs the real compile pipeline (lex → parse → expand →
   validate → dimcheck → lint → autodiff) over a source, accumulating EVERY
   diagnostic — errors, warnings, and infos alike — into the returned
   [Diagnostics.t], without rendering to stderr and without aborting via
   [report_and_exit]. It is the shared non-aborting core behind both
   [collect_diagnostics] (test/tooling: keeps only the diagnostic list) and
   `inspect`'s `run_check` (the CLI: also renders the summary off the
   [compile_detail]). [compile] is the aborting counterpart that runs the
   same stages but renders and exits on errors.

   Routing `run_check` through this core is the cure for the recurring
   check/compile divergence (gh#9 re dimcheck, gh#170 re validate): there is
   now ONE place that defines "the front-end pipeline", so `check` and
   `compile` cannot disagree on a model's validity.

   The pipeline short-circuits exactly as [compile] does: a structural
   [Validate] error stops before dimcheck (Validate runs first precisely
   because dimcheck ICEs on unknown params). On the no-error path, all of
   dimcheck, lint, and autodiff run, so non-blocking warnings/lints (e.g.
   L402 on a clean-compiling model) are captured.

   Return shape mirrors [front_end_collect]: [(detail, diags, source)] with
   [detail = None] when lex/parse/expand structurally failed (then [diags]
   holds the E001), [Some d] otherwise (then [diags] is [d.ctx.diags], now
   also carrying any validate/dimcheck/lint/autodiff diagnostics). *)
let collect_detail ?(name = "model") ?(filename = "<input>") (src : string)
    : compile_detail option * Diagnostics.t * Source_cache.t =
  let (detail, diags, source) = front_end_collect ~name ~filename src in
  (match detail with
   | None -> ()                     (* lex/parse/expand failed; diags has the E001 *)
   | Some d ->
     (* Same staged pipeline as [compile], minus rendering/abort: Validate
        first (it gates dimcheck, which ICEs on unknown params), then
        dimcheck + lint, then autodiff. Short-circuit after Validate matches
        [compile]; downstream passes run only on a structurally-valid model. *)
     if not (Passtime.time "validate" (fun () -> run_validate d)) then begin
       Passtime.time "dimcheck" (fun () -> run_dimcheck d);
       Passtime.time "lint" (fun () -> run_lint d);
       if not (Diagnostics.has_errors d.ctx.diags) then
         ignore (differentiate_transitions d)
     end);
  (detail, diags, source)

(* [collect_diagnostics] is the thin test/tooling projection of
   [collect_detail]: it discards the model/summary and returns just the
   accumulated diagnostics in source order. A fixture-driven test over it
   exercises the same diagnostic surface as the CLI. *)
let collect_diagnostics ?(name = "model") ?(filename = "<input>") (src : string)
    : Diagnostics.diagnostic list =
  let (_detail, diags, _source) = collect_detail ~name ~filename src in
  (* diags accumulates newest-first via [emit]; reverse to source order. *)
  List.rev diags.Diagnostics.diags

(** [compile_outcome] (gh#181 step 1): the non-raising projection of
    [collect_detail] into the structured {!outcome}. [collect_detail] runs the
    full pipeline (expand → validate → dimcheck → lint → autodiff),
    accumulating into [diags] without rendering or aborting; this wraps it.

    [value] is the expanded [compile_detail] exactly when no Error-severity
    diagnostic fired. A structural lex/parse/expand failure already yields
    [detail = None] (with the E001 in [diags]), so the [has_errors] gate
    subsumes that case. Unlike [compile], a late-phase error (validate E5xx,
    autodiff E600) arrives here as a value in [diagnostics] rather than a
    raised [Compile_error]. *)
let compile_outcome ?(name = "model") ?(filename = "<input>") (src : string)
    : compile_detail outcome =
  let (detail, diags, source) = collect_detail ~name ~filename src in
  { value       = (if Diagnostics.has_errors diags then None else detail);
    diagnostics = List.rev diags.Diagnostics.diags;
    source }
