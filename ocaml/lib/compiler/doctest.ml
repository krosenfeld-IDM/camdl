(* Doctest: compile the ```camdl fenced code blocks in Markdown docs against the
   real compiler and classify each block's outcome.

   The oracle is [Compiler.collect_diagnostics] — the same full, non-aborting
   pipeline the CLI runs (lex -> parse -> expand -> validate -> dimcheck -> lint
   -> autodiff), returning structured diagnostics as values.

   v1 infers intent from the compiler's verdict plus block shape rather than a
   directive vocabulary, so the spec needs no mass-tagging:

     - no Error diagnostic                 -> Pass
     - depends on an external file (read()) -> Skip_data
     - only E001 (syntax)                  -> Skip_parse   (legend / bare expr)
     - errors but not a model (no compartments) -> Skip_fragment
     - a complete-model-shaped block that errors -> Fail
     - an explicit ```camdl ignore fence   -> Skip_ignore  (escape hatch) *)

(* ── string helpers (no Str dependency) ──────────────────────────────────── *)

let contains ~sub s =
  let ls = String.length s and lsub = String.length sub in
  if lsub = 0 then true
  else begin
    let rec go i =
      if i + lsub > ls then false
      else if String.sub s i lsub = sub then true
      else go (i + 1)
    in
    go 0
  end

let lstrip s =
  let n = String.length s in
  let i = ref 0 in
  while !i < n && (s.[!i] = ' ' || s.[!i] = '\t') do incr i done;
  String.sub s !i (n - !i)

let starts_with ~prefix s =
  String.length s >= String.length prefix
  && String.sub s 0 (String.length prefix) = prefix

(* Split an info string into tokens on spaces, tabs and commas. *)
let tokens s =
  String.map (fun c -> if c = '\t' || c = ',' then ' ' else c) s
  |> String.split_on_char ' '
  |> List.filter (fun t -> t <> "")

(* ── Markdown fence extraction ────────────────────────────────────────────── *)

type block = {
  file    : string;
  line    : int;        (* 1-based line of the opening fence *)
  ignore_ : bool;       (* ```camdl ignore directive present *)
  source  : string;
}

(* A two-state scanner. A fence is any line whose first non-blank chars are
   ```; when not inside a block it opens one, when inside it closes one. Only
   blocks whose info string's first token is "camdl" are captured; ```bash /
   bare ``` blocks are consumed-and-discarded so their bodies never leak. CAMDL
   block bodies do not themselves contain ``` lines. *)
let extract_blocks file : block list =
  let ic = open_in file in
  let blocks = ref [] in
  let in_block = ref false in
  let capturing = ref false in
  let buf = Buffer.create 256 in
  let start = ref 0 in
  let ign = ref false in
  let lineno = ref 0 in
  (try
     while true do
       let line = input_line ic in
       incr lineno;
       let t = lstrip line in
       if starts_with ~prefix:"```" t then begin
         if not !in_block then begin
           let info = String.trim (String.sub t 3 (String.length t - 3)) in
           match tokens info with
           | "camdl" :: rest ->
             in_block := true; capturing := true;
             start := !lineno; ign := List.mem "ignore" rest;
             Buffer.clear buf
           | _ ->
             in_block := true; capturing := false
         end else begin
           if !capturing then
             blocks :=
               { file; line = !start; ignore_ = !ign; source = Buffer.contents buf }
               :: !blocks;
           in_block := false; capturing := false
         end
       end else if !in_block && !capturing then begin
         Buffer.add_string buf line;
         Buffer.add_char buf '\n'
       end
     done
   with End_of_file -> ());
  close_in ic;
  List.rev !blocks

(* ── classification ───────────────────────────────────────────────────────── *)

type verdict =
  | Pass
  | Skip_ignore
  | Skip_parse
  | Skip_data
  | Skip_fragment
  | Fail of Diagnostics.diagnostic list
  | Ice of string

let classify (b : block) : verdict =
  if b.ignore_ then Skip_ignore
  else
    match
      (try `Ok (Compiler.collect_diagnostics ~filename:b.file b.source)
       with e -> `Raised (Printexc.to_string e))
    with
    | `Raised msg -> Ice msg
    | `Ok diags ->
      let errors =
        List.filter (fun (d : Diagnostics.diagnostic) -> d.severity = Diagnostics.Error) diags
      in
      if errors = [] then Pass
      else begin
        let codes = List.map (fun (d : Diagnostics.diagnostic) -> d.code) errors in
        if contains ~sub:"read(" b.source || List.mem "E200" codes then Skip_data
        else if List.for_all (fun c -> c = "E001") codes then Skip_parse
        else if not (contains ~sub:"compartments" b.source) then Skip_fragment
        else Fail errors
      end

(* ── report / entry point ─────────────────────────────────────────────────── *)

let run ~gate ~verbose files =
  let total = ref 0 and npass = ref 0 and nfail = ref 0 in
  let n_parse = ref 0 and n_frag = ref 0 and n_data = ref 0 and n_ign = ref 0 in
  List.iter
    (fun file ->
       let blocks = extract_blocks file in
       Printf.printf "\n%s — %d camdl block(s)\n" file (List.length blocks);
       List.iter
         (fun b ->
            incr total;
            match classify b with
            | Pass -> incr npass; if verbose then Printf.printf "  pass   L%d\n" b.line
            | Skip_ignore ->
              incr n_ign; if verbose then Printf.printf "  skip   L%d  (ignore)\n" b.line
            | Skip_parse ->
              incr n_parse; if verbose then Printf.printf "  skip   L%d  (parse-only fragment)\n" b.line
            | Skip_data ->
              incr n_data; if verbose then Printf.printf "  skip   L%d  (needs external data file)\n" b.line
            | Skip_fragment ->
              incr n_frag; if verbose then Printf.printf "  skip   L%d  (fragment)\n" b.line
            | Fail errors ->
              incr nfail;
              let codes = List.map (fun (d : Diagnostics.diagnostic) -> d.code) errors in
              let msg = match errors with d :: _ -> d.Diagnostics.message | [] -> "" in
              Printf.printf "  FAIL   L%d  [%s]  %s\n" b.line (String.concat "," codes) msg
            | Ice msg ->
              incr nfail;
              Printf.printf "  FAIL   L%d  [ICE]  compiler raised: %s\n" b.line msg)
         blocks)
    files;
  let nskip = !n_parse + !n_frag + !n_data + !n_ign in
  Printf.printf "\n── summary ──\n";
  Printf.printf
    "%d blocks: %d pass, %d skip (%d parse, %d fragment, %d data, %d ignore), %d FAIL\n"
    !total !npass nskip !n_parse !n_frag !n_data !n_ign !nfail;
  if gate && !nfail > 0 then begin
    Printf.printf "\ngate: FAILED (%d block(s) did not compile)\n" !nfail;
    exit 1
  end

let main args =
  let gate = ref false and verbose = ref false and files = ref [] in
  List.iter
    (fun a ->
       match a with
       | "--gate" -> gate := true
       | "--verbose" | "-v" -> verbose := true
       | s when String.length s > 0 && s.[0] = '-' ->
         Printf.eprintf "doctest: unknown flag %s\n" s; exit 1
       | s -> files := s :: !files)
    args;
  let files = List.rev !files in
  if files = [] then begin
    print_endline "usage: camdlc doctest [--gate] [--verbose] FILE.md ...";
    exit 1
  end;
  run ~gate:!gate ~verbose:!verbose files
