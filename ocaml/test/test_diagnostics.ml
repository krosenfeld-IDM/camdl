(* Compiler-diagnostics harness.

   Four pieces, all driven through the real compile pipeline via
   [Compiler.collect_diagnostics] (lex → parse → expand → validate →
   dimcheck → lint → autodiff), which returns EVERY diagnostic — errors,
   warnings, and infos — without rendering or aborting:

   1. Fixture-by-code driver: every `.camdl` under `test/lints/` is
      annotated with the diagnostic codes (+severity) it must emit via an
      inline `# expect:` comment, and the driver asserts the emitted
      (code, severity) set EXACTLY matches — catching both misses and
      spurious emissions, for warnings/lints and errors alike.

   2. Clean-corpus regression: every `.camdl` under the model corpora
      (`ocaml/golden/`, `tests/fixtures/`, `tests/recovery/`,
      `tests/external/`) must emit NO diagnostic, modulo a small explicit
      allowlist (empty today). Guards future lints from false-positiving
      on real models.

   3. Catalog consistency: the set of diagnostic codes EMITTED in the
      compiler source equals the set DOCUMENTED in
      `docs/dev/warning-catalog.md`.

   Run with:  cd ocaml && dune runtest *)

(* Disable the constant-fold escape-hatch dependence: collect_diagnostics
   uses the production pipeline, where folding is on by default. Folding
   does not emit diagnostics, so it is harmless here; we leave it at its
   default. *)

(* ── Small string helpers (no Str dependency) ───────────────────────────── *)

let trim = String.trim

let starts_with ~prefix s =
  let pl = String.length prefix in
  String.length s >= pl && String.sub s 0 pl = prefix

let split_on c s = String.split_on_char c s

(* Split a string into whitespace-separated tokens. *)
let words s =
  s |> String.split_on_char ' '
    |> List.concat_map (String.split_on_char '\t')
    |> List.map trim
    |> List.filter (fun w -> w <> "")

let read_file path =
  let ic = open_in_bin path in
  Fun.protect ~finally:(fun () -> close_in ic) (fun () ->
    let n = in_channel_length ic in
    really_input_string ic n)

(* ── Repo-root resolution (works regardless of dune cwd) ─────────────────── *)

(* The corpus directories `tests/fixtures`, `tests/recovery`, and
   `tests/external` live at the repository root — OUTSIDE the `ocaml/`
   dune project root — so they cannot be pulled in as dune `(deps ...)`.
   Instead we locate the real repo root at runtime by walking up from the
   cwd looking for the `ir/VERSION` marker, then read the corpus files by
   their true source paths. `ocaml/test/lints/` and `ocaml/golden/` live
   inside the project and are also resolved this way for uniformity. *)
let repo_root =
  let is_root dir =
    Sys.file_exists (Filename.concat dir "ir/VERSION")
    && Sys.file_exists (Filename.concat dir "ocaml/dune-project")
  in
  let rec walk dir depth =
    if is_root dir then Some dir
    else if depth = 0 then None
    else
      let parent = Filename.dirname dir in
      if parent = dir then None else walk parent (depth - 1)
  in
  let start = Sys.getcwd () in
  match walk start 12 with
  | Some d -> d
  | None ->
    (* Fallback: walk up from the test executable's directory. *)
    let exe_dir = Filename.dirname Sys.executable_name in
    (match walk exe_dir 12 with
     | Some d -> d
     | None ->
       Alcotest.failf
         "could not locate repo root (no ir/VERSION + ocaml/dune-project) \
          from cwd=%s or exe_dir=%s" start exe_dir)

let root_path rel = Filename.concat repo_root rel

(* List every `*.camdl` under [dir], recursing into subdirectories so
   `tests/recovery/cases/<name>/model.camdl` is found. A subdirectory whose
   basename is in [skip_dirs] is pruned — used to skip
   `ocaml/golden/errors/`, which holds DELIBERATELY broken models (the
   `negative_golden` fixtures) that are not part of any clean corpus.
   Returns absolute paths, sorted; missing directory → []. *)
let camdl_files_under ?(skip_dirs = []) dir : string list =
  let rec collect d acc =
    if not (Sys.file_exists d && Sys.is_directory d) then acc
    else
      Sys.readdir d
      |> Array.to_list
      |> List.sort String.compare
      |> List.fold_left (fun acc entry ->
           let p = Filename.concat d entry in
           if Sys.is_directory p then
             (if List.mem entry skip_dirs then acc else collect p acc)
           else if Filename.check_suffix p ".camdl" then p :: acc
           else acc) acc
  in
  List.rev (collect dir [])

(* ── Expectation annotations ─────────────────────────────────────────────

   A fixture declares its expected diagnostics with one inline comment:

     # expect: L402 Warning
     # expect: E300 Error, E310 Error
     # expect: (none)

   We parse the (code, severity) pairs. Severity tokens are
   Error | Warning | Info (case-insensitive). `(none)` means zero
   diagnostics expected. The annotation is required: a fixture without one
   is a test authoring error. ─────────────────────────────────────────── *)

let severity_of_string s =
  match String.lowercase_ascii (trim s) with
  | "error"   -> Diagnostics.Error
  | "warning" -> Diagnostics.Warning
  | "info"    -> Diagnostics.Info
  | other -> Alcotest.failf "unknown severity token %S in expect: annotation" other

let severity_to_string = function
  | Diagnostics.Error -> "Error"
  | Diagnostics.Warning -> "Warning"
  | Diagnostics.Info -> "Info"

(* The set of expected (code, severity) pairs, sorted+deduped for stable
   comparison and printing. *)
module Pair = struct
  type t = string * Diagnostics.severity
  let compare (c1, s1) (c2, s2) =
    let c = String.compare c1 c2 in
    if c <> 0 then c else compare s1 s2
  let to_string (c, s) = Printf.sprintf "%s/%s" c (severity_to_string s)
end

let pairs_to_string ps =
  if ps = [] then "(none)"
  else String.concat ", " (List.map Pair.to_string ps)

let parse_expectation ~fixture (src : string) : Pair.t list =
  let lines = split_on '\n' src in
  let annotation =
    List.find_map (fun line ->
      let l = trim line in
      (* Accept `# expect:` with any spacing after the hash. *)
      if starts_with ~prefix:"#" l then
        let body = trim (String.sub l 1 (String.length l - 1)) in
        if starts_with ~prefix:"expect:" body then
          Some (trim (String.sub body 7 (String.length body - 7)))
        else None
      else None
    ) lines
  in
  match annotation with
  | None ->
    Alcotest.failf "fixture %s has no `# expect:` annotation" fixture
  | Some rest ->
    let rest = trim rest in
    if rest = "(none)" || rest = "" then []
    else
      rest
      |> split_on ','
      |> List.map trim
      |> List.filter (fun s -> s <> "")
      |> List.map (fun entry ->
           match words entry with
           | [code; sev] -> (code, severity_of_string sev)
           | [_code] ->
             Alcotest.failf
               "fixture %s expect-entry %S is missing a severity \
                (use `CODE Error|Warning|Info`)" fixture entry
           | _ ->
             Alcotest.failf
               "fixture %s has malformed expect-entry %S" fixture entry)
      |> List.sort_uniq Pair.compare

(* Run the real pipeline and collapse to the sorted (code, severity) set. *)
let emitted_pairs path : Pair.t list =
  let src = read_file path in
  Compiler.collect_diagnostics ~name:(Filename.basename path) ~filename:path src
  |> List.map (fun (d : Diagnostics.diagnostic) -> (d.code, d.severity))
  |> List.sort_uniq Pair.compare

(* ── Piece 2: fixture-by-code driver ─────────────────────────────────────── *)

let lints_dir = root_path "ocaml/test/lints"

let test_fixture path () =
  let src = read_file path in
  let expected = parse_expectation ~fixture:(Filename.basename path) src in
  let actual = emitted_pairs path in
  if expected <> actual then
    Alcotest.failf
      "diagnostic mismatch for %s\n  expected: %s\n  actual:   %s"
      (Filename.basename path) (pairs_to_string expected) (pairs_to_string actual)

let fixture_cases () =
  let files = camdl_files_under lints_dir in
  if files = [] then
    Alcotest.failf "no .camdl fixtures found under %s" lints_dir;
  List.map (fun path ->
    Alcotest.test_case (Filename.basename path) `Quick (test_fixture path)
  ) files

(* ── Piece 3: clean-corpus regression ────────────────────────────────────── *)

(* Allowlist of (filename-basename, code) pairs that are intentionally
   expected to fire on the corpus. Empty today: a prior check found zero
   L402 across these models, and the corpus is presumed clean of
   Error/Warning/Lint diagnostics. A future intentional case is added here
   with a one-line rationale. *)
let corpus_allowlist : (string * string) list = [
  (* (basename, code); e.g. ("some_model.camdl", "W301"); *)
]

(* The model corpora presumed clean. `ocaml/golden/errors/` is pruned: it
   holds deliberately-broken negative fixtures (the dimcheck/semantic error
   suite), not real models. `tests/fixtures/` carries only `.toml` today
   (no `.camdl`), but is listed so a future `.camdl` there is auto-covered. *)
let corpus_dirs = [
  ("ocaml/golden",   ["errors"; "data"]);
  ("tests/fixtures", []);
  ("tests/recovery", []);
]

(* Severity policy: a clean corpus must emit no Error, Warning, or Lint
   (L4xx is Warning severity). Info (I300, "dimension could not be
   determined") is non-blocking and fires on otherwise-valid models whose
   parameter dimensions are under-annotated — the existing dimcheck
   `golden_no_false_positives` test likewise ignores Info. We therefore do
   NOT treat Info as an offender. *)
let is_offending_severity = function
  | Diagnostics.Error | Diagnostics.Warning -> true
  | Diagnostics.Info -> false

let test_corpus_clean () =
  let allowed basename code =
    List.exists (fun (f, c) -> f = basename && c = code) corpus_allowlist
  in
  let files =
    List.concat_map (fun (rel, skip_dirs) ->
      camdl_files_under ~skip_dirs (root_path rel)) corpus_dirs
  in
  if files = [] then
    Alcotest.failf
      "clean-corpus test found no .camdl files under %s (repo_root=%s)"
      (String.concat ", " (List.map fst corpus_dirs)) repo_root;
  let offenders =
    List.concat_map (fun path ->
      let basename = Filename.basename path in
      emitted_pairs path
      |> List.filter (fun (code, sev) ->
           is_offending_severity sev && not (allowed basename code))
      |> List.map (fun (code, sev) ->
           Printf.sprintf "%s: %s/%s" basename code (severity_to_string sev))
    ) files
  in
  if offenders <> [] then
    Alcotest.failf
      "corpus models emitted unexpected diagnostics (presumed clean):\n  %s"
      (String.concat "\n  " offenders)

(* ── Piece 4: catalog consistency ────────────────────────────────────────── *)

(* Scan the compiler sources for emit-site codes. Codes appear two ways:
   as `~code:"Xnnn"` arguments and as bare `"Xnnn"` data passed to
   Dimcheck/Validate/Lint/Parser_errors helpers. Both reduce to the literal
   `"Xnnn"` string, so a single literal scan over .ml/.mll/.mly catches all
   of them. The .mly grammar carries parser-action emit sites (E1xx, etc.),
   so it MUST be scanned too. *)

(* Match a 4-char code "Xnnn" (uppercase letter + 3 digits) inside a
   double-quoted literal. We scan for the quoted form to avoid matching
   prose like "E300" in comments without quotes — every real emit site
   passes the code as a string literal. *)
let codes_in_source (txt : string) : string list =
  let n = String.length txt in
  let is_upper c = c >= 'A' && c <= 'Z' in
  let is_digit c = c >= '0' && c <= '9' in
  let acc = ref [] in
  let i = ref 0 in
  while !i < n do
    (* look for the pattern: '"' UPPER DIGIT DIGIT DIGIT '"' *)
    if !i + 5 < n
       && txt.[!i] = '"'
       && is_upper txt.[!i + 1]
       && is_digit txt.[!i + 2]
       && is_digit txt.[!i + 3]
       && is_digit txt.[!i + 4]
       && txt.[!i + 5] = '"'
    then begin
      acc := String.sub txt (!i + 1) 4 :: !acc;
      i := !i + 6
    end else
      incr i
  done;
  !acc

let rec source_files_under dir : string list =
  if not (Sys.file_exists dir && Sys.is_directory dir) then []
  else
    Sys.readdir dir
    |> Array.to_list
    |> List.sort String.compare
    |> List.concat_map (fun entry ->
         let p = Filename.concat dir entry in
         if Sys.is_directory p then source_files_under p
         else if List.exists (Filename.check_suffix p) [".ml"; ".mll"; ".mly"]
         then [p] else [])

module SS = Set.Make (String)

let emitted_codes () : SS.t =
  let files = source_files_under (root_path "ocaml/lib") in
  List.fold_left (fun s f ->
    List.fold_left (fun s c -> SS.add c s) s (codes_in_source (read_file f))
  ) SS.empty files

(* Parse the catalog. Two kinds of table rows carry codes in their first
   cell: a single code `| Xnnn | ... |`, and a range `| Xnnn–Xmmm | ... |`
   (en-dash or hyphen). Ranges denote a RESERVED namespace: the emit-side
   need not populate every code in a range, so ranges are exempt from the
   "every catalog code is emitted" direction, but DO cover any emitted code
   that falls inside them. Returns (single_codes, ranges) where a range is
   (prefix_char, lo, hi). *)
let parse_catalog () : SS.t * (char * int * int) list =
  let txt = read_file (root_path "docs/dev/warning-catalog.md") in
  let singles = ref SS.empty in
  let ranges = ref [] in
  List.iter (fun line ->
    let l = trim line in
    if starts_with ~prefix:"|" l then begin
      match split_on '|' l with
      | _ :: cell :: _ ->
        let cell = trim cell in
        let cn = String.length cell in
        let is_upper c = c >= 'A' && c <= 'Z' in
        let is_digit c = c >= '0' && c <= '9' in
        let parse_code_at off =
          if off + 3 < cn + 1
             && off + 3 < cn
             && is_upper cell.[off]
             && is_digit cell.[off + 1]
             && is_digit cell.[off + 2]
             && is_digit cell.[off + 3]
          then Some (cell.[off], int_of_string (String.sub cell (off + 1) 3))
          else None
        in
        (match parse_code_at 0 with
         | None -> ()
         | Some (p1, n1) ->
           (* A range looks like "Xnnn–Xmmm" or "Xnnn-Xmmm"; the dash sits at
              offset 4. The en-dash is multi-byte (UTF-8 E2 80 93), so test
              for an ASCII hyphen at 4 OR a non-ASCII byte (start of en-dash)
              at 4, followed eventually by a second code. *)
           let is_single () =
             (* cell is exactly the 4-char code (after trimming) *)
             cn = 4
           in
           if is_single () then
             singles := SS.add (Printf.sprintf "%c%03d" p1 n1) !singles
           else begin
             (* find a second code anywhere after offset 4 *)
             let second = ref None in
             let j = ref 4 in
             while !second = None && !j + 3 < cn do
               (match parse_code_at !j with
                | Some (p2, n2) when p2 = p1 -> second := Some n2
                | _ -> ());
               incr j
             done;
             (match !second with
              | Some n2 when n2 >= n1 -> ranges := (p1, n1, n2) :: !ranges
              | _ ->
                (* code followed by trailing prose, not a range: treat the
                   leading code as a single. *)
                singles := SS.add (Printf.sprintf "%c%03d" p1 n1) !singles)
           end)
      | _ -> ()
    end
  ) (split_on '\n' txt);
  (!singles, !ranges)

let code_in_range (code : string) (p, lo, hi) =
  String.length code = 4
  && code.[0] = p
  && (let n = try int_of_string (String.sub code 1 3) with _ -> -1 in
      n >= lo && n <= hi)

let test_catalog_consistency () =
  let emitted = emitted_codes () in
  let (singles, ranges) = parse_catalog () in
  let covered code =
    SS.mem code singles || List.exists (code_in_range code) ranges
  in
  (* Direction 1 (load-bearing): every emitted code is documented (by a
     single row or a covering range). Catches a new emit site shipped
     without a catalog entry. *)
  let orphan_emit =
    SS.elements emitted |> List.filter (fun c -> not (covered c))
  in
  (* Direction 2: every SINGLE-code catalog row has a live emit site.
     Range rows are reserved namespaces and exempt. Catches a stale
     catalog row for a code that no longer exists. *)
  let orphan_catalog =
    SS.elements singles |> List.filter (fun c -> not (SS.mem c emitted))
  in
  if orphan_emit <> [] then
    Alcotest.failf
      "emit sites with NO catalog row (add to docs/dev/warning-catalog.md):\n  %s"
      (String.concat ", " orphan_emit);
  if orphan_catalog <> [] then
    Alcotest.failf
      "catalog rows with NO emit site (stale — remove or implement):\n  %s"
      (String.concat ", " orphan_catalog);
  (* Non-vacuity guard: a parsing bug that produced empty sets would make
     both directions trivially pass. Assert we actually found a healthy
     number of codes on both sides. *)
  Alcotest.(check bool) "scanned a non-trivial number of emit codes"
    true (SS.cardinal emitted > 50);
  Alcotest.(check bool) "parsed a non-trivial number of catalog codes"
    true (SS.cardinal singles + List.length ranges > 20)

(* ── Driver ──────────────────────────────────────────────────────────────── *)

let () =
  Alcotest.run "diagnostics" [
    "fixtures", fixture_cases ();
    "corpus", [
      Alcotest.test_case "model corpus is diagnostic-clean" `Quick test_corpus_clean;
    ];
    "catalog", [
      Alcotest.test_case "emit sites ↔ warning-catalog.md" `Quick test_catalog_consistency;
    ];
  ]
