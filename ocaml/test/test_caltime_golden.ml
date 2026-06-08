(* Cross-language calendar-time golden (gh#98).

   Reads `ir/golden/caltime.tsv` — the single source of truth shared with
   `rust/crates/ir/tests/caltime_golden.rs` — and asserts, for every row,
   that the OCaml date machinery (Expander.parse_iso_date /
   parse_date_to_float) agrees with the committed `delta_days` / `t` (accept
   rows) or rejects the string (reject rows). The Rust test reads the same
   file, so the two parsers are pinned to the same acceptance set and the
   same conversion — closing the two-un-pinned-date-parsers hole the
   typed-time proposal §5.6 flagged. *)

(* Resolve the repo root (the TSV lives at ir/golden/, outside the ocaml
   dune root, so it can't be pulled in via (deps ...)). Same walk as
   test_diagnostics.ml. *)
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
  match walk (Sys.getcwd ()) 12 with
  | Some d -> d
  | None ->
    (match walk (Filename.dirname Sys.executable_name) 12 with
     | Some d -> d
     | None -> Alcotest.failf "could not locate repo root")

let read_lines path =
  let ic = open_in path in
  let rec loop acc =
    match input_line ic with
    | line -> loop (line :: acc)
    | exception End_of_file -> close_in ic; List.rev acc
  in loop []

let unit_lit_of_string = function
  | "days"   -> Ast.Days
  | "weeks"  -> Ast.Weeks
  | "months" -> Ast.Months
  | "years"  -> Ast.Years
  | other    -> Alcotest.failf "unknown time_unit in golden: %s" other

(* Split a TSV line on tabs WITHOUT trimming the cells — a date cell in the
   golden deliberately carries surrounding whitespace (the trim-acceptance
   row), so we must hand the raw cell to parse_iso_date. *)
let split_tab s = String.split_on_char '\t' s

type row = {
  origin     : string;
  date       : string;
  time_unit  : string;
  expect     : string;          (* "accept" | "reject" *)
  delta_days : string;          (* integer or "-" *)
  t          : string;          (* float or "-" *)
}

let parse_rows () =
  let path = Filename.concat repo_root "ir/golden/caltime.tsv" in
  if not (Sys.file_exists path) then
    Alcotest.failf "golden table missing: %s" path;
  read_lines path
  |> List.filter (fun l ->
       let lt = String.trim l in
       lt <> "" && not (String.length lt > 0 && lt.[0] = '#'))
  |> List.filter_map (fun l ->
       match split_tab l with
       | [origin; date; time_unit; expect; delta_days; t] ->
         (* Skip the header row. *)
         if origin = "origin" then None
         else Some { origin; date; time_unit; expect; delta_days; t }
       | cells ->
         Alcotest.failf "malformed golden row (%d cells): %S"
           (List.length cells) l)

(* One Alcotest case per row, so a failure names the exact (origin, date). *)
let check_row r () =
  match r.expect with
  | "reject" ->
    (match Expander.parse_iso_date r.date with
     | Ok _ ->
       Alcotest.failf "expected REJECT for date %S but it parsed" r.date
     | Error _ -> ())
  | "accept" ->
    (* delta_days: rata_die(date) - rata_die(origin). *)
    let (oy, om, od) = match Expander.parse_iso_date r.origin with
      | Ok v -> v | Error e -> Alcotest.failf "origin %S: %s" r.origin e in
    let (ty, tm, td) = match Expander.parse_iso_date r.date with
      | Ok v -> v | Error e -> Alcotest.failf "date %S: %s" r.date e in
    let delta = Expander.days_of_date ty tm td
                - Expander.days_of_date oy om od in
    Alcotest.(check int)
      (Printf.sprintf "%s -> %s delta_days" r.origin (String.trim r.date))
      (int_of_string r.delta_days) delta;
    (* t: route through the production conversion (parse_date_to_float),
       which divides by days_per_unit — exercises the real code, not a
       duplicated constant. *)
    let t = Expander.parse_date_to_float r.origin r.date
              (unit_lit_of_string r.time_unit) in
    Alcotest.(check (float 1e-6))
      (Printf.sprintf "%s -> %s t (%s)" r.origin (String.trim r.date) r.time_unit)
      (float_of_string r.t) t
  | other -> Alcotest.failf "unknown expect column: %S" other

let () =
  let rows = parse_rows () in
  let cases = List.map (fun r ->
    let name = Printf.sprintf "%s %s|%s|%s"
      r.expect r.origin (String.trim r.date) r.time_unit in
    Alcotest.test_case name `Quick (check_row r)
  ) rows in
  Alcotest.run "caltime_golden" [ "rows", cases ]
