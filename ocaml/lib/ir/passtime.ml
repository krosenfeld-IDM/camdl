(* Lightweight, env-gated per-pass timing for the compiler.

   Active only when CAMDL_TIME_PASSES is set to a non-empty value; otherwise
   every entry point is a couple of no-op nanoseconds. When active it records a
   (label, cpu_seconds) pair per compiler pass and dumps a breakdown to stderr
   at the end of a `camdlc` compile — never to stdout, so the emitted IR is
   byte-for-byte unaffected.

   This is the compile-side analogue of CAMDL_TRACE_STEPS in the Rust runtime:
   a diagnostic channel for "which pass dominates", used by the bench_compile
   harness and ad-hoc profiling. Uses Sys.time (processor time, stdlib) to
   avoid pulling unix into the ir/compiler libraries; for the CPU-bound,
   single-threaded compiler this tracks wall time closely (GC time included,
   which is what we want — allocation-heavy passes pay for their garbage). *)

let enabled : bool Lazy.t =
  lazy (match Sys.getenv_opt "CAMDL_TIME_PASSES" with
        | Some s -> s <> "" && s <> "0"
        | None -> false)

(* Insertion-ordered (label, seconds) records, newest first. *)
let records : (string * float) list ref = ref []

(** [record label dt] logs [dt] seconds against [label]. No-op when disabled. *)
let record (label : string) (dt : float) : unit =
  if Lazy.force enabled then records := (label, dt) :: !records

(** [time label f] runs [f], records its processor time under [label], and
    returns its result. Exceptions propagate (no time recorded on failure). *)
let time (label : string) (f : unit -> 'a) : 'a =
  if not (Lazy.force enabled) then f ()
  else begin
    let t0 = Sys.time () in
    let r = f () in
    record label (Sys.time () -. t0);
    r
  end

(** [dump ()] writes the collected breakdown to stderr. No-op when disabled or
    when nothing was recorded. Safe to call once at the end of a compile. *)
let dump () : unit =
  if Lazy.force enabled && !records <> [] then begin
    let rows = List.rev !records in
    let total = List.fold_left (fun a (_, dt) -> a +. dt) 0.0 rows in
    Printf.eprintf "\n[camdl pass timing]  (processor seconds, CAMDL_TIME_PASSES)\n";
    List.iter (fun (label, dt) ->
      let pct = if total > 0.0 then 100.0 *. dt /. total else 0.0 in
      Printf.eprintf "  %-12s %9.3f s  %5.1f%%\n" label dt pct
    ) rows;
    Printf.eprintf "  %-12s %9.3f s  100.0%%\n" "TOTAL" total
  end
