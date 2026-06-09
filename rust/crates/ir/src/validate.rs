use std::collections::HashSet;
use thiserror::Error;
use crate::{
    expr::Expr,
    model::{CompartmentKind, Model},
};

#[derive(Debug, Error)]
pub enum ValidationError {
    #[error("duplicate compartment name: {0}")]
    DuplicateCompartment(String),

    #[error("duplicate transition name: {0}")]
    DuplicateTransition(String),

    #[error("duplicate parameter name: {0}")]
    DuplicateParameter(String),

    #[error("transition '{transition}' stoichiometry references unknown compartment '{compartment}'")]
    UnknownCompartmentInStoichiometry { transition: String, compartment: String },

    #[error("transition '{transition}' stoichiometry entry has zero delta for '{compartment}'")]
    ZeroDeltaInStoichiometry { transition: String, compartment: String },

    #[error("transition '{transition}' stoichiometry references real compartment '{compartment}'; real compartments cannot appear in stoichiometry")]
    RealCompartmentInStoichiometry { transition: String, compartment: String },

    #[error("real compartment '{0}' has no ODE equation")]
    MissingOdeEquation(String),

    #[error("ODE equation targets '{0}' which is not a real compartment")]
    OdeForNonRealCompartment(String),

    #[error("expression references unknown parameter '{0}'")]
    UnknownParameter(String),

    #[error("expression references unknown compartment '{0}'")]
    UnknownCompartment(String),

    #[error("expression references unknown table '{0}'")]
    UnknownTable(String),

    #[error("expression references unknown time function '{0}'")]
    UnknownTimeFunction(String),

    #[error("observation '{obs}' cumulative_flow references unknown transition '{transition}'")]
    UnknownTransitionInObservation { obs: String, transition: String },

    #[error("parameter '{0}': prior and hierarchical are mutually exclusive — \
             a parameter is either fitted under a single-level prior or pooled \
             under a hierarchical prior, not both")]
    PriorAndHierarchicalBothSet(String),

    #[error("intervention '{intervention}' action references unknown compartment '{compartment}'")]
    UnknownCompartmentInIntervention { intervention: String, compartment: String },

    #[error("balance constraint targets unknown compartment '{0}'")]
    UnknownCompartmentInBalance(String),

    #[error("initial condition references unknown compartment '{0}'")]
    UnknownCompartmentInInitialConditions(String),

    #[error("table lookup of '{table}' has wrong arity: {got} indices but the IR table \
             is rank-1 (multi-dimensional tables are pre-flattened by the compiler to a \
             single linear index, so a lookup must carry exactly 1 index)")]
    TableLookupArity { table: String, got: usize },

    #[error("initial value for compartment '{compartment}' is not finite (got {value}); \
             initial conditions must be finite numbers")]
    InitialValueNotFinite { compartment: String, value: f64 },

    #[error("initial value for compartment '{compartment}' must be nonnegative (got {value}); \
             a compartment cannot start with a negative population")]
    InitialValueNegative { compartment: String, value: f64 },

    #[error("initial value for integer compartment '{compartment}' must be a whole number \
             (got {value}); a fractional value would be silently truncated. Round it to a \
             whole count, or declare the compartment real if fractional state is intended")]
    InitialValueNotInteger { compartment: String, value: f64 },
}

pub fn validate(model: &Model) -> Result<(), Vec<ValidationError>> {
    let mut errors = Vec::new();

    // ── Build name sets ───────────────────────────────────────────────────────

    let mut comp_names:  HashSet<&str> = HashSet::new();
    let mut real_comps:  HashSet<&str> = HashSet::new();
    let mut int_comps:   HashSet<&str> = HashSet::new();
    let mut param_names: HashSet<&str> = HashSet::new();
    let mut table_names: HashSet<&str> = HashSet::new();
    let mut tf_names:    HashSet<&str> = HashSet::new();
    let mut tr_names:    HashSet<&str> = HashSet::new();

    for c in &model.compartments {
        if !comp_names.insert(c.name.as_str()) {
            errors.push(ValidationError::DuplicateCompartment(c.name.clone()));
        }
        match c.kind {
            CompartmentKind::Real    => { real_comps.insert(c.name.as_str()); }
            CompartmentKind::Integer => { int_comps.insert(c.name.as_str()); }
        }
    }

    for p in &model.parameters {
        if !param_names.insert(p.name.as_str()) {
            errors.push(ValidationError::DuplicateParameter(p.name.clone()));
        }
        if p.prior.is_some() && p.hierarchical.is_some() {
            errors.push(ValidationError::PriorAndHierarchicalBothSet(p.name.clone()));
        }
    }
    for t in &model.tables {
        table_names.insert(t.name.as_str());
    }
    for tf in &model.time_functions {
        tf_names.insert(tf.name.as_str());
    }
    for tr in &model.transitions {
        if !tr_names.insert(tr.name.as_str()) {
            errors.push(ValidationError::DuplicateTransition(tr.name.clone()));
        }
    }

    // ── Stoichiometry checks ──────────────────────────────────────────────────

    for tr in &model.transitions {
        for entry in &tr.stoichiometry {
            let comp = &entry.0;
            let delta = entry.1;
            if !comp_names.contains(comp.as_str()) {
                errors.push(ValidationError::UnknownCompartmentInStoichiometry {
                    transition: tr.name.clone(),
                    compartment: comp.clone(),
                });
            } else if real_comps.contains(comp.as_str()) {
                errors.push(ValidationError::RealCompartmentInStoichiometry {
                    transition: tr.name.clone(),
                    compartment: comp.clone(),
                });
            }
            if delta == 0 {
                errors.push(ValidationError::ZeroDeltaInStoichiometry {
                    transition: tr.name.clone(),
                    compartment: comp.clone(),
                });
            }
        }
    }

    // ── ODE equation checks ───────────────────────────────────────────────────

    let ode_comps: HashSet<&str> = model.ode_equations.iter().map(|e| e.compartment.as_str()).collect();
    for rc in &real_comps {
        if !ode_comps.contains(*rc) {
            errors.push(ValidationError::MissingOdeEquation(rc.to_string()));
        }
    }
    for eq in &model.ode_equations {
        if !real_comps.contains(eq.compartment.as_str()) {
            errors.push(ValidationError::OdeForNonRealCompartment(eq.compartment.clone()));
        }
    }

    // ── Expression reference checks ───────────────────────────────────────────

    let ctx = RefCtx { comp_names: &comp_names, param_names: &param_names, table_names: &table_names, tf_names: &tf_names };

    for tr in &model.transitions {
        check_expr(&tr.rate, &ctx, false, &mut errors);
    }
    for eq in &model.ode_equations {
        check_expr(&eq.derivative, &ctx, false, &mut errors);
    }
    for obs in &model.observations {
        // projection
        match &obs.projection {
            crate::observation::Projection::CumulativeFlow(tn) => {
                if !tr_names.contains(tn.as_str()) {
                    errors.push(ValidationError::UnknownTransitionInObservation {
                        obs: obs.name.clone(),
                        transition: tn.clone(),
                    });
                }
            }
            crate::observation::Projection::CumulativeFlowSum(tns) => {
                for tn in tns {
                    if !tr_names.contains(tn.as_str()) {
                        errors.push(ValidationError::UnknownTransitionInObservation {
                            obs: obs.name.clone(),
                            transition: tn.clone(),
                        });
                    }
                }
            }
            _ => {}
        }
        // likelihood exprs (projected is allowed)
        check_likelihood_exprs(&obs.likelihood, &ctx, &mut errors);
    }

    // ── Intervention & event action target checks (gh#123) ────────────────────
    //
    // Interventions (`interventions {}`) and events (`events {}`, marked
    // `always_active`) both lower to `Intervention` in the IR. Every action
    // names compartment(s) it modifies; a dangling target reaches the runtime
    // as a silent no-op or an out-of-range panic. Validate the names — and
    // recurse into the action value/count/fraction expressions, which may
    // reference params/compartments/tables.
    for iv in &model.interventions {
        let check_target = |comp: &str, errors: &mut Vec<ValidationError>| {
            if !comp_names.contains(comp) {
                errors.push(ValidationError::UnknownCompartmentInIntervention {
                    intervention: iv.name.clone(),
                    compartment: comp.to_string(),
                });
            }
        };
        for action in &iv.actions {
            use crate::intervention::Action;
            match action {
                Action::FractionTransfer(ft) => {
                    check_target(&ft.src, &mut errors);
                    check_target(&ft.dst, &mut errors);
                    check_expr(&ft.fraction, &ctx, false, &mut errors);
                }
                Action::AbsoluteTransfer(at) => {
                    check_target(&at.src, &mut errors);
                    check_target(&at.dst, &mut errors);
                    check_expr(&at.count, &ctx, false, &mut errors);
                }
                Action::Set(s) => {
                    check_target(&s.compartment, &mut errors);
                    check_expr(&s.value, &ctx, false, &mut errors);
                }
                Action::Add(a) => {
                    check_target(&a.compartment, &mut errors);
                    check_expr(&a.count, &ctx, false, &mut errors);
                }
            }
        }
    }

    // ── Balance constraint target check (gh#123) ──────────────────────────────
    //
    // The balance constraint overwrites its target compartment with `expr`
    // every substep. A dangling target silently does nothing.
    if let Some(b) = &model.balance {
        if !comp_names.contains(b.target.as_str()) {
            errors.push(ValidationError::UnknownCompartmentInBalance(b.target.clone()));
        }
        check_expr(&b.expr, &ctx, false, &mut errors);
    }

    // ── Initial-condition key checks (gh#114 Rust-side) ────────────────────────
    //
    // Every init key must resolve to a declared (expanded) compartment — a
    // stratified model can otherwise carry an init value for nonexistent `S`
    // while the real cells (e.g. `S_child_kano`) default to zero, silently
    // starting the epidemic in an empty population. The Parameterized variant
    // also carries an expression per key; recurse into it.
    //
    // ── Initial-condition VALUE domain checks (gh#124) ─────────────────────────
    //
    // For the Explicit variant the IR carries a concrete f64 per compartment.
    // The runtime converts an integer init via `*val as i64`
    // (compiled_model.rs), which truncates and saturates: 0.6 → 0, -3 → a
    // negative compartment from t=0, NaN/inf → 0 / i64::MAX. Each is a "model
    // runs but starts in the wrong population" failure, so we reject them here
    // at the contract boundary:
    //   - non-finite (NaN / ±inf) for any compartment,
    //   - negative for any compartment (a count is nonnegative, int or real),
    //   - non-integer for INTEGER compartments (a near-integer tolerance allows
    //     for float round-trip noise; a clearly-fractional value errors).
    // Real compartments may hold fractional (but finite, nonnegative) values.
    // Parameterized / FromDistribution inits carry expressions / priors rather
    // than literals, so there is nothing to range-check statically here; their
    // values are produced (and bounds-enforced) at sim/inference time.
    {
        use crate::model::InitialConditions;
        // Tolerance for the integer check: a value within this of its nearest
        // integer is treated as that integer (absorbs float round-trip noise
        // like 3.0000000001). Mirrors the `1e-9` tolerance the issue specifies
        // for `checked_int_initial_value`.
        const INT_TOL: f64 = 1e-9;
        let check_init_key = |comp: &str, errors: &mut Vec<ValidationError>| {
            if !comp_names.contains(comp) {
                errors.push(ValidationError::UnknownCompartmentInInitialConditions(
                    comp.to_string(),
                ));
            }
        };
        let check_init_value = |comp: &str, v: f64, errors: &mut Vec<ValidationError>| {
            // Only meaningful for declared compartments; the key check above
            // already reports an unknown name, so skip the value check for it
            // (we can't classify int-vs-real for a name we don't know).
            if !comp_names.contains(comp) {
                return;
            }
            if !v.is_finite() {
                errors.push(ValidationError::InitialValueNotFinite {
                    compartment: comp.to_string(),
                    value: v,
                });
                return;
            }
            if v < 0.0 {
                errors.push(ValidationError::InitialValueNegative {
                    compartment: comp.to_string(),
                    value: v,
                });
                return;
            }
            if int_comps.contains(comp) && (v - v.round()).abs() > INT_TOL {
                errors.push(ValidationError::InitialValueNotInteger {
                    compartment: comp.to_string(),
                    value: v,
                });
            }
        };
        match &model.initial_conditions {
            InitialConditions::Explicit(map) => {
                for (k, v) in map {
                    check_init_key(k, &mut errors);
                    check_init_value(k, *v, &mut errors);
                }
            }
            InitialConditions::Parameterized(map) => {
                for (k, e) in map {
                    check_init_key(k, &mut errors);
                    check_expr(e, &ctx, false, &mut errors);
                }
            }
            InitialConditions::FromDistribution(map) => {
                for k in map.keys() {
                    check_init_key(k, &mut errors);
                }
            }
        }
    }

    if !errors.is_empty() {
        return Err(errors);
    }
    Ok(())
}

struct RefCtx<'a> {
    comp_names:  &'a HashSet<&'a str>,
    param_names: &'a HashSet<&'a str>,
    table_names: &'a HashSet<&'a str>,
    tf_names:    &'a HashSet<&'a str>,
}

fn check_expr(expr: &Expr, ctx: &RefCtx<'_>, allow_projected: bool, errors: &mut Vec<ValidationError>) {
    match expr {
        Expr::Const(_) | Expr::Time(_) | Expr::Dt(_) => {}
        Expr::Projected(_) => {
            // Allow in likelihood context; validate at call-site via allow_projected
            // (we pass allow_projected=true from check_likelihood_exprs)
            if !allow_projected {
                // We don't emit an error here currently; the schema validator handles it.
            }
        }
        Expr::Param(p) => {
            if !ctx.param_names.contains(p.param.as_str()) {
                errors.push(ValidationError::UnknownParameter(p.param.clone()));
            }
        }
        Expr::Pop(p) => {
            if !ctx.comp_names.contains(p.pop.as_str()) {
                errors.push(ValidationError::UnknownCompartment(p.pop.clone()));
            }
        }
        Expr::PopSum(ps) => {
            for name in &ps.pop_sum {
                if !ctx.comp_names.contains(name.as_str()) {
                    errors.push(ValidationError::UnknownCompartment(name.clone()));
                }
            }
        }
        Expr::BinOp(w) => {
            check_expr(&w.bin_op.left,  ctx, allow_projected, errors);
            check_expr(&w.bin_op.right, ctx, allow_projected, errors);
        }
        Expr::UnOp(w) => {
            check_expr(&w.un_op.arg, ctx, allow_projected, errors);
        }
        Expr::Cond(w) => {
            check_expr(&w.cond.pred,  ctx, allow_projected, errors);
            check_expr(&w.cond.then,  ctx, allow_projected, errors);
            check_expr(&w.cond.else_, ctx, allow_projected, errors);
        }
        Expr::TimeFunc(w) => {
            if !ctx.tf_names.contains(w.time_func.name.as_str()) {
                errors.push(ValidationError::UnknownTimeFunction(w.time_func.name.clone()));
            }
        }
        Expr::TableLookup(w) => {
            if !ctx.table_names.contains(w.table_lookup.table.as_str()) {
                errors.push(ValidationError::UnknownTable(w.table_lookup.table.clone()));
            }
            // Arity check (gh#123, reviewer feedback on the prior #123 attempt):
            // the IR table is rank-1 — the OCaml compiler pre-flattens any
            // multi-dimensional table to a single linear index, and the
            // runtime evaluator rejects any other count (propensity.rs /
            // resolved_expr.rs). A lookup carrying ≠1 index is malformed IR; we
            // reject it here at the contract boundary rather than deferring to a
            // runtime eval error. This is an item-count (arity) check, NOT an
            // out-of-range linear-index check — the runtime already rejects a
            // fully out-of-range index via OobPolicy::Error (gh#112 is the
            // OCaml-side under-index-selects-wrong-cell fix, not this).
            if w.table_lookup.indices.len() != 1 {
                errors.push(ValidationError::TableLookupArity {
                    table: w.table_lookup.table.clone(),
                    got: w.table_lookup.indices.len(),
                });
            }
            for idx in &w.table_lookup.indices {
                check_expr(idx, ctx, allow_projected, errors);
            }
        }
        Expr::UncheckedDim(w) => {
            // Recurse into the inner expression for name-resolution
            // checks — the escape only affects dim-check, not name
            // resolution.
            check_expr(&w.unchecked_dim.inner, ctx, allow_projected, errors);
        }
        Expr::Reduce(w) => {
            for t in &w.reduce {
                check_expr(t, ctx, allow_projected, errors);
            }
        }
        // Leaf: binding-name resolution happens at CompiledModel::new (binding_index).
        Expr::BindingRef(_) => {}
    }
}

fn check_likelihood_exprs(
    likelihood: &crate::observation::Likelihood,
    ctx: &RefCtx<'_>,
    errors: &mut Vec<ValidationError>,
) {
    use crate::observation::Likelihood;
    match likelihood {
        Likelihood::Poisson(l)      => check_expr(&l.rate, ctx, true, errors),
        Likelihood::NegBinomial(l)  => {
            check_expr(&l.mean, ctx, true, errors);
            check_expr(&l.dispersion, ctx, true, errors);
        }
        Likelihood::Normal(l) => {
            check_expr(&l.mean, ctx, true, errors);
            check_expr(&l.sd,   ctx, true, errors);
        }
        Likelihood::Binomial(l) => {
            check_expr(&l.n, ctx, true, errors);
            check_expr(&l.p, ctx, true, errors);
        }
        Likelihood::BetaBinomial(l) => {
            check_expr(&l.n,     ctx, true, errors);
            check_expr(&l.alpha, ctx, true, errors);
            check_expr(&l.beta,  ctx, true, errors);
        }
        Likelihood::Bernoulli(l) => {
            check_expr(&l.p, ctx, true, errors);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parameter::{Parameter, PriorDist, NormalPrior, HierarchicalKind, HierarchicalPrior};

    fn param_both_set() -> Parameter {
        Parameter {
            name:          "beta".into(),
            value:         Some(1.0),
            bounds:        None,
            prior:         Some(PriorDist::Normal(NormalPrior { mean: 0.0, sd: 1.0 })),
            hierarchical:  Some(HierarchicalPrior {
                kind: HierarchicalKind::Normal,
                args: Default::default(),
                pool_over: "".into(),
            }),
            transform:     None,
            initial_value: None,
            param_kind:    None,
            param_dim:     None,
        }
    }

    fn param_only_prior() -> Parameter {
        let mut p = param_both_set();
        p.hierarchical = None;
        p
    }

    fn param_only_hierarchical() -> Parameter {
        let mut p = param_both_set();
        p.prior = None;
        p
    }

    fn load_sir() -> Model {
        let s = std::fs::read_to_string(concat!(
            env!("CARGO_MANIFEST_DIR"), "/../../../ir/golden/sir_basic.ir.json"))
            .expect("read sir_basic.ir.json");
        // gh#audit-C8. Use envelope-aware deserializer.
        crate::from_str(&s).expect("parse sir_basic")
    }

    #[test]
    fn prior_and_hierarchical_both_set_is_rejected() {
        let mut m = load_sir();
        m.parameters.push(param_both_set());
        let errs = validate(&m).expect_err("must reject parameter with both fields set");
        assert!(errs.iter().any(|e| matches!(e,
            ValidationError::PriorAndHierarchicalBothSet(name) if name == "beta")),
            "expected PriorAndHierarchicalBothSet for 'beta', got: {:?}", errs);
    }

    #[test]
    fn only_prior_is_accepted() {
        let mut m = load_sir();
        // Use a fresh name to avoid the duplicate-parameter check tripping.
        let mut p = param_only_prior();
        p.name = "beta_extra".into();
        m.parameters.push(p);
        validate(&m).expect("only prior set must validate");
    }

    #[test]
    fn only_hierarchical_is_accepted() {
        let mut m = load_sir();
        let mut p = param_only_hierarchical();
        p.name = "beta_extra".into();
        m.parameters.push(p);
        validate(&m).expect("only hierarchical set must validate");
    }

    // ── gh#123: reference checks for intervention/event targets, balance,
    //    init keys, and table-lookup arity ──────────────────────────────────

    use crate::intervention::{
        Action, FractionTransfer, Intervention, InterventionSchedule, SetAction,
    };
    use crate::model::{BalanceSpec, InitialConditions};
    use crate::expr::{Expr, TableLookupExpr, TableLookupWrap};
    use crate::table::{OobPolicy, Table, TableSource};

    /// (1a) An intervention `set`/`add` action whose target compartment does
    /// not exist must be rejected. The runtime would otherwise silently no-op
    /// or panic on an out-of-range index (gh#123).
    #[test]
    fn intervention_set_target_unknown_compartment_is_rejected() {
        let mut m = load_sir();
        m.interventions.push(Intervention {
            name: "shock".into(),
            base_name: None,
            schedule: InterventionSchedule::AtTimes(vec![10.0]),
            actions: vec![Action::Set(SetAction {
                compartment: "Q".into(), // not declared (model has S, I, R)
                value: Expr::const_(0.0),
            })],
            always_active: false,
        });
        let errs = validate(&m).expect_err("must reject intervention targeting unknown 'Q'");
        assert!(
            errs.iter().any(|e| matches!(e,
                ValidationError::UnknownCompartmentInIntervention { intervention, compartment }
                    if intervention == "shock" && compartment == "Q")),
            "expected UnknownCompartmentInIntervention for 'shock'/'Q', got: {:?}", errs);
    }

    /// (1b) An event (always_active intervention) `transfer` action whose
    /// `dst` does not exist must be rejected — events fire every substep, so a
    /// dangling target is a hard model bug.
    #[test]
    fn event_transfer_dst_unknown_compartment_is_rejected() {
        let mut m = load_sir();
        m.interventions.push(Intervention {
            name: "import".into(),
            base_name: None,
            schedule: InterventionSchedule::AtTimes(vec![1.0]),
            actions: vec![Action::FractionTransfer(FractionTransfer {
                src: "S".into(),         // declared
                dst: "Nowhere".into(),   // not declared
                fraction: Expr::const_(0.1),
            })],
            always_active: true,
        });
        let errs = validate(&m).expect_err("must reject transfer to unknown 'Nowhere'");
        assert!(
            errs.iter().any(|e| matches!(e,
                ValidationError::UnknownCompartmentInIntervention { intervention, compartment }
                    if intervention == "import" && compartment == "Nowhere")),
            "expected UnknownCompartmentInIntervention for 'import'/'Nowhere', got: {:?}", errs);
    }

    /// (2) A balance constraint whose target compartment does not exist must be
    /// rejected. The runtime overwrites the target each substep; a dangling
    /// target silently does nothing.
    #[test]
    fn balance_target_unknown_compartment_is_rejected() {
        let mut m = load_sir();
        m.balance = Some(BalanceSpec {
            target: "Residual".into(), // not declared
            expr: Expr::const_(0.0),
        });
        let errs = validate(&m).expect_err("must reject balance targeting unknown 'Residual'");
        assert!(
            errs.iter().any(|e| matches!(e,
                ValidationError::UnknownCompartmentInBalance(c) if c == "Residual")),
            "expected UnknownCompartmentInBalance for 'Residual', got: {:?}", errs);
    }

    /// (3) gh#114 Rust-side: an initial-condition key that does not resolve to
    /// a declared (expanded) compartment must be rejected. A stratified model
    /// can otherwise carry an init value for nonexistent `S` while the real
    /// cells default to zero — a plausible-but-wrong epidemic.
    #[test]
    fn init_key_unknown_compartment_is_rejected() {
        let mut m = load_sir();
        // sir_basic uses Parameterized init keyed on S/I; add a dangling key.
        match &mut m.initial_conditions {
            InitialConditions::Parameterized(map) => {
                map.insert("S_ghost".into(), Expr::const_(0.0));
            }
            other => panic!("expected Parameterized init in sir_basic, got {:?}", other),
        }
        let errs = validate(&m).expect_err("must reject init key for unknown 'S_ghost'");
        assert!(
            errs.iter().any(|e| matches!(e,
                ValidationError::UnknownCompartmentInInitialConditions(c) if c == "S_ghost")),
            "expected UnknownCompartmentInInitialConditions for 'S_ghost', got: {:?}", errs);
    }

    /// (4) A table-lookup whose index ARITY differs from the IR table's rank
    /// (1, since the compiler pre-flattens multi-dim tables to a single linear
    /// index) must be rejected by validation, not deferred to a runtime eval
    /// error. This is the arity check, NOT an out-of-range linear index (the
    /// runtime already rejects out-of-range via OobPolicy::Error).
    #[test]
    fn table_lookup_wrong_arity_is_rejected() {
        let mut m = load_sir();
        m.tables.push(Table {
            name: "kernel".into(),
            source: TableSource::Inline {
                values: vec![Expr::const_(1.0), Expr::const_(2.0)],
            },
            out_of_bounds: OobPolicy::Error,
            cell_kind: None,
        });
        // A two-index lookup against the rank-1 IR table: wrong arity.
        let two_index_lookup = Expr::TableLookup(TableLookupWrap {
            table_lookup: TableLookupExpr {
                table: "kernel".into(),
                indices: vec![Expr::const_(0.0), Expr::const_(1.0)],
            },
        });
        // Plant the lookup in a transition rate (a checked Expr location).
        m.transitions[0].rate = two_index_lookup;
        let errs = validate(&m).expect_err("must reject 2-index lookup against rank-1 table");
        assert!(
            errs.iter().any(|e| matches!(e,
                ValidationError::TableLookupArity { table, got } if table == "kernel" && *got == 2)),
            "expected TableLookupArity for 'kernel' got=2, got: {:?}", errs);
    }

    /// Negative control for arity: a correct single-index lookup must validate.
    #[test]
    fn table_lookup_single_index_is_accepted() {
        let mut m = load_sir();
        m.tables.push(Table {
            name: "kernel".into(),
            source: TableSource::Inline {
                values: vec![Expr::const_(1.0), Expr::const_(2.0)],
            },
            out_of_bounds: OobPolicy::Error,
            cell_kind: None,
        });
        m.transitions[0].rate = Expr::TableLookup(TableLookupWrap {
            table_lookup: TableLookupExpr {
                table: "kernel".into(),
                indices: vec![Expr::const_(0.0)],
            },
        });
        validate(&m).expect("single-index lookup against rank-1 table must validate");
    }

    /// Negative control: the unmodified sir_basic model (with valid init keys,
    /// no interventions, no balance) must validate.
    #[test]
    fn sir_basic_validates() {
        let m = load_sir();
        validate(&m).expect("sir_basic.ir.json must validate");
    }

    // ── gh#124: explicit initial-condition VALUE domain checks ────────────────
    //
    // The runtime converts an explicit integer init via `*val as i64`
    // (compiled_model.rs), which truncates and saturates: I0=0.6 → 0 silently,
    // I0=-3 → a negative compartment from t=0, I0=NaN → 0, I0=1e20 → i64::MAX.
    // Each is a "model runs but starts in the wrong population" failure. Reject
    // them at the contract boundary instead.

    /// sir_basic is Parameterized; swap in an Explicit init map keyed on the
    /// model's (integer) compartments so the VALUE-domain checks have something
    /// to inspect.
    fn sir_with_explicit_init(s: f64, i: f64, r: f64) -> Model {
        let mut m = load_sir();
        let mut map = std::collections::HashMap::new();
        map.insert("S".to_string(), s);
        map.insert("I".to_string(), i);
        map.insert("R".to_string(), r);
        m.initial_conditions = InitialConditions::Explicit(map);
        m
    }

    /// (124a) A negative explicit init value must be rejected — a negative
    /// compartment from t=0 is never physical (population counts are
    /// nonnegative). Reproduces the `I0 = -3` row of gh#124.
    #[test]
    fn init_value_negative_is_rejected() {
        let m = sir_with_explicit_init(99.0, -3.0, 0.0);
        let errs = validate(&m).expect_err("must reject I0 = -3");
        assert!(
            errs.iter().any(|e| matches!(e,
                ValidationError::InitialValueNegative { compartment, value }
                    if compartment == "I" && *value == -3.0)),
            "expected InitialValueNegative for 'I' = -3, got: {:?}", errs);
    }

    /// (124b) A non-finite explicit init value (NaN) must be rejected — it
    /// converts to 0 under `as i64` with no warning. Reproduces the
    /// `I0 = NaN` row of gh#124.
    #[test]
    fn init_value_nan_is_rejected() {
        let m = sir_with_explicit_init(99.0, f64::NAN, 0.0);
        let errs = validate(&m).expect_err("must reject I0 = NaN");
        assert!(
            errs.iter().any(|e| matches!(e,
                ValidationError::InitialValueNotFinite { compartment, .. }
                    if compartment == "I")),
            "expected InitialValueNotFinite for 'I' = NaN, got: {:?}", errs);
    }

    /// (124b') A positive-infinity explicit init value must be rejected — it
    /// saturates to i64::MAX under `as i64`. Same NaN/inf class as above.
    #[test]
    fn init_value_inf_is_rejected() {
        let m = sir_with_explicit_init(99.0, f64::INFINITY, 0.0);
        let errs = validate(&m).expect_err("must reject I0 = inf");
        assert!(
            errs.iter().any(|e| matches!(e,
                ValidationError::InitialValueNotFinite { compartment, .. }
                    if compartment == "I")),
            "expected InitialValueNotFinite for 'I' = inf, got: {:?}", errs);
    }

    /// (124c) A clearly-fractional explicit init value on an INTEGER
    /// compartment must be rejected, not silently truncated. Reproduces the
    /// `I0 = 0.6` row of gh#124 (which `as i64` truncates to 0).
    #[test]
    fn init_value_fractional_on_integer_compartment_is_rejected() {
        let m = sir_with_explicit_init(99.0, 0.6, 0.0);
        let errs = validate(&m).expect_err("must reject I0 = 0.6 on integer compartment");
        assert!(
            errs.iter().any(|e| matches!(e,
                ValidationError::InitialValueNotInteger { compartment, value }
                    if compartment == "I" && *value == 0.6)),
            "expected InitialValueNotInteger for 'I' = 0.6, got: {:?}", errs);
    }

    /// Negative control: integer-valued explicit inits (including a within-
    /// tolerance near-integer like 3.0 + 1e-12) must validate.
    #[test]
    fn init_value_integer_on_integer_compartment_is_accepted() {
        let m = sir_with_explicit_init(99.0, 1.0 + 1e-12, 0.0);
        validate(&m).expect("near-integer init within tolerance must validate");
    }

    /// (124d) A fractional value on a REAL compartment must be accepted — real
    /// compartments may hold fractional (but nonnegative, finite) values.
    #[test]
    fn init_value_fractional_on_real_compartment_is_accepted() {
        let mut m = load_sir();
        // Make R a real compartment with an ODE so the model still validates
        // structurally, then give it a fractional init.
        for c in &mut m.compartments {
            if c.name == "R" {
                c.kind = CompartmentKind::Real;
            }
        }
        m.ode_equations.push(crate::ode_equation::OdeEquation {
            compartment: "R".into(),
            derivative: Expr::const_(0.0),
        });
        // R no longer participates in integer stoichiometry in sir_basic's
        // recovery transition; drop any stoichiometry entry naming R so the
        // RealCompartmentInStoichiometry check doesn't fire (we're isolating
        // the init-VALUE domain behaviour, not stoichiometry).
        for tr in &mut m.transitions {
            tr.stoichiometry.retain(|e| e.0 != "R");
        }
        let mut map = std::collections::HashMap::new();
        map.insert("S".to_string(), 99.0);
        map.insert("I".to_string(), 1.0);
        map.insert("R".to_string(), 0.6); // fractional on a real compartment: OK
        m.initial_conditions = InitialConditions::Explicit(map);
        validate(&m).expect("fractional init on a real compartment must validate");
    }

    /// (124e) A negative value on a REAL compartment must still be rejected —
    /// population values are nonnegative regardless of int/real.
    #[test]
    fn init_value_negative_on_real_compartment_is_rejected() {
        let mut m = load_sir();
        for c in &mut m.compartments {
            if c.name == "R" {
                c.kind = CompartmentKind::Real;
            }
        }
        m.ode_equations.push(crate::ode_equation::OdeEquation {
            compartment: "R".into(),
            derivative: Expr::const_(0.0),
        });
        for tr in &mut m.transitions {
            tr.stoichiometry.retain(|e| e.0 != "R");
        }
        let mut map = std::collections::HashMap::new();
        map.insert("S".to_string(), 99.0);
        map.insert("I".to_string(), 1.0);
        map.insert("R".to_string(), -0.5);
        m.initial_conditions = InitialConditions::Explicit(map);
        let errs = validate(&m).expect_err("must reject negative init on real compartment");
        assert!(
            errs.iter().any(|e| matches!(e,
                ValidationError::InitialValueNegative { compartment, value }
                    if compartment == "R" && *value == -0.5)),
            "expected InitialValueNegative for 'R' = -0.5, got: {:?}", errs);
    }

    /// Regression guard for the gh#123/gh#114 reference checks: every committed
    /// golden IR (which exercises real interventions, balance, stratified init,
    /// and table lookups) must still validate. A false positive in the new
    /// checks — rejecting legitimate compiler-emitted IR — would surface here.
    #[test]
    fn all_golden_ir_validates() {
        let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/../../../ir/golden");
        let mut checked = 0usize;
        for entry in std::fs::read_dir(dir).expect("read ir/golden dir") {
            let path = entry.expect("dir entry").path();
            let is_ir = path.file_name()
                .and_then(|n| n.to_str())
                .map(|n| n.ends_with(".ir.json"))
                .unwrap_or(false);
            if !is_ir {
                continue;
            }
            let s = std::fs::read_to_string(&path)
                .unwrap_or_else(|e| panic!("read {}: {e}", path.display()));
            let m = crate::from_str(&s)
                .unwrap_or_else(|e| panic!("parse {}: {e}", path.display()));
            validate(&m)
                .unwrap_or_else(|errs| panic!("{} must validate, got: {:?}", path.display(), errs));
            checked += 1;
        }
        assert!(checked > 0, "no golden .ir.json files found under {dir}");
    }
}
