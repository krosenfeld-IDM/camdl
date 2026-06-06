(* Self-test for the doc-doctest classifier (Doctest module in the compiler lib).

   Drives the real extractor + classifier over a fixture Markdown file whose
   blocks are ordered to exercise every verdict bucket, and asserts the verdict
   of each block by position. This is the non-vacuous proof that the gate can
   both pass the right blocks and FAIL a broken one — a classifier that only
   ever returned Pass would fail block 5, and one that never skipped would fail
   blocks 2-4 and 6. *)

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
  ; "skip:data"      (* 4. read() *)
  ; "fail"           (* 5. complete model, E300 *)
  ; "skip:ignore"    (* 6. ```camdl ignore *)
  ; "pass"           (* 7. fragment + context=demo *)
  ]

let test_classifications () =
  let blocks = Doctest.extract_blocks spec in
  let got = List.map (fun b -> verdict_name (Doctest.classify b)) blocks in
  Alcotest.(check int) "block count" (List.length expected) (List.length blocks);
  List.iteri
    (fun i (want, b) ->
       let g = verdict_name (Doctest.classify b) in
       Alcotest.(check string)
         (Printf.sprintf "block %d @ L%d" (i + 1) b.Doctest.line) want g)
    (List.combine expected blocks);
  ignore got

(* Negative control: the FAIL block (5) must actually carry an E300 error, and
   the context block (7) must compile only because the hidden preamble supplied
   N0/I0/gamma — strip the context and it would fail. *)
let test_fail_is_real () =
  let blocks = Doctest.extract_blocks spec in
  let fail_block = List.nth blocks 4 in
  (match Doctest.classify fail_block with
   | Doctest.Fail errs ->
     let codes = List.map (fun (d : Diagnostics.diagnostic) -> d.code) errs in
     Alcotest.(check bool) "fail block reports E300" true (List.mem "E300" codes)
   | other ->
     Alcotest.failf "expected Fail, got %s" (verdict_name other));
  (* Same block body without the context must NOT pass. *)
  let ctx_block = { (List.nth blocks 6) with Doctest.context = None } in
  Alcotest.(check bool) "context block fails without its preamble" false
    (match Doctest.classify ctx_block with Doctest.Pass -> true | _ -> false)

let () =
  Alcotest.run "doctest"
    [ ("classify",
       [ Alcotest.test_case "every verdict bucket" `Quick test_classifications
       ; Alcotest.test_case "fail and context are non-vacuous" `Quick test_fail_is_real
       ]) ]
