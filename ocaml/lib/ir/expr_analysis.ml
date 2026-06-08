(* Dependency classification for IR expressions.

   A single source of truth for "what does this expression depend on?",
   replacing the two ad-hoc one-bit classifiers that exist today (each a
   projection of this lattice, computed independently per language):
   - OCaml `autodiff.ml` treats `BindingRef` as param-free (d/dp = 0);
   - Rust `resolved_expr.rs::references_state` treats `BindingRef` as
     state-derived.

   The dependency classes form a join-semilattice. `Const` is the least
   element (a literal depends on nothing); `join` returns the more-dynamic
   of two classes. The chain is the declaration order below:

     Const  <  Data  <  Param  <  Time  <  State  <  Projected

   so `join` is "the one further along the chain". The chosen order makes
   the existing booleans clean projections:
     references_state(e)  ≡  dep e ⊒ State   (State or Projected)
     param-free(e)        ≡  dep e ≠ Param   (a binding body is param-free
                              iff its class is not exactly Param — the
                              expander hoists only Const/Data/Time/State
                              bodies, never Param).

   Pure and total: every constructor of `Ir.expr` is classified, no
   exceptions, no side effects. *)

(* NB: we do NOT `open Ir` — this module's [dep] constructors `Const` and
   `Param` would shadow `Ir.Const` / `Ir.Param`. IR expression
   constructors are qualified explicitly in the match below. *)

type dep =
  | Const      (* literal: depends on nothing *)
  | Data       (* compile-time table data (constant-indexed lookups) *)
  | Param      (* model parameter (estimable / runtime-supplied) *)
  | Time       (* simulation time, dt, or a time function of them *)
  | State      (* compartment populations (varies as the system advances) *)
  | Projected  (* projection output in a likelihood (state-derived) *)

(* Position in the chain. join is max-by-rank, which is a valid
   semilattice join precisely because the order is a total chain. *)
let rank = function
  | Const     -> 0
  | Data      -> 1
  | Param     -> 2
  | Time      -> 3
  | State     -> 4
  | Projected -> 5

let join a b = if rank a >= rank b then a else b

let join_list ds = List.fold_left join Const ds

(* Lowercase one-word label, for reports / diagnostics. *)
let dep_name = function
  | Const     -> "const"
  | Data      -> "data"
  | Param     -> "param"
  | Time      -> "time"
  | State     -> "state"
  | Projected -> "projected"

(* Classify an expression. [binding_dep name] resolves a [BindingRef] to
   the (already-computed) class of the named binding; callers that have no
   binding environment can pass [fun _ -> Const] (a BindingRef then floors
   to Const, which is only safe when the model has no bindings — use
   [model_binding_deps] otherwise). *)
let dep_of_expr ~binding_dep (e : Ir.expr) : dep =
  let rec go : Ir.expr -> dep = function
    | Ir.Const _ -> Const
    | Ir.Param _ -> Param
    | Ir.Pop _ | Ir.PopSum _ -> State
    | Ir.Time | Ir.Dt | Ir.TimeFunc _ -> Time
    (* A table lookup is at least Data (its compile-time cells); its index
       expressions may pull it more-dynamic (e.g. a state-indexed lookup). *)
    | Ir.TableLookup (_, idxs) -> join Data (join_list (List.map go idxs))
    | Ir.BindingRef name -> binding_dep name
    | Ir.Projected -> Projected
    | Ir.BinOp b -> join (go b.left) (go b.right)
    | Ir.UnOp u  -> go u.arg
    | Ir.Cond c  -> join (go c.pred) (join (go c.then_) (go c.else_))
    | Ir.Reduce terms -> join_list (List.map go terms)
    | Ir.UncheckedDim u -> go u.inner
  in
  go e

(* Compute each binding's dep in topological order. `m.bindings` is
   topo-ordered (a BindingRef only references an earlier binding), so a
   single forward pass suffices: each binding resolves its own
   BindingRefs against the deps already accumulated. An unknown name
   (shouldn't happen on a valid model) floors to Const. *)
let model_binding_deps (m : Ir.model) : string -> dep =
  let tbl : (string, dep) Hashtbl.t = Hashtbl.create 16 in
  let lookup name = match Hashtbl.find_opt tbl name with Some d -> d | None -> Const in
  List.iter
    (fun (b : Ir.binding) ->
      let d = dep_of_expr ~binding_dep:lookup b.bexpr in
      Hashtbl.replace tbl b.bname d)
    m.bindings;
  lookup
