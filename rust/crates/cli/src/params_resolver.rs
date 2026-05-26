//! Unified parameter-value resolver — single source of truth for the
//! precedence chain documented in `docs/camdl-run-spec.md §1.3`.
//!
//! Background
//! ----------
//!
//! Before this module, three half-resolvers + several inline blocks
//! implemented the same precedence rules independently:
//!
//!   - `util::resolve_run_model` for `simulate` / `lineage` (no
//!     `[estimate]` semantics)
//!   - `fit::config_v2::FixedParams::resolve_with_model` for `survey`
//!     / `profile` `[fixed]` resolution (no CLI `--fixed`)
//!   - inline blocks in `profile.rs:437-453`, `if2.rs:109-168`,
//!     `pfilter.rs:47-55` (each subcommand-specific)
//!
//! Each was correct on its own slice; together they let small details
//! drift silently. See
//! `docs/dev/proposals/2026-05-25-cli-init-and-params-ux.md` for the
//! full audit and design rationale.
//!
//! Design
//! ------
//!
//! Two verbs, one resolver:
//!
//!   - `--fixed` carries explicit `NAME=VALUE` pairs (CLI side) and
//!     bulk files (`--fixed-file`).
//!   - On inference subcommands, names that appear in `--fixed` are
//!     also kicked out of the `[estimate]` set — `gamma=0.1` on a
//!     profile means "slice through gamma=0.1", which requires gamma
//!     to be fixed at 0.1 *and* not estimated.
//!
//! The resolver owns precedence (`resolve_parameters`), records
//! provenance (`ResolvedParameter.source`), and is the sole writer of
//! `model.parameters[i].value` outside the IR layer.
//!
//! Precedence (last wins)
//! ----------------------
//!
//!   1. Model parameter default (`p.value` from DSL)
//!   2. `fit.toml [fixed]` block (when present)
//!   3. `--fixed-file <toml>` (each file layered in order; later
//!      overrides earlier)
//!   4. Scenario preset (`preset.params` and multiplicative
//!      `preset.scale`)
//!   5. `--fixed NAME=VALUE` (highest)
//!
//! **Deliberate deviation from proposal:** the 2026-05-25 CLI UX
//! rev 2 proposal lists scenario at tier 2 (below `--fixed-file`).
//! That contradicts `docs/camdl-run-spec.md §1.3` which documents
//! and tests scenario > `params.toml` for forward simulation
//! (`scenario_runtime_application.rs` locks the behaviour in).
//! The proposal's own §"What this proposal does NOT touch" says
//! the spec order is "preserved exactly" — so the spec is the
//! load-bearing artifact and the resolver implements that order.
//! See `docs/dev/notes/2026-05-25-cli-ux-impl-questions.md`
//! §"Decision D".
//!
//! `[estimate]` membership rule:
//!   - Start: `estimate_set = inputs.fit_toml_estimate`
//!   - Remove every name that appears in (3) or (5) — i.e. user-
//!     explicit `--fixed{,-file}` assertions. Scenario does NOT
//!     kick from `[estimate]` because scenarios are σ-layer
//!     constructs (counterfactual modifications), not user
//!     assertions about a specific value.
//!   - Emit a warning (not an error) for each such removal
//!
//! On non-inference subcommands, `inputs.fit_toml_estimate` is empty;
//! the kick-out logic is a no-op.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;

use indexmap::{IndexMap, IndexSet};
use ir::table::TableSource;

// ─── Public types ─────────────────────────────────────────────────────────────

/// Inputs gathered from the CLI + model. Every subcommand assembles
/// one of these before dispatch; `resolve_parameters` returns the
/// per-parameter outcome plus provenance.
pub struct ParameterInputs<'a> {
    pub model:              &'a ir::Model,
    pub scenario:           Option<&'a str>,
    pub adhoc_enable:       &'a [String],
    pub adhoc_disable:      &'a [String],
    pub fixed_cli:          &'a [(String, f64)],
    pub fixed_files:        &'a [PathBuf],
    pub fit_toml_fixed:     &'a IndexMap<String, f64>,
    pub fit_toml_estimate:  &'a IndexSet<String>,
    pub table_files:        &'a HashMap<String, PathBuf>,
}

/// Where a parameter's final value came from. Serialised verbatim
/// into `run.json`'s `parameters_provenance` block via
/// [`ValueSource::tag`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ValueSource {
    /// `p.value` from the DSL — the model's authored default.
    ModelDefault,
    /// A named scenario's `preset.params` entry (or composed entries).
    Scenario(String),
    /// The `[fixed]` block of a `--fit` toml.
    FitTomlFixed,
    /// A `--fixed-file <toml>` invocation; carries the path so
    /// provenance distinguishes which file won under layering.
    FixedFile { path: PathBuf },
    /// A `--fixed NAME=VALUE` CLI flag.
    FixedCli,
}

impl ValueSource {
    /// Stable string tag for `run.json` serialisation.
    pub fn tag(&self) -> &'static str {
        match self {
            ValueSource::ModelDefault    => "model_default",
            ValueSource::Scenario(_)     => "scenario",
            ValueSource::FitTomlFixed    => "fit_toml_fixed",
            ValueSource::FixedFile { .. } => "fixed_file",
            ValueSource::FixedCli        => "fixed_cli",
        }
    }
}

/// Resolver-decided role for a parameter. ADT-shaped rather than
/// `bool fixed` so the *reason* a parameter ended up fixed is
/// first-class — the `run.json` provenance distinguishes "never in
/// [estimate]" from "was in [estimate], --fixed kicked it out",
/// which matters for auditing whether a profile slice did what the
/// user intended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ParameterRole {
    Fixed { reason: FixReason },
    Estimated,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FixReason {
    /// The parameter was never in `[estimate]` to begin with — either
    /// no `--fit` toml was passed, or the toml did not list it. On
    /// non-inference subcommands, every parameter falls here.
    NotInEstimate,
    /// The parameter was listed in `[estimate]`, but `--fixed` /
    /// `--fixed-file` pinned it to an explicit value, kicking it out.
    KickedFromEstimate { by: ValueSource },
}

#[derive(Debug, Clone)]
pub struct ResolvedParameter {
    pub name:   String,
    pub value:  f64,
    pub source: ValueSource,
    pub role:   ParameterRole,
}

#[derive(Debug, Clone)]
pub struct ResolvedParameters {
    pub params:       Vec<ResolvedParameter>,
    pub estimate_set: IndexSet<String>,
    pub model:        ir::Model,
    pub warnings:     Vec<ResolverWarning>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum ResolverWarning {
    KickedFromEstimate { name: String, by: ValueSource },
}

impl ResolverWarning {
    /// Human-readable rendering for stderr.
    pub fn format(&self) -> String {
        match self {
            ResolverWarning::KickedFromEstimate { name, by } => {
                let source_clause = match by {
                    ValueSource::FixedCli => format!("--fixed {}", name),
                    ValueSource::FixedFile { path } => {
                        format!("--fixed-file {}", path.display())
                    }
                    other => format!("source {:?}", other),
                };
                format!(
                    "warning: {} removes `{}` from [estimate]; it will be \
                     pinned to its resolved value rather than inferred.",
                    source_clause, name)
            }
        }
    }
}

#[derive(Debug, Clone)]
pub enum ResolveError {
    UnknownParameter     { name: String, source: ValueSource, candidates: Vec<String> },
    NonFiniteValue       { name: String, value: f64, source: ValueSource },
    UnsetRequired        { name: String },
    SchemaMismatch       { path: PathBuf, msg: String },
    ScenarioNotFound     { name: String, available: Vec<String> },
    ExternalTableMissing { table: String },
    BoundsViolation      { name: String, value: f64, lo: f64, hi: f64 },
    NestedCompose        { name: String },
}

impl std::fmt::Display for ResolveError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ResolveError::UnknownParameter { name, source, candidates } => {
                let src_label = match source {
                    ValueSource::FixedCli => "--fixed".to_string(),
                    ValueSource::FixedFile { path } =>
                        format!("--fixed-file {}", path.display()),
                    ValueSource::FitTomlFixed => "fit.toml [fixed]".to_string(),
                    other => format!("{:?}", other),
                };
                write!(f,
                    "unknown parameter `{name}` from {src_label}.\n  \
                     Available parameters: {}",
                    if candidates.is_empty() { "(none)".to_string() }
                    else { candidates.join(", ") })
            }
            ResolveError::NonFiniteValue { name, value, source } => {
                write!(f,
                    "parameter `{name}` resolved to non-finite value {value} \
                     (from {}).\n  \
                     Fix: supply a finite numeric value via --fixed, \
                     --fixed-file, or the scenario block.",
                    source.tag())
            }
            ResolveError::UnsetRequired { name } => {
                write!(f,
                    "parameter `{name}` has no value: no model default, no \
                     scenario, no --fit toml entry, no --fixed-file, no \
                     --fixed.\n  \
                     Fix: declare a default in the .camdl model, or pin via \
                     `--fixed {name}=<value>`.")
            }
            ResolveError::SchemaMismatch { path, msg } =>
                write!(f, "schema mismatch in {}: {}", path.display(), msg),
            ResolveError::ScenarioNotFound { name, available } => {
                write!(f, "scenario `{name}` not found in model.\n  Available: {}",
                    if available.is_empty() { "(none)".to_string() }
                    else { available.join(", ") })
            }
            ResolveError::ExternalTableMissing { table } => {
                write!(f,
                    "table `{table}` is declared as external() but --table \
                     {table}=<file> was not provided")
            }
            ResolveError::BoundsViolation { name, value, lo, hi } => {
                write!(f,
                    "parameter `{name}` = {value} is outside declared bounds \
                     [{lo}, {hi}].\n  \
                     Fix: either widen the bounds in the model, or supply a \
                     value within the declared range.")
            }
            ResolveError::NestedCompose { name } => {
                write!(f,
                    "nested compose is not supported. Scenario `{name}` \
                     referenced in compose = [...] itself uses compose.")
            }
        }
    }
}

impl std::error::Error for ResolveError {}

// ─── Entry point ──────────────────────────────────────────────────────────────

/// Resolve a `ParameterInputs` to a `ResolvedParameters`, walking the
/// 5-tier precedence chain and recording provenance.
///
/// Side effect: the returned `ResolvedParameters.model` carries the
/// mutated `parameters[*].value` fields (and any scenario-applied
/// `interventions` filter + filled-in external tables). This is the
/// shape downstream `CompiledModel::new(model)` expects.
pub fn resolve_parameters<'a>(
    inputs: ParameterInputs<'a>,
) -> Result<ResolvedParameters, ResolveError> {
    let mut model = inputs.model.clone();
    let mut warnings: Vec<ResolverWarning> = Vec::new();

    // ── Tier 1: model defaults (already in model.parameters) ────────────
    //
    // No mutation needed — the IR carries `p.value` straight from the
    // DSL. The resolver layers tiers 2..5 on top.
    //
    // Track each parameter's *current* source as we walk tiers. The
    // map starts with whatever the IR supplied (ModelDefault for
    // params with a value, sentinel "unset" for those without).
    let mut current_source: HashMap<String, Option<ValueSource>> =
        model.parameters.iter()
            .map(|p| (p.name.clone(),
                if p.value.is_some() { Some(ValueSource::ModelDefault) } else { None }))
            .collect();

    let model_param_set: HashSet<String> = model.parameters.iter()
        .map(|p| p.name.clone()).collect();
    let model_param_names: Vec<String> = model.parameters.iter()
        .map(|p| p.name.clone()).collect();

    // Pre-resolve the scenario preset (and recursively-composed
    // sub-scenarios) so we know which intervention enable/disable
    // names to apply *and* which params/scales to layer at tier 4.
    // The intervention filter applies regardless of tier ordering
    // because it modifies `model.interventions`, not parameter
    // values.
    let scenario_name = inputs.scenario.map(|s| s.to_string());
    let (scenario_enable, scenario_disable, scenario_params, scenario_scale):
        (Vec<String>, Vec<String>, Vec<(String, f64)>, Vec<(String, f64)>) =
        if let Some(name) = scenario_name.as_deref() {
            let preset = model.presets.iter().find(|p| p.name == name)
                .ok_or_else(|| ResolveError::ScenarioNotFound {
                    name: name.to_string(),
                    available: model.presets.iter().map(|p| p.name.clone()).collect(),
                })?
                .clone();
            let mut composed_enable: Vec<String> = Vec::new();
            let mut composed_disable: Vec<String> = Vec::new();
            let mut composed_params: Vec<(String, f64)> = Vec::new();
            let mut composed_scale: Vec<(String, f64)> = Vec::new();
            for sc_name in &preset.compose {
                let sub = model.presets.iter().find(|p| p.name == *sc_name)
                    .ok_or_else(|| ResolveError::ScenarioNotFound {
                        name: sc_name.clone(),
                        available: model.presets.iter().map(|p| p.name.clone()).collect(),
                    })?;
                if !sub.compose.is_empty() {
                    return Err(ResolveError::NestedCompose { name: sc_name.clone() });
                }
                composed_enable.extend(sub.enable.clone());
                composed_disable.extend(sub.disable.clone());
                composed_params.extend(sub.params.iter().map(|(k, &v)| (k.clone(), v)));
                composed_scale.extend(sub.scale.iter().map(|(k, &v)| (k.clone(), v)));
            }
            composed_enable.extend(preset.enable.clone());
            composed_disable.extend(preset.disable.clone());
            composed_params.extend(preset.params.iter().map(|(k, &v)| (k.clone(), v)));
            composed_scale.extend(preset.scale.iter().map(|(k, &v)| (k.clone(), v)));
            (composed_enable, composed_disable, composed_params, composed_scale)
        } else {
            (inputs.adhoc_enable.to_vec(), inputs.adhoc_disable.to_vec(),
             Vec::new(), Vec::new())
        };

    // Intervention filter (independent of value precedence).
    if scenario_name.is_some() {
        crate::util::apply_scenario_filter(
            &mut model, &scenario_enable, &scenario_disable)
            .map_err(|msg| ResolveError::SchemaMismatch {
                path: PathBuf::from("(scenario filter)"),
                msg,
            })?;
    } else if !inputs.adhoc_enable.is_empty() || !inputs.adhoc_disable.is_empty() {
        crate::util::apply_scenario_filter(
            &mut model, inputs.adhoc_enable, inputs.adhoc_disable)
            .map_err(|msg| ResolveError::SchemaMismatch {
                path: PathBuf::from("(scenario filter)"),
                msg,
            })?;
    }

    // ── Tier 2: fit.toml [fixed] block ──────────────────────────────────
    for (name, &v) in inputs.fit_toml_fixed {
        if !model_param_set.contains(name) {
            return Err(ResolveError::UnknownParameter {
                name: name.clone(),
                source: ValueSource::FitTomlFixed,
                candidates: model_param_names.clone(),
            });
        }
        for p in &mut model.parameters {
            if p.name == *name {
                p.value = Some(v);
                current_source.insert(name.clone(), Some(ValueSource::FitTomlFixed));
            }
        }
    }

    // ── Tier 3: --fixed-file <toml> (layered, last wins) ────────────────
    for path in inputs.fixed_files {
        let path_str = path.to_string_lossy().into_owned();
        let overrides = crate::util::load_params_toml(&path_str)
            .map_err(|msg| ResolveError::SchemaMismatch {
                path: path.clone(),
                msg,
            })?;
        for name in overrides.keys() {
            if !model_param_set.contains(name) {
                return Err(ResolveError::UnknownParameter {
                    name: name.clone(),
                    source: ValueSource::FixedFile { path: path.clone() },
                    candidates: model_param_names.clone(),
                });
            }
        }
        for p in &mut model.parameters {
            if let Some(&v) = overrides.get(&p.name) {
                p.value = Some(v);
                current_source.insert(p.name.clone(),
                    Some(ValueSource::FixedFile { path: path.clone() }));
            }
        }
    }

    // ── Tier 4: scenario params + scale ─────────────────────────────────
    //
    // Order is spec-§1.3-compliant: scenarios override `--fixed-file`
    // (the legacy `--params FILE`). The intervention filter for the
    // scenario was applied earlier; only `params` / `scale` happen
    // here, layered on top of the file overrides.
    if let Some(name) = scenario_name.as_deref() {
        for (k, v) in &scenario_params {
            for p in &mut model.parameters {
                if p.name == *k {
                    p.value = Some(*v);
                    current_source.insert(k.clone(),
                        Some(ValueSource::Scenario(name.to_string())));
                }
            }
        }
        for (k, factor) in &scenario_scale {
            for p in &mut model.parameters {
                if p.name == *k {
                    if let Some(v) = p.value {
                        p.value = Some(v * factor);
                        current_source.insert(k.clone(),
                            Some(ValueSource::Scenario(name.to_string())));
                    }
                }
            }
        }
    }

    // ── Tier 5: --fixed NAME=VALUE (highest) ────────────────────────────
    for (name, v) in inputs.fixed_cli {
        if !model_param_set.contains(name) {
            return Err(ResolveError::UnknownParameter {
                name: name.clone(),
                source: ValueSource::FixedCli,
                candidates: model_param_names.clone(),
            });
        }
        for p in &mut model.parameters {
            if p.name == *name {
                p.value = Some(*v);
                current_source.insert(name.clone(), Some(ValueSource::FixedCli));
            }
        }
    }

    // ── Estimate-set kick-out + provenance assembly ─────────────────────
    let mut estimate_set: IndexSet<String> = inputs.fit_toml_estimate.clone();

    // A name is "kicked from [estimate]" if it appears in tier 4 or
    // tier 5 (CLI / file `--fixed*`) — those are user-explicit "pin
    // this" assertions. Tier 3 (fit.toml [fixed]) is a no-op here
    // because the toml's `[fixed]` block already excludes those
    // names from `[estimate]` at config-load time.
    let kicker_names: HashMap<String, ValueSource> = {
        let mut m: HashMap<String, ValueSource> = HashMap::new();
        for path in inputs.fixed_files {
            let path_str = path.to_string_lossy().into_owned();
            // We already validated the file; load it again for the
            // name list. The cost is negligible vs simulation, and
            // it keeps tier 4 / tier 5 source attribution explicit.
            if let Ok(overrides) = crate::util::load_params_toml(&path_str) {
                for name in overrides.keys() {
                    m.insert(name.clone(),
                        ValueSource::FixedFile { path: path.clone() });
                }
            }
        }
        for (name, _) in inputs.fixed_cli {
            m.insert(name.clone(), ValueSource::FixedCli);
        }
        m
    };

    let mut kicked: HashMap<String, ValueSource> = HashMap::new();
    estimate_set.retain(|name| {
        if let Some(by) = kicker_names.get(name) {
            warnings.push(ResolverWarning::KickedFromEstimate {
                name: name.clone(),
                by: by.clone(),
            });
            kicked.insert(name.clone(), by.clone());
            false
        } else {
            true
        }
    });

    // Assemble ResolvedParameter entries in declaration order.
    let mut params: Vec<ResolvedParameter> = Vec::with_capacity(model.parameters.len());
    for p in &model.parameters {
        let Some(value) = p.value else {
            return Err(ResolveError::UnsetRequired { name: p.name.clone() });
        };
        if !value.is_finite() {
            let source = current_source.get(&p.name)
                .and_then(|s| s.clone())
                .unwrap_or(ValueSource::ModelDefault);
            return Err(ResolveError::NonFiniteValue {
                name: p.name.clone(),
                value,
                source,
            });
        }
        if let Some((lo, hi)) = p.bounds {
            if value < lo || value > hi {
                return Err(ResolveError::BoundsViolation {
                    name: p.name.clone(),
                    value, lo, hi,
                });
            }
        }
        let source = current_source.get(&p.name)
            .and_then(|s| s.clone())
            .unwrap_or(ValueSource::ModelDefault);
        let role = if estimate_set.contains(&p.name) {
            ParameterRole::Estimated
        } else if let Some(by) = kicked.get(&p.name) {
            ParameterRole::Fixed {
                reason: FixReason::KickedFromEstimate { by: by.clone() },
            }
        } else {
            ParameterRole::Fixed { reason: FixReason::NotInEstimate }
        };
        params.push(ResolvedParameter {
            name: p.name.clone(),
            value,
            source,
            role,
        });
    }

    // ── External tables ─────────────────────────────────────────────────
    for table in &mut model.tables {
        if let TableSource::External { external: ref name } = table.source {
            let logical_name = name.clone();
            match inputs.table_files.get(&logical_name) {
                None => return Err(ResolveError::ExternalTableMissing {
                    table: logical_name,
                }),
                Some(path) => {
                    let path_str = path.to_string_lossy().into_owned();
                    let values = crate::util::load_table_file(&path_str)
                        .map_err(|msg| ResolveError::SchemaMismatch {
                            path: path.clone(),
                            msg,
                        })?;
                    table.source = TableSource::Inline { values };
                }
            }
        }
    }

    Ok(ResolvedParameters {
        params,
        estimate_set,
        model,
        warnings,
    })
}

/// Render and print every warning in `resolved.warnings` to stderr.
/// Subcommand wrappers call this once after `resolve_parameters`.
pub fn print_warnings(resolved: &ResolvedParameters) {
    for w in &resolved.warnings {
        eprintln!("{}", w.format());
    }
}

// ─── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use ir::model::{InitialConditions, OutputConfig, OutputSchedule, Preset, SimulationConfig};
    use ir::parameter::Parameter;

    /// Minimal `ir::Model` for resolver tests. Parameters supplied via
    /// the argument; everything else is empty-but-valid.
    fn mk_model(parameters: Vec<Parameter>) -> ir::Model {
        ir::Model {
            name: "test".into(),
            version: "0.3".into(),
            time_unit: "days".into(),
            description: None,
            origin: None,
            origin_rata_die: None,
            compartments: vec![],
            transitions: vec![],
            ode_equations: vec![],
            time_functions: vec![],
            tables: vec![],
            interventions: vec![],
            observations: vec![],
            parameters,
            initial_conditions: InitialConditions::Explicit(HashMap::new()),
            output: OutputConfig {
                times: OutputSchedule::AtTimes(vec![]),
                format: "tsv".into(),
                trajectory: true,
                observations: false,
            },
            simulation: SimulationConfig {
                t_start: 0.0,
                t_end: 1.0,
                time_semantics: "continuous".into(),
                dt: None,
                rng_seed: None,
            },
            presets: vec![],
            model_structure: None,
            balance: None,
            identity_tracked_compartments: vec![],
        }
    }

    fn mk_param(name: &str, value: Option<f64>) -> Parameter {
        Parameter {
            name: name.into(),
            value,
            bounds: None,
            prior: None,
            transform: None,
            initial_value: None,
            param_kind: None,
            param_dim: None,
            hierarchical: None,
        }
    }

    fn mk_param_bounded(name: &str, value: Option<f64>, bounds: (f64, f64)) -> Parameter {
        Parameter {
            name: name.into(),
            value,
            bounds: Some(bounds),
            prior: None,
            transform: None,
            initial_value: None,
            param_kind: None,
            param_dim: None,
            hierarchical: None,
        }
    }

    fn empty_inputs<'a>(model: &'a ir::Model,
                        fixed_cli: &'a [(String, f64)],
                        fixed_files: &'a [PathBuf],
                        fit_toml_fixed: &'a IndexMap<String, f64>,
                        fit_toml_estimate: &'a IndexSet<String>) -> ParameterInputs<'a> {
        // Static empties (lifetimes work out because we pass refs to
        // owned containers held by the caller).
        ParameterInputs {
            model,
            scenario: None,
            adhoc_enable: &[],
            adhoc_disable: &[],
            fixed_cli,
            fixed_files,
            fit_toml_fixed,
            fit_toml_estimate,
            table_files: &EMPTY_TABLES,
        }
    }

    use std::sync::LazyLock;
    static EMPTY_TABLES: LazyLock<HashMap<String, PathBuf>> =
        LazyLock::new(HashMap::new);

    // ── Tier 1: model defaults ──────────────────────────────────────────

    #[test]
    fn tier1_model_default_flows_through() {
        let model = mk_model(vec![mk_param("beta", Some(0.5))]);
        let fcli = vec![];
        let ffiles = vec![];
        let ftf = IndexMap::new();
        let fte = IndexSet::new();
        let resolved = resolve_parameters(empty_inputs(&model, &fcli, &ffiles, &ftf, &fte))
            .expect("resolution should succeed");
        assert_eq!(resolved.params.len(), 1);
        assert_eq!(resolved.params[0].name, "beta");
        assert_eq!(resolved.params[0].value, 0.5);
        assert_eq!(resolved.params[0].source, ValueSource::ModelDefault);
        assert!(matches!(resolved.params[0].role,
            ParameterRole::Fixed { reason: FixReason::NotInEstimate }));
    }

    #[test]
    fn tier1_unset_required_errors() {
        // No model default, no override → UnsetRequired.
        let model = mk_model(vec![mk_param("beta", None)]);
        let fcli = vec![];
        let ffiles = vec![];
        let ftf = IndexMap::new();
        let fte = IndexSet::new();
        let err = resolve_parameters(empty_inputs(&model, &fcli, &ffiles, &ftf, &fte))
            .unwrap_err();
        assert!(matches!(err, ResolveError::UnsetRequired { ref name } if name == "beta"));
    }

    // ── Tier 2: scenario ────────────────────────────────────────────────

    #[test]
    fn tier2_scenario_overrides_model_default() {
        let mut model = mk_model(vec![mk_param("beta", Some(0.5))]);
        let mut scen_params = HashMap::new();
        scen_params.insert("beta".to_string(), 0.9);
        model.presets.push(Preset {
            name: "baseline".into(),
            label: "baseline".into(),
            params: scen_params,
            scale: HashMap::new(),
            enable: vec![],
            disable: vec![],
            compose: vec![],
            t_end: None,
        });
        let fcli = vec![];
        let ffiles = vec![];
        let ftf = IndexMap::new();
        let fte = IndexSet::new();
        let mut inputs = empty_inputs(&model, &fcli, &ffiles, &ftf, &fte);
        inputs.scenario = Some("baseline");
        let resolved = resolve_parameters(inputs).expect("ok");
        assert_eq!(resolved.params[0].value, 0.9);
        assert!(matches!(&resolved.params[0].source,
            ValueSource::Scenario(name) if name == "baseline"));
    }

    #[test]
    fn tier2_scenario_not_found_errors() {
        let model = mk_model(vec![mk_param("beta", Some(0.5))]);
        let fcli = vec![];
        let ffiles = vec![];
        let ftf = IndexMap::new();
        let fte = IndexSet::new();
        let mut inputs = empty_inputs(&model, &fcli, &ffiles, &ftf, &fte);
        inputs.scenario = Some("nonesuch");
        let err = resolve_parameters(inputs).unwrap_err();
        assert!(matches!(err, ResolveError::ScenarioNotFound { ref name, .. } if name == "nonesuch"));
    }

    // ── Tier 3: fit.toml [fixed] ────────────────────────────────────────

    #[test]
    fn tier3_fit_toml_fixed_overrides_model_default() {
        let model = mk_model(vec![mk_param("beta", Some(0.5))]);
        let fcli = vec![];
        let ffiles = vec![];
        let mut ftf = IndexMap::new();
        ftf.insert("beta".into(), 0.7);
        let fte = IndexSet::new();
        let resolved = resolve_parameters(empty_inputs(&model, &fcli, &ffiles, &ftf, &fte))
            .expect("ok");
        assert_eq!(resolved.params[0].value, 0.7);
        assert_eq!(resolved.params[0].source, ValueSource::FitTomlFixed);
    }

    #[test]
    fn tier3_unknown_param_in_fit_toml_errors() {
        let model = mk_model(vec![mk_param("beta", Some(0.5))]);
        let fcli = vec![];
        let ffiles = vec![];
        let mut ftf = IndexMap::new();
        ftf.insert("typo".into(), 0.7);
        let fte = IndexSet::new();
        let err = resolve_parameters(empty_inputs(&model, &fcli, &ffiles, &ftf, &fte))
            .unwrap_err();
        assert!(matches!(err, ResolveError::UnknownParameter { ref name, .. } if name == "typo"));
    }

    // ── Tier 5: --fixed CLI (highest) ───────────────────────────────────

    #[test]
    fn tier5_fixed_cli_overrides_everything() {
        let mut model = mk_model(vec![mk_param("beta", Some(0.5))]);
        let mut scen_params = HashMap::new();
        scen_params.insert("beta".to_string(), 0.9);
        model.presets.push(Preset {
            name: "baseline".into(),
            label: "baseline".into(),
            params: scen_params,
            scale: HashMap::new(),
            enable: vec![],
            disable: vec![],
            compose: vec![],
            t_end: None,
        });
        let fcli = vec![("beta".to_string(), 1.1)];
        let ffiles = vec![];
        let mut ftf = IndexMap::new();
        ftf.insert("beta".into(), 0.7);
        let fte = IndexSet::new();
        let mut inputs = empty_inputs(&model, &fcli, &ffiles, &ftf, &fte);
        inputs.scenario = Some("baseline");
        let resolved = resolve_parameters(inputs).expect("ok");
        assert_eq!(resolved.params[0].value, 1.1);
        assert_eq!(resolved.params[0].source, ValueSource::FixedCli);
    }

    #[test]
    fn tier5_unknown_cli_param_errors() {
        let model = mk_model(vec![mk_param("beta", Some(0.5))]);
        let fcli = vec![("typo".to_string(), 0.7)];
        let ffiles = vec![];
        let ftf = IndexMap::new();
        let fte = IndexSet::new();
        let err = resolve_parameters(empty_inputs(&model, &fcli, &ffiles, &ftf, &fte))
            .unwrap_err();
        assert!(matches!(err, ResolveError::UnknownParameter { ref name, ref source, .. }
            if name == "typo" && *source == ValueSource::FixedCli));
    }

    // ── [estimate] kick-out ─────────────────────────────────────────────

    #[test]
    fn cli_fixed_kicks_out_of_estimate_with_warning() {
        let model = mk_model(vec![
            mk_param("beta", Some(0.5)),
            mk_param("gamma", Some(0.1)),
        ]);
        let fcli = vec![("gamma".to_string(), 0.2)];
        let ffiles = vec![];
        let ftf = IndexMap::new();
        let mut fte: IndexSet<String> = IndexSet::new();
        fte.insert("beta".into());
        fte.insert("gamma".into());
        let resolved = resolve_parameters(empty_inputs(&model, &fcli, &ffiles, &ftf, &fte))
            .expect("ok");

        // beta stayed estimated; gamma got kicked.
        assert!(resolved.estimate_set.contains("beta"));
        assert!(!resolved.estimate_set.contains("gamma"));
        let gamma = resolved.params.iter().find(|p| p.name == "gamma").unwrap();
        assert!(matches!(&gamma.role,
            ParameterRole::Fixed { reason: FixReason::KickedFromEstimate { by } }
            if *by == ValueSource::FixedCli));
        assert_eq!(resolved.warnings.len(), 1);
        match &resolved.warnings[0] {
            ResolverWarning::KickedFromEstimate { name, by } => {
                assert_eq!(name, "gamma");
                assert_eq!(*by, ValueSource::FixedCli);
            }
        }
    }

    #[test]
    fn fit_toml_fixed_does_not_warn_or_kick() {
        // fit.toml [fixed] is mutually-exclusive with [estimate] at
        // config-load time, so the resolver doesn't emit a kick-out
        // warning for tier-3 sources.
        let model = mk_model(vec![mk_param("beta", Some(0.5))]);
        let fcli = vec![];
        let ffiles = vec![];
        let mut ftf = IndexMap::new();
        ftf.insert("beta".into(), 0.7);
        let mut fte: IndexSet<String> = IndexSet::new();
        // Note: in real use, beta would not be in BOTH; we set it to
        // verify the resolver's behaviour if the caller hands a
        // pathological pair. Tier-3 does NOT warn.
        fte.insert("beta".into());
        let resolved = resolve_parameters(empty_inputs(&model, &fcli, &ffiles, &ftf, &fte))
            .expect("ok");
        // beta remains in estimate_set; the warning fires only for
        // tier 4 / tier 5 kickers.
        assert!(resolved.estimate_set.contains("beta"));
        assert!(resolved.warnings.is_empty());
    }

    // ── Bounds + finite checks ──────────────────────────────────────────

    #[test]
    fn bounds_violation_errors() {
        let model = mk_model(vec![mk_param_bounded("beta", Some(0.5), (0.0, 1.0))]);
        let fcli = vec![("beta".to_string(), 2.0)];
        let ffiles = vec![];
        let ftf = IndexMap::new();
        let fte = IndexSet::new();
        let err = resolve_parameters(empty_inputs(&model, &fcli, &ffiles, &ftf, &fte))
            .unwrap_err();
        assert!(matches!(err, ResolveError::BoundsViolation {
            ref name, value, ..
        } if name == "beta" && (value - 2.0).abs() < 1e-12));
    }

    #[test]
    fn non_finite_value_errors() {
        let model = mk_model(vec![mk_param("beta", Some(0.5))]);
        let fcli = vec![("beta".to_string(), f64::NAN)];
        let ffiles = vec![];
        let ftf = IndexMap::new();
        let fte = IndexSet::new();
        let err = resolve_parameters(empty_inputs(&model, &fcli, &ffiles, &ftf, &fte))
            .unwrap_err();
        assert!(matches!(err, ResolveError::NonFiniteValue { ref name, .. } if name == "beta"));
    }

    // ── Provenance round-trip ───────────────────────────────────────────

    // ── Spec-§1.3 precedence: scenario > --fixed-file > --fixed CLI ─────

    #[test]
    fn scenario_beats_fit_toml_fixed_per_spec_section_1_3() {
        // Spec §1.3 says: params.toml < scenario. The resolver
        // implements this — fit-toml [fixed] (tier 2) is overwritten
        // by scenario params (tier 4). Locked in by the integration
        // test `scenario_runtime_application::scenario_set_replaces_mu_value`.
        let mut model = mk_model(vec![mk_param("beta", Some(0.5))]);
        let mut scen_params = HashMap::new();
        scen_params.insert("beta".to_string(), 0.9);
        model.presets.push(Preset {
            name: "preset".into(),
            label: "preset".into(),
            params: scen_params,
            scale: HashMap::new(),
            enable: vec![],
            disable: vec![],
            compose: vec![],
            t_end: None,
        });
        let fcli = vec![];
        let ffiles = vec![];
        let mut ftf = IndexMap::new();
        ftf.insert("beta".into(), 0.7);
        let fte = IndexSet::new();
        let mut inputs = empty_inputs(&model, &fcli, &ffiles, &ftf, &fte);
        inputs.scenario = Some("preset");
        let resolved = resolve_parameters(inputs).expect("ok");
        // Scenario value wins, not the fit-toml fixed value.
        assert_eq!(resolved.params[0].value, 0.9);
        assert!(matches!(&resolved.params[0].source,
            ValueSource::Scenario(name) if name == "preset"));
    }

    #[test]
    fn fixed_cli_beats_scenario_per_spec_section_1_3() {
        // Spec §1.3 says: scenario < --param CLI. `--fixed CLI`
        // (tier 5) must override scenario (tier 4).
        let mut model = mk_model(vec![mk_param("beta", Some(0.5))]);
        let mut scen_params = HashMap::new();
        scen_params.insert("beta".to_string(), 0.9);
        model.presets.push(Preset {
            name: "preset".into(),
            label: "preset".into(),
            params: scen_params,
            scale: HashMap::new(),
            enable: vec![],
            disable: vec![],
            compose: vec![],
            t_end: None,
        });
        let fcli = vec![("beta".to_string(), 1.5)];
        let ffiles = vec![];
        let ftf = IndexMap::new();
        let fte = IndexSet::new();
        let mut inputs = empty_inputs(&model, &fcli, &ffiles, &ftf, &fte);
        inputs.scenario = Some("preset");
        let resolved = resolve_parameters(inputs).expect("ok");
        // --fixed CLI wins over scenario.
        assert_eq!(resolved.params[0].value, 1.5);
        assert_eq!(resolved.params[0].source, ValueSource::FixedCli);
    }

    #[test]
    fn scenario_scale_multiplies_resolved_value_not_just_model_default() {
        // Scenario `scale` applies multiplicatively to whatever
        // value is currently in the slot. The order ensures that
        // tier 2 + tier 3 layered values feed into the multiplication.
        let mut model = mk_model(vec![mk_param("beta", Some(0.5))]);
        let mut scen_scale = HashMap::new();
        scen_scale.insert("beta".to_string(), 2.0);
        model.presets.push(Preset {
            name: "doubled".into(),
            label: "doubled".into(),
            params: HashMap::new(),
            scale: scen_scale,
            enable: vec![],
            disable: vec![],
            compose: vec![],
            t_end: None,
        });
        let fcli = vec![];
        let ffiles = vec![];
        let mut ftf = IndexMap::new();
        ftf.insert("beta".into(), 0.7);  // tier 2 sets beta=0.7
        let fte = IndexSet::new();
        let mut inputs = empty_inputs(&model, &fcli, &ffiles, &ftf, &fte);
        inputs.scenario = Some("doubled");
        let resolved = resolve_parameters(inputs).expect("ok");
        // 0.7 (fit_toml_fixed) × 2.0 (scale) = 1.4
        assert!((resolved.params[0].value - 1.4).abs() < 1e-12,
            "scenario scale must multiply tier-2/3 value; got {}",
            resolved.params[0].value);
    }

    #[test]
    fn resolved_model_carries_mutated_values() {
        // The `model` field in `ResolvedParameters` must carry the
        // post-resolution `parameters[i].value`. Downstream
        // `CompiledModel::new(model)` reads these.
        let model = mk_model(vec![mk_param("beta", Some(0.5))]);
        let fcli = vec![("beta".to_string(), 0.9)];
        let ffiles = vec![];
        let ftf = IndexMap::new();
        let fte = IndexSet::new();
        let resolved = resolve_parameters(empty_inputs(&model, &fcli, &ffiles, &ftf, &fte))
            .expect("ok");
        let beta_in_model = resolved.model.parameters.iter()
            .find(|p| p.name == "beta").unwrap();
        assert_eq!(beta_in_model.value, Some(0.9));
    }

    #[test]
    fn warning_format_is_actionable() {
        // The warning format must name the parameter and the source so
        // a user re-reading stderr can localise the kick-out.
        let model = mk_model(vec![mk_param("gamma", Some(0.1))]);
        let fcli = vec![("gamma".to_string(), 0.2)];
        let ffiles = vec![];
        let ftf = IndexMap::new();
        let mut fte: IndexSet<String> = IndexSet::new();
        fte.insert("gamma".into());
        let resolved = resolve_parameters(empty_inputs(&model, &fcli, &ffiles, &ftf, &fte))
            .expect("ok");
        assert_eq!(resolved.warnings.len(), 1);
        let msg = resolved.warnings[0].format();
        assert!(msg.contains("gamma"), "warning must name `gamma`: {}", msg);
        assert!(msg.contains("--fixed"), "warning must mention --fixed: {}", msg);
        assert!(msg.contains("[estimate]"), "warning must mention [estimate]: {}", msg);

        // `print_warnings` is a thin wrapper; smoke-call it to confirm
        // no panic and to keep the symbol live.
        print_warnings(&resolved);
    }

    #[test]
    fn value_source_tag_is_stable() {
        // Tags are serialised verbatim into run.json; pin them.
        assert_eq!(ValueSource::ModelDefault.tag(), "model_default");
        assert_eq!(ValueSource::Scenario("x".into()).tag(), "scenario");
        assert_eq!(ValueSource::FitTomlFixed.tag(), "fit_toml_fixed");
        assert_eq!(ValueSource::FixedFile { path: PathBuf::from("p") }.tag(), "fixed_file");
        assert_eq!(ValueSource::FixedCli.tag(), "fixed_cli");
    }

    #[test]
    fn provenance_distinguishes_sources() {
        let mut model = mk_model(vec![
            mk_param("a", Some(1.0)),  // ModelDefault
            mk_param("b", Some(1.0)),  // Scenario
            mk_param("c", Some(1.0)),  // FitTomlFixed
            mk_param("d", Some(1.0)),  // FixedCli
        ]);
        let mut scen_params = HashMap::new();
        scen_params.insert("b".to_string(), 2.0);
        model.presets.push(Preset {
            name: "preset".into(),
            label: "preset".into(),
            params: scen_params,
            scale: HashMap::new(),
            enable: vec![],
            disable: vec![],
            compose: vec![],
            t_end: None,
        });
        let fcli = vec![("d".to_string(), 4.0)];
        let ffiles = vec![];
        let mut ftf = IndexMap::new();
        ftf.insert("c".into(), 3.0);
        let fte = IndexSet::new();
        let mut inputs = empty_inputs(&model, &fcli, &ffiles, &ftf, &fte);
        inputs.scenario = Some("preset");
        let resolved = resolve_parameters(inputs).expect("ok");
        let by_name: HashMap<&str, &ResolvedParameter> =
            resolved.params.iter().map(|p| (p.name.as_str(), p)).collect();
        assert_eq!(by_name["a"].source, ValueSource::ModelDefault);
        assert!(matches!(&by_name["b"].source, ValueSource::Scenario(_)));
        assert_eq!(by_name["c"].source, ValueSource::FitTomlFixed);
        assert_eq!(by_name["d"].source, ValueSource::FixedCli);
    }
}
