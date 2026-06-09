//! Fix B safety net: a shared binding body must be **state-only** — it may not
//! reference an estimated `Param`.
//!
//! Gradient correctness silently rests on this. `autodiff.ml` maps
//! `BindingRef → 0` and `pgas::collect_param_refs` returns `{}` for a
//! `BindingRef`, both justified *only* because `d(binding)/dp ≡ 0`. If a
//! binding body ever held a `Param`, NUTS would see a zero where a real
//! derivative belongs and fit the wrong answer with no error — the worst
//! class of bug for software that informs public-health decisions.
//!
//! The OCaml extraction guard (`expander.ml::body_refs_param_or_let`) prevents
//! the compiler from ever emitting such a binding. This test pins the
//! independent Rust-side guard in `CompiledModel::new`, which rejects a
//! hand-written or future IR that smuggles a `Param` into a binding body
//! rather than degrading inference in silence.

use std::path::PathBuf;

use sim::compiled_model::CompiledModel;

/// The binding-bearing golden (Fix-B trap #1 model: an 8-term mixed int/real
/// aggregate hoisted into `model.bindings`).
const BINDING_MODEL: &str = "sir_reservoir_mixed";

fn load_with_params(name: &str) -> ir::Model {
    let manifest = std::env::var("CARGO_MANIFEST_DIR").unwrap();
    let path = PathBuf::from(&manifest)
        .join("../../../ocaml/golden")
        .join(format!("{name}.ir.json"));
    let contents = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("read {name}: {e}"));
    let mut model: ir::Model =
        ir::from_str(&contents).unwrap_or_else(|e| panic!("parse {name}: {e}"));
    // Resolve parameter values (preset first, then a placeholder) so
    // `CompiledModel::new` reaches the binding check instead of bailing on a
    // missing value — the guard under test is about bindings, not params.
    let preset = model.presets.first().cloned();
    for p in &mut model.parameters {
        if p.value.resolved_value().is_none() {
            let v = preset
                .as_ref()
                .and_then(|pr| pr.params.get(&p.name).copied())
                .unwrap_or(1.0);
            p.value = p.value.with_value(v);
        }
    }
    model
}

/// Negative control: the unmodified state-only-binding model compiles. Proves
/// (a) the fixture really carries bindings and (b) the guard does not
/// false-positive on a legitimate state-only binding.
#[test]
fn state_only_binding_model_compiles() {
    let model = load_with_params(BINDING_MODEL);
    assert!(
        !model.bindings.is_empty(),
        "fixture {BINDING_MODEL} must carry bindings, else this whole test is vacuous"
    );
    assert!(
        CompiledModel::new(model).is_ok(),
        "a state-only-binding model must compile"
    );
}

/// A binding body that references an estimated `Param` must be rejected loudly.
#[test]
fn binding_referencing_a_param_is_rejected() {
    let mut model = load_with_params(BINDING_MODEL);
    assert!(!model.bindings.is_empty(), "fixture must carry bindings");
    let param_name = model
        .parameters
        .first()
        .expect("model has parameters")
        .name
        .clone();
    let binding_name = model.bindings[0].name.clone();

    // Smuggle a Param into a binding body — exactly what the OCaml guard forbids.
    model.bindings[0].expr = ir::expr::Expr::param(&param_name);

    let err = match CompiledModel::new(model) {
        Ok(_) => panic!(
            "a binding referencing a Param must be rejected (else its gradient is silently zeroed)"
        ),
        Err(e) => e,
    };
    let msg = err.to_string();
    // Non-vacuous: the error must actually be the binding guard (naming the
    // offending binding), not an unrelated failure (e.g. a missing param value).
    assert!(
        msg.contains(&binding_name),
        "error must name the offending binding '{binding_name}'; got: {msg}"
    );
    assert!(
        msg.to_lowercase().contains("param") || msg.to_lowercase().contains("state-only"),
        "error must explain the state-only/param violation; got: {msg}"
    );
}
