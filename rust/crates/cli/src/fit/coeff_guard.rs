//! NUTS guard: refuse to estimate a parameter that drives only a forcing or
//! inline-table coefficient (proposal `2026-06-09-const-parametric-forcing.md`
//! §4/§6).
//!
//! The value half made forcing/table coefficients live, so IF2 and the
//! bootstrap particle filter (gradient-free) now estimate such a parameter
//! correctly. NUTS still cannot: the compiler's autodiff (`autodiff.ml`)
//! differentiates `TimeFunc` and `TableLookup` to `Const 0.0`, so a parameter
//! referenced ONLY inside a coefficient has an identically-zero dynamics
//! gradient — NUTS would propose against a flat surface and silently
//! mis-sample. Until the gradient half emits those derivatives, reject such a
//! fit loudly rather than return a garbage posterior.
//!
//! Scope (matching §6): `body_refs` are the rate / observation / initial-value
//! expressions; `coeff_refs` are forcing-coefficient and inline-table-value
//! expressions. A parameter in both is not flagged — the body gradient carries
//! it. The coefficient sub-expressions live in `model.time_functions[*].kind`
//! and `model.tables[*]`, not in the rate AST, so this needs its own traversal.

use std::collections::HashSet;

use ir::expr::Expr;
use ir::model::InitialConditions;
use ir::observation::Likelihood;
use ir::time_func::TimeFuncKind;

/// Collect parameter names referenced anywhere in `e`.
fn collect(e: &Expr, out: &mut HashSet<String>) {
    match e {
        Expr::Param(p) => {
            out.insert(p.param.clone());
        }
        Expr::BinOp(w) => {
            collect(&w.bin_op.left, out);
            collect(&w.bin_op.right, out);
        }
        Expr::UnOp(w) => collect(&w.un_op.arg, out),
        Expr::Cond(w) => {
            collect(&w.cond.pred, out);
            collect(&w.cond.then, out);
            collect(&w.cond.else_, out);
        }
        Expr::TableLookup(w) => {
            // The lookup INDEX is a body sub-expression; the table's VALUES are
            // coefficients and are collected separately from `model.tables`.
            for i in &w.table_lookup.indices {
                collect(i, out);
            }
        }
        Expr::Reduce(w) => {
            for t in &w.reduce {
                collect(t, out);
            }
        }
        Expr::UncheckedDim(w) => collect(&w.unchecked_dim.inner, out),
        // Leaves / non-param nodes. A `BindingRef` body is state-only
        // (param-free, enforced at `CompiledModel::new`), so it adds no refs.
        Expr::Const(_)
        | Expr::Pop(_)
        | Expr::PopSum(_)
        | Expr::Time(_)
        | Expr::Dt(_)
        | Expr::TimeFunc(_)
        | Expr::Projected(_)
        | Expr::BindingRef(_) => {}
    }
}

/// Collect parameters in a likelihood's coefficient expressions.
fn collect_likelihood(lik: &Likelihood, out: &mut HashSet<String>) {
    match lik {
        Likelihood::Poisson(p) => collect(&p.rate, out),
        Likelihood::NegBinomial(nb) => {
            collect(&nb.mean, out);
            collect(&nb.dispersion, out);
        }
        Likelihood::Normal(n) => {
            collect(&n.mean, out);
            collect(&n.sd, out);
        }
        Likelihood::Binomial(b) => {
            collect(&b.n, out);
            collect(&b.p, out);
        }
        Likelihood::BetaBinomial(bb) => {
            collect(&bb.n, out);
            collect(&bb.alpha, out);
            collect(&bb.beta, out);
        }
        Likelihood::Bernoulli(b) => collect(&b.p, out),
    }
}

/// Collect parameters in a forcing's scalar coefficient expressions.
fn collect_forcing(kind: &TimeFuncKind, out: &mut HashSet<String>) {
    match kind {
        TimeFuncKind::Sinusoidal(s) => {
            collect(&s.amplitude, out);
            collect(&s.period, out);
            collect(&s.phase, out);
            collect(&s.baseline, out);
        }
        TimeFuncKind::Piecewise(p) => {
            p.breakpoints.iter().for_each(|e| collect(e, out));
            p.values.iter().for_each(|e| collect(e, out));
        }
        TimeFuncKind::Interpolated(i) => {
            i.times.iter().for_each(|e| collect(e, out));
            i.values.iter().for_each(|e| collect(e, out));
        }
        TimeFuncKind::Periodic(p) => {
            collect(&p.period, out);
            p.values.iter().for_each(|e| collect(e, out));
        }
        TimeFuncKind::Fourier(f) => {
            collect(&f.period, out);
            f.harmonics.iter().for_each(|(a, b)| {
                collect(a, out);
                collect(b, out);
            });
        }
        TimeFuncKind::PeriodicSpline(ps) => {
            collect(&ps.period, out);
            ps.coefs.iter().for_each(|e| collect(e, out));
        }
    }
}

/// Estimated parameters that NUTS cannot estimate because their gradient is a
/// silent zero: referenced inside a forcing/table coefficient, present in no
/// rate/observation/initial-value body, **and** carrying no emitted `rate_grad`
/// entry.
///
/// The `rate_grad` exclusion is what keeps this honest after the gradient half
/// (gh#119): the compiler now emits an analytic ∂forcing/∂coef for Sinusoidal
/// and Fourier coefficients and constant-indexed parameter tables, so such a
/// parameter *does* have a usable gradient and must NOT be flagged — otherwise
/// the guard would refuse the very fits the gradient half enables. What remains
/// flagged is the genuinely gradient-less case: a coefficient parameter whose
/// kind has no derivative (it would have failed to compile if it entered a
/// rate — the E600 floor; so this is reachable for a forcing referenced only
/// through an observation, where the obs gradient zeroes the forcing). Sorted,
/// for a deterministic diagnostic.
pub fn coefficient_only_estimated(model: &ir::Model, estimated: &HashSet<String>) -> Vec<String> {
    let mut body = HashSet::new();
    for t in &model.transitions {
        collect(&t.rate, &mut body);
    }
    for o in &model.observations {
        collect_likelihood(&o.likelihood, &mut body);
    }
    if let InitialConditions::Parameterized(map) = &model.initial_conditions {
        for e in map.values() {
            collect(e, &mut body);
        }
    }

    let mut coeff = HashSet::new();
    for tf in &model.time_functions {
        collect_forcing(&tf.kind, &mut coeff);
    }
    for tbl in &model.tables {
        if let Some(values) = tbl.source.values() {
            for e in values {
                collect(e, &mut coeff);
            }
        }
    }

    // Parameters the compiler emitted a derivative for (any transition) — these
    // have a usable NUTS gradient and are never blocked.
    let mut has_grad: HashSet<&str> = HashSet::new();
    for t in &model.transitions {
        for name in t.rate_grad.keys() {
            has_grad.insert(name.as_str());
        }
    }

    let mut offenders: Vec<String> = estimated
        .iter()
        .filter(|p| coeff.contains(*p) && !body.contains(*p) && !has_grad.contains(p.as_str()))
        .cloned()
        .collect();
    offenders.sort();
    offenders
}

/// Error message for a NUTS fit blocked by [`coefficient_only_estimated`].
pub fn nuts_guard_error(offenders: &[String]) -> String {
    format!(
        "NUTS cannot estimate parameter(s) [{}]: each appears only inside a \
         forcing or inline-table coefficient, where the gradient is not yet \
         emitted (the compiler differentiates a forcing/table to zero), so NUTS \
         would sample against a flat surface and silently mis-estimate them. \
         Use IF2 or the bootstrap particle filter (gradient-free, and these \
         parameters now evaluate live), reference the parameter in a transition \
         rate, or run PGAS with --no-nuts.",
        offenders.join(", ")
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use ir::expr::Expr;
    use ir::time_func::{Sinusoidal, TimeFunction};

    fn base_model() -> ir::Model {
        ir::Model {
            name: "t".into(),
            version: "0.3".into(),
            time_unit: "days".into(),
            description: None,
            origin: None,
            origin_rata_die: None,
            compartments: vec![ir::model::Compartment {
                name: "S".into(),
                kind: ir::model::CompartmentKind::Integer,
            }],
            transitions: vec![],
            ode_equations: vec![],
            time_functions: vec![],
            tables: vec![],
            interventions: vec![],
            observations: vec![],
            bindings: vec![],
            parameters: vec![],
            initial_conditions: InitialConditions::Parameterized(HashMap::new()),
            output: ir::model::OutputConfig {
                times: ir::model::OutputSchedule::AtTimes(vec![0.0]),
                format: "tsv".into(),
                trajectory: true,
                observations: false,
            },
            simulation: ir::model::SimulationConfig {
                t_start: 0.0,
                t_end: 1.0,
                time_semantics: "continuous".into(),
                dt: None,
                rng_seed: Some(1),
            },
            presets: vec![],
            model_structure: None,
            balance: None,
            identity_tracked_compartments: vec![],
        }
    }

    fn sinusoidal_forcing(amplitude: Expr) -> TimeFunction {
        TimeFunction {
            name: "seasonal".into(),
            kind: TimeFuncKind::Sinusoidal(Sinusoidal {
                amplitude,
                period: Expr::const_(365.0),
                phase: Expr::const_(0.0),
                baseline: Expr::const_(1.0),
            }),
            dim: (0, 0),
        }
    }

    /// `alpha` drives only the forcing amplitude and no transition emits a
    /// derivative for it (no `rate_grad`) → no usable gradient → flagged. This
    /// is the residual case the guard exists for after the gradient half (e.g.
    /// a forcing referenced only through an observation).
    #[test]
    fn flags_param_only_in_forcing_coefficient() {
        let mut m = base_model();
        m.time_functions = vec![sinusoidal_forcing(Expr::param("alpha"))];
        let estimated: HashSet<String> = ["alpha".to_string()].into_iter().collect();
        assert_eq!(coefficient_only_estimated(&m, &estimated), vec!["alpha".to_string()]);
    }

    /// `alpha` also appears in a rate → the rate gradient carries it → not flagged.
    #[test]
    fn does_not_flag_param_also_in_a_rate() {
        let mut m = base_model();
        m.time_functions = vec![sinusoidal_forcing(Expr::param("alpha"))];
        m.transitions = vec![ir::transition::Transition {
            name: "decay".into(),
            stoichiometry: vec![ir::transition::StoichiometryEntry("S".into(), -1)],
            rate: Expr::bin_op(ir::expr::BinOp::Mul, Expr::param("alpha"), Expr::pop("S")),
            metadata: None,
            draw_method: ir::transition::DrawMethod::Poisson,
            rate_grad: HashMap::new(),
            lineage: None,
        }];
        let estimated: HashSet<String> = ["alpha".to_string()].into_iter().collect();
        assert!(coefficient_only_estimated(&m, &estimated).is_empty());
    }

    /// gh#119 gradient half: `alpha` enters only the forcing coefficient, but
    /// the compiler now emits ∂rate/∂alpha (the Sinusoidal derivative), so it
    /// has a usable NUTS gradient and must NOT be flagged — otherwise the guard
    /// would refuse the very fits the gradient half enables (the headline
    /// regression this fixes).
    #[test]
    fn does_not_flag_forcing_param_with_emitted_gradient() {
        let mut m = base_model();
        m.time_functions = vec![sinusoidal_forcing(Expr::param("alpha"))];
        // `alpha` is not in the rate body (the forcing is opaque there), but the
        // compiler emitted a derivative entry for it.
        let mut rate_grad = HashMap::new();
        rate_grad.insert("alpha".to_string(), Expr::pop("S"));
        m.transitions = vec![ir::transition::Transition {
            name: "infection".into(),
            stoichiometry: vec![ir::transition::StoichiometryEntry("S".into(), -1)],
            rate: Expr::pop("S"),
            metadata: None,
            draw_method: ir::transition::DrawMethod::Poisson,
            rate_grad,
            lineage: None,
        }];
        let estimated: HashSet<String> = ["alpha".to_string()].into_iter().collect();
        assert!(coefficient_only_estimated(&m, &estimated).is_empty(),
            "a forcing param with an emitted rate_grad has a usable gradient and \
             must not be blocked");
    }

    /// A non-estimated coefficient parameter is not flagged.
    #[test]
    fn does_not_flag_unestimated_param() {
        let mut m = base_model();
        m.time_functions = vec![sinusoidal_forcing(Expr::param("alpha"))];
        let estimated: HashSet<String> = HashSet::new();
        assert!(coefficient_only_estimated(&m, &estimated).is_empty());
    }

    /// An inline-table value parameter, used nowhere else, is flagged.
    #[test]
    fn flags_param_only_in_inline_table_value() {
        let mut m = base_model();
        m.tables = vec![ir::table::Table {
            name: "k_tbl".into(),
            source: ir::table::TableSource::Inline { values: vec![Expr::param("k")] },
            out_of_bounds: ir::table::OobPolicy::Error,
            cell_kind: None,
        }];
        let estimated: HashSet<String> = ["k".to_string()].into_iter().collect();
        assert_eq!(coefficient_only_estimated(&m, &estimated), vec!["k".to_string()]);
    }
}
