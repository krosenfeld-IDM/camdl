(* Self-test for the doc-doctest classifier (Doctest module in the compiler lib).

   Drives the real parser + classifier over a fixture Markdown file whose blocks
   are ordered to exercise every verdict bucket, including a `preamble=` block
   (hidden HTML-comment preamble) and a `read()` block resolved by inline
   `camdl-doctest-data`. Non-vacuous: it asserts the FAIL block really reports
   E300, that the preamble block fails when its preamble is stripped, and that
   the data block needs its inline data. *)

let spec = "doctest_fixtures/spec.md"

let verdict_name : Doctest.verdict -> string = function
  | Doctest.Pass          -> "pass"
  | Doctest.Skip_ignore   -> "skip:ignore"
  | Doctest.Skip_parse    -> "skip:parse"
  | Doctest.Skip_data     -> "skip:data"
  | Doctest.Skip_fragment -> "skip:fragment"
  | Doctest.Fail _        -> "fail"
  | Doctest.Ice msg       -> "ice:" ^ msg

(* Expected verdict for each block, in file order. *)
let expected =
  [ "pass"           (* 1. clean model *)
  ; "skip:parse"     (* 2. bare expression *)
  ; "skip:fragment"  (* 3. transitions-only fragment *)
  ; "skip:data"      (* 4. read(), no inline data *)
  ; "fail"           (* 5. complete model, E300 *)
  ; "skip:ignore"    (* 6. ```camdl ignore *)
  ; "pass"           (* 7. fragment + preamble=demo *)
  ; "pass"           (* 8. read() + inline data *)
  ]

let test_classifications () =
  let doc = Doctest.parse_doc spec in
  let basedir = Doctest.materialize_data doc.datas in
  Alcotest.(check int) "block count" (List.length expected) (List.length doc.blocks);
  List.iteri
    (fun i (want, b) ->
       let v = verdict_name (Doctest.classify ~preambles:doc.preambles ~basedir b) in
       Alcotest.(check string)
         (Printf.sprintf "block %d @ L%d" (i + 1) b.Doctest.line) want v)
    (List.combine expected doc.blocks);
  Doctest.rm_rf basedir

let test_non_vacuous () =
  let doc = Doctest.parse_doc spec in
  let basedir = Doctest.materialize_data doc.datas in
  let nth = List.nth doc.blocks in
  (* block 5: a genuine E300, not a vacuous pass *)
  (match Doctest.classify ~preambles:doc.preambles ~basedir (nth 4) with
   | Doctest.Fail errs ->
     let codes = List.map (fun (d : Diagnostics.diagnostic) -> d.code) errs in
     Alcotest.(check bool) "block 5 reports E300" true (List.mem "E300" codes)
   | other -> Alcotest.failf "block 5: expected Fail, got %s" (verdict_name other));
  (* block 7 passes only because of its preamble *)
  let stripped = { (nth 6) with Doctest.preamble = None } in
  Alcotest.(check bool) "preamble block fails without its preamble" false
    (match Doctest.classify ~preambles:doc.preambles ~basedir stripped with
     | Doctest.Pass -> true | _ -> false);
  (* block 8 passes only because of its inline data *)
  let empty = Doctest.make_temp_dir () in
  Alcotest.(check bool) "data block needs its inline data" true
    (match Doctest.classify ~preambles:doc.preambles ~basedir:empty (nth 7) with
     | Doctest.Skip_data -> true | _ -> false);
  Doctest.rm_rf empty;
  Doctest.rm_rf basedir

let () =
  Alcotest.run "doctest"
    [ ("classify",
       [ Alcotest.test_case "every verdict bucket" `Quick test_classifications
       ; Alcotest.test_case "fail / preamble / data are non-vacuous" `Quick test_non_vacuous
       ]) ]
