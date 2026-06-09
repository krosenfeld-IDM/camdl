use serde::{Deserialize, Serialize};
use crate::expr::Expr;

// ── Schedule ──────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RecurringSchedule {
    pub start:  f64,
    pub period: f64,
    pub end:    f64,
    /// Day within each period when the event fires. Fire times are
    /// `at_day + k * period` for the smallest k where target >= start.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub at_day: Option<f64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InterventionSchedule {
    AtTimes(Vec<f64>),
    /// gh#69: parametric `at [...]` lists. Each `Expr` is evaluated
    /// once per simulation start against the current `params` vector
    /// to yield a concrete fire time. The OCaml expander emits this
    /// variant only when at least one entry references a parameter
    /// (or other non-constant expression); fully-constant lists stay
    /// in `AtTimes` so existing golden IRs remain byte-identical.
    AtTimesExpr(Vec<Expr>),
    Recurring(RecurringSchedule),
}

// ── Actions ───────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FractionTransfer {
    pub src:      String,
    pub dst:      String,
    pub fraction: Expr,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AbsoluteTransfer {
    pub src:   String,
    pub dst:   String,
    pub count: Expr,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SetAction {
    pub compartment: String,
    pub value:       Expr,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AddAction {
    pub compartment: String,
    pub count:       Expr,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Action {
    FractionTransfer(FractionTransfer),
    AbsoluteTransfer(AbsoluteTransfer),
    Set(SetAction),
    Add(AddAction),
}

// ── Intervention ──────────────────────────────────────────────────────────────

/// Distinguishes the two DSL constructs that both lower to [`Intervention`]
/// (gh#107). Replaces the former `always_active: bool` — a named enum names
/// the distinction and extends to a future kind (e.g. reactive, gh#204)
/// instead of bolting on a second bool.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InterventionKind {
    /// `interventions {}` — toggled by enable/disable/set/scale scenarios.
    #[default]
    Scenario,
    /// `events {}` — fires unconditionally every substep.
    Event,
}

impl InterventionKind {
    /// True for `Scenario` — the serialisation default, skipped on the wire
    /// (mirrors the former `always_active` skip-false discipline, so a
    /// scenario intervention carries no `kind` key).
    pub fn is_scenario(&self) -> bool {
        matches!(self, Self::Scenario)
    }
    /// True for `Event` — fires unconditionally, not scenario-toggled.
    /// Reads at call sites exactly where `always_active` did.
    pub fn is_event(&self) -> bool {
        matches!(self, Self::Event)
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Intervention {
    pub name:     String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base_name: Option<String>,
    pub schedule: InterventionSchedule,
    pub actions:  Vec<Action>,
    /// Which DSL construct declared this — `Event` (fires unconditionally,
    /// from `events {}`) or `Scenario` (scenario-toggled, from
    /// `interventions {}`). Absent on the wire ⇒ `Scenario` (the default).
    #[serde(default, skip_serializing_if = "InterventionKind::is_scenario")]
    pub kind: InterventionKind,
}
