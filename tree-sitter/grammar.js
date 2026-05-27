/// <reference types="tree-sitter-cli/dsl" />
// @ts-check

// tree-sitter grammar for the camdl DSL.
//
// Refresh history:
//   2026-05-26 — bring grammar up to current DSL surface (see README).
//   2026-03-16 — initial commit.

module.exports = grammar({
  name: "camdl",

  extras: ($) => [/\s/, $.comment],

  word: ($) => $.identifier,

  conflicts: ($) => [],

  rules: {
    // ── Top level ────────────────────────────────────────────────────────

    source_file: ($) => repeat($.declaration),

    declaration: ($) =>
      choice(
        $.time_unit_decl,
        $.description_decl,
        $.origin_decl,
        $.dimensions_block,
        $.compartments_block,
        $.parameters_block,
        $.tables_block,
        $.functions_block,
        $.forcing_block,
        $.transitions_block,
        $.observations_block,
        $.interventions_block,
        $.events_block,
        $.ode_block,
        $.output_block,
        $.simulate_block,
        $.init_block,
        $.timepoints_block,
        $.stratify_decl,
        $.let_decl,
        $.scenarios_block,
        $.balance_block,
      ),

    // ── Top-level declarations ───────────────────────────────────────────

    time_unit_decl: ($) =>
      seq("time_unit", "=", $.unit_literal),

    description_decl: ($) =>
      seq("description", "=", $.string),

    // origin = date("YYYY-MM-DD") — anchored-mode declaration.
    origin_decl: ($) =>
      seq(
        "origin", "=", "date", "(",
        field("iso_date", $.string),
        ")",
      ),

    // ── Dimensions ───────────────────────────────────────────────────────

    dimensions_block: ($) =>
      seq("dimensions", "{", repeat($.dim_entry), "}"),

    dim_entry: ($) =>
      seq(
        field("name", $.identifier),
        "=",
        field("source", choice($.dim_inline, $.dim_read)),
      ),

    dim_inline: ($) =>
      seq("[", commaSep(field("level", $.identifier)), "]"),

    // e.g. read_levels("path.csv", col = "patch")
    dim_read: ($) =>
      seq(
        field("fn", $.identifier), "(",
        field("path", $.string), ",",
        field("col_kw", $.identifier), "=",
        field("col", $.string),
        ")",
      ),

    // ── Compartments ─────────────────────────────────────────────────────

    compartments_block: ($) =>
      seq("compartments", "{", commaSep($.compartment_decl), "}"),

    compartment_decl: ($) =>
      seq(
        field("name", $.identifier),
        optional(seq(":", field("kind", choice("real", "integer")))),
      ),

    // ── Parameters ───────────────────────────────────────────────────────

    parameters_block: ($) =>
      seq("parameters", "{", repeat($.parameter_decl), "}"),

    parameter_decl: ($) =>
      seq(
        field("name", $.identifier),
        optional($.param_index),
        ":",
        field("kind", $.param_kind),
        optional(field("dim_annotation_unit", $.unit_literal)),
        optional(field("dim_annotation_bracket", $.dim_literal)),
        optional($.param_bounds),
        optional($.param_prior),
      ),

    // Tier-3 dimension annotation in square brackets, e.g. `[1]`, `[P]`,
    // `[T^-1]`, `[P*T^-1]`, `[P/T]`. The OCaml parser validates the exact
    // set; tree-sitter accepts any short token sequence so highlighting
    // works while errors land at compile time.
    dim_literal: ($) =>
      seq(
        "[",
        repeat1(choice($.identifier, $.number, "*", "/", "^", "-")),
        "]",
      ),

    param_index: ($) => seq("[", field("dim", $.identifier), "]"),

    param_bounds: ($) =>
      seq(
        "in", "[",
        field("lo", $.expr), ",",
        field("hi", $.expr),
        "]",
      ),

    // ~ Distribution(args) [| pool_over_dim]
    param_prior: ($) =>
      seq(
        "~",
        field("dist", $.identifier), "(",
        commaSep($.kw_arg),
        ")",
        optional(seq("|", field("pool_over", $.identifier))),
      ),

    param_kind: (_) =>
      choice(
        "rate", "probability", "positive", "count", "real",
        "instant", "duration",
      ),

    // ── Tables ───────────────────────────────────────────────────────────

    tables_block: ($) =>
      seq("tables", "{", repeat($.table_decl), "}"),

    table_decl: ($) =>
      choice(
        // Multi-name shared declaration: `pop, init_sus : patch = read(...)`.
        seq(
          commaSep1(field("name", $.identifier)),
          ":",
          field("dims", $.table_dims),
          "=",
          field("value", $.expr),
        ),
        seq(field("name", $.identifier), "=", field("value", $.expr)),
      ),

    table_dims: ($) => sep1($.table_dim_entry, "×"),

    // dim, optionally with cell-kind annotation (`age :rate`, gh#32) and/or
    // a tier-3 unit literal.
    table_dim_entry: ($) =>
      seq(
        field("dim", $.identifier),
        optional(seq(":", field("cell_kind", $.param_kind))),
        optional(field("unit", $.unit_literal)),
      ),

    // ── Functions and forcing (time-varying) ─────────────────────────────

    functions_block: ($) =>
      seq("functions", "{", repeat($.function_decl), "}"),

    forcing_block: ($) =>
      seq("forcing", "{", repeat($.function_decl), "}"),

    function_decl: ($) =>
      seq(
        field("name", $.identifier),
        optional(field("indices", $.index_bindings)),
        ":",
        field("kind", $.identifier),
        optional(field("unit", $.unit_literal)),
        "{",
        repeat($.func_arg),
        "}",
      ),

    func_arg: ($) =>
      seq(field("key", $.identifier), "=", field("value", $.expr)),

    // ── Transitions ──────────────────────────────────────────────────────

    transitions_block: ($) =>
      seq("transitions", "{", repeat($.transition_decl), "}"),

    transition_decl: ($) =>
      seq(
        optional($.attribute),
        field("name", $.identifier),
        optional(field("indices", $.index_bindings)),
        ":",
        field("src", optional($.stoich_ref_list)),
        "-->",
        choice(
          // standard form: dsts @ rate [where guard] [tag] OR dsts { rate = ..., ... }
          seq(
            field("dst", optional($.stoich_ref_list)),
            choice(
              seq(
                "@",
                field("rate", $.expr),
                optional($.where_clause),
                optional($.tag_clause),
              ),
              seq("{", repeat($.transition_body_entry), "}"),
            ),
          ),
          // branching form: { D1 : w1, ... } @ rate [where guard]
          seq(
            "{",
            field("branches", commaSep1($.branch_entry)),
            "}",
            "@",
            field("rate", $.expr),
            optional($.where_clause),
          ),
        ),
      ),

    branch_entry: ($) =>
      seq(field("name", $.identifier), ":", field("weight", $.expr)),

    // #[lineage] or any future #[…] attribute. Lexer rule: the `#[`
    // opener is one token with no intervening space.
    attribute: ($) =>
      seq($.hash_lbracket, field("name", $.identifier), "]"),

    hash_lbracket: (_) => token("#["),

    // A + B + C — multi-source stoichiometry on the LHS, multi-destination
    // (sum) on the RHS of -->.
    stoich_ref_list: ($) => sep1($.stoich_ref, "+"),

    stoich_ref: ($) =>
      seq(field("name", $.identifier), optional($.index_items)),

    where_clause: ($) => seq("where", $.guard_expr),

    tag_clause: ($) => seq("tag", "=", $.string),

    transition_body_entry: ($) =>
      choice(
        seq("rate", "=", $.expr),
        seq("where", $.guard_expr),
        seq("tag", "=", $.string),
      ),

    // ── Guard expressions ────────────────────────────────────────────────

    guard_expr: ($) =>
      choice(
        $.guard_atom,
        prec.left(1, seq($.guard_expr, "and", $.guard_expr)),
        prec.left(1, seq($.guard_expr, "or",  $.guard_expr)),
      ),

    guard_atom: ($) =>
      choice(
        seq(field("left", $.identifier), "==", field("right", $.identifier)),
        seq(field("left", $.identifier), "!=", field("right", $.identifier)),
        seq("(", $.guard_expr, ")"),
      ),

    // ── Index bindings  [a in age, (a, a_next) in consecutive(age)] ──────

    index_bindings: ($) =>
      seq("[", commaSep($.index_binding), "]"),

    index_binding: ($) =>
      choice(
        seq(field("var", $.identifier), "in", field("dim", $.identifier)),
        seq(field("var", $.identifier), "in", "compartments"),
        seq(
          "(",
          field("var", $.identifier),
          ",",
          field("next", $.identifier),
          ")",
          "in",
          "consecutive",
          "(",
          field("dim", $.identifier),
          ")",
        ),
      ),

    // ── Index items  [a, b]  or  [row = a, col = b] ──────────────────────

    index_items: ($) => seq("[", commaSep($.index_item), "]"),

    index_item: ($) =>
      choice(
        seq(field("key", $.identifier), "=", field("value", $.expr)),
        field("expr", $.expr),
      ),

    // ── Observations ─────────────────────────────────────────────────────

    observations_block: ($) =>
      seq("observations", "{", repeat($.obs_decl), "}"),

    obs_decl: ($) =>
      seq(
        field("name", $.identifier),
        optional(field("indices", $.index_bindings)),
        ":",
        "{",
        repeat($.obs_kv),
        "}",
      ),

    obs_kv: ($) =>
      choice(
        seq("every", "=", $.expr),
        seq("at", "=", "[", commaSep($.expr), "]"),
        seq("likelihood", "=", $.expr),
        // String literals are now part of `expr`, so a single
        // `IDENT = expr` form covers both data-stream paths
        // (`data = "file.tsv"`) and projection expressions
        // (`projected = incidence(infection)`).
        seq($.identifier, "=", $.expr),
        // nested-kind form: `data : tsv { path = "..." }` etc.
        seq($.identifier, ":", $.identifier, "{", repeat($.func_arg), "}"),
      ),

    // ── Interventions / events (same shape) ──────────────────────────────

    interventions_block: ($) =>
      seq("interventions", "{", repeat($.intervention_decl), "}"),

    events_block: ($) =>
      seq("events", "{", repeat($.intervention_decl), "}"),

    intervention_decl: ($) =>
      choice(
        // block form: name : { at = [...] | every = ... ; action = ... }
        seq(
          field("name", $.identifier),
          optional(field("indices", $.index_bindings)),
          ":",
          "{",
          repeat($.iv_kv),
          "}",
          optional($.where_clause),
        ),
        // transfer(...) at [...] — one-shot
        seq(
          field("name", $.identifier),
          optional(field("indices", $.index_bindings)),
          ":",
          "transfer", "(", commaSep($.transfer_kwarg), ")",
          "at", "[", commaSep($.expr), "]",
          optional($.where_clause),
        ),
        // transfer(...) { every = ..., from = ..., until = ... } — recurring
        seq(
          field("name", $.identifier),
          optional(field("indices", $.index_bindings)),
          ":",
          "transfer", "(", commaSep($.transfer_kwarg), ")",
          "{", repeat($.recurring_kv), "}",
          optional($.where_clause),
        ),
        // add(COMP, EXPR) at [...] — one-shot
        seq(
          field("name", $.identifier),
          optional(field("indices", $.index_bindings)),
          ":",
          "add", "(", field("comp", $.identifier), ",", field("count", $.expr), ")",
          "at", "[", commaSep($.expr), "]",
          optional($.where_clause),
        ),
        // add(COMP, EXPR) { every = ..., from = ..., until = ... } — recurring
        seq(
          field("name", $.identifier),
          optional(field("indices", $.index_bindings)),
          ":",
          "add", "(", field("comp", $.identifier), ",", field("count", $.expr), ")",
          "{", repeat($.recurring_kv), "}",
          optional($.where_clause),
        ),
        // add(COMP, EXPR) every PERIOD at_day DAY
        seq(
          field("name", $.identifier),
          optional(field("indices", $.index_bindings)),
          ":",
          "add", "(", field("comp", $.identifier), ",", field("count", $.expr), ")",
          "every", field("period", $.expr),
          "at_day", field("day", $.expr),
          optional($.where_clause),
        ),
      ),

    transfer_kwarg: ($) =>
      choice(
        seq(field("key", $.identifier), "=", field("value", $.expr)),
        seq("from", "=", field("value", $.expr)),
        seq("to",   "=", field("value", $.expr)),
        seq("count", "=", field("value", $.expr)),
      ),

    iv_kv: ($) =>
      choice(
        seq("at", "=", "[", commaSep($.expr), "]"),
        seq("every", "=", $.expr, "from", "=", $.expr, "to", "=", $.expr),
        seq($.identifier, "=", $.expr),
      ),

    recurring_kv: ($) =>
      choice(
        seq("every", "=", $.expr),
        seq("from",  "=", $.expr),
        seq("until", "=", $.expr),
      ),

    // ── ODE block ────────────────────────────────────────────────────────

    ode_block: ($) =>
      seq("ode", "{", repeat($.ode_decl), "}"),

    ode_decl: ($) =>
      seq(field("comp", $.identifier), "=", field("deriv", $.expr)),

    // ── Output block ─────────────────────────────────────────────────────

    output_block: ($) =>
      seq("output", "{", repeat($.output_section), "}"),

    output_section: ($) =>
      seq($.identifier, "{", repeat($.func_arg), "}"),

    // ── Simulate block ───────────────────────────────────────────────────

    simulate_block: ($) =>
      seq("simulate", "{", repeat($.simulate_kv), "}"),

    simulate_kv: ($) =>
      choice(
        seq("from", "=", $.expr),
        seq("to",   "=", $.expr),
      ),

    // ── Init block ───────────────────────────────────────────────────────

    init_block: ($) =>
      seq("init", "{", repeat($.init_entry), "}"),

    init_entry: ($) =>
      seq(
        field("comp", $.identifier),
        // brackets may carry either concrete index items (S[young] = ...)
        // or loop bindings (S[p in patch] = ...). One unified rule with a
        // choice inside avoids the ambiguity tree-sitter would otherwise see
        // when only the bracket content distinguishes the two forms.
        optional($.init_brackets),
        "=",
        field("value", $.expr),
      ),

    init_brackets: ($) =>
      seq("[", commaSep(choice($.index_binding, $.index_item)), "]"),

    // ── Timepoints block ─────────────────────────────────────────────────

    timepoints_block: ($) =>
      seq("timepoints", "{", repeat($.timepoint_decl), "}"),

    timepoint_decl: ($) =>
      seq(field("name", $.identifier), "=", field("time", $.expr)),

    // ── Stratify ─────────────────────────────────────────────────────────

    stratify_decl: ($) =>
      seq("stratify", "(", commaSep($.stratify_kv), ")"),

    stratify_kv: ($) =>
      choice(
        seq("by", "=", field("dim", $.identifier)),
        seq("values", "=", "[", commaSep($.identifier), "]"),
        seq("only", "=", "[", commaSep($.identifier), "]"),
      ),

    // ── Let binding (optional kind annotation) ───────────────────────────

    let_decl: ($) =>
      seq(
        "let",
        field("name", $.identifier),
        optional(field("indices", $.index_bindings)),
        optional(seq(":", field("kind", $.param_kind))),
        "=",
        field("body", $.expr),
      ),

    // ── Scenarios ────────────────────────────────────────────────────────

    scenarios_block: ($) =>
      seq("scenarios", "{", repeat($.scenario_block), "}"),

    scenario_block: ($) =>
      seq(
        field("name", $.identifier),
        "{",
        repeat($.scenario_field),
        "}",
      ),

    scenario_field: ($) =>
      choice(
        // simulate { to = ... } — nested simulate overrides
        seq("simulate", "{", repeat($.simulate_kv), "}"),
        // set | scale = { name = value, ... } — param overrides
        seq(
          field("kind", choice("set", "scale")),
          "=", "{", repeat($.scenario_kv_item), "}",
        ),
        // enable | disable | compose = [ident, ident, ...] — toggles
        seq(
          field("kind", choice("enable", "disable", "compose")),
          "=", "[", commaSep(field("ref", $.identifier)), "]",
        ),
        // label = "..."
        seq("label", "=", $.string),
        // extends = other_scenario
        seq("extends", "=", $.expr),
      ),

    scenario_kv_item: ($) =>
      seq(
        field("name", $.identifier),
        optional(seq("[", commaSep(field("idx", $.identifier)), "]")),
        "=",
        field("value", $.expr),
      ),

    // ── Balance ──────────────────────────────────────────────────────────

    // balance { S = N - I - R } — population-conservation constraint applied
    // last every substep.
    balance_block: ($) =>
      seq(
        "balance", "{",
        field("comp", $.identifier),
        "=",
        field("expr", $.expr),
        "}",
      ),

    // ── Expressions ──────────────────────────────────────────────────────

    expr: ($) =>
      choice(
        $.cond_expr,
        $.binary_expr,
        $.unary_expr,
        $.sum_expr,
        $.call_expr,
        $.index_expr,
        $.list_expr,
        $.paren_expr,
        $.unit_number,
        $.number,
        $.string,
        $.identifier,
        "null",
      ),

    cond_expr: ($) =>
      prec.right(0, seq("if", $.expr, "then", $.expr, "else", $.expr)),

    binary_expr: ($) =>
      choice(
        prec.left(1,  seq($.expr, choice("==", "!=", "<", ">", "<=", ">="), $.expr)),
        prec.left(2,  seq($.expr, "+",  $.expr)),
        prec.left(2,  seq($.expr, "-",  $.expr)),
        prec.left(3,  seq($.expr, "*",  $.expr)),
        prec.left(3,  seq($.expr, "/",  $.expr)),
        prec.left(3,  seq($.expr, "×",  $.expr)),
        prec.right(4, seq($.expr, "^",  $.expr)),
      ),

    unary_expr: ($) => prec(5, seq("-", $.expr)),

    sum_expr: ($) =>
      seq(
        "sum",
        "(",
        field("var", $.identifier),
        "in",
        field("dim", $.identifier),
        ",",
        field("body", $.expr),
        ")",
      ),

    call_expr: ($) =>
      seq(
        field("func", $.identifier),
        "(",
        commaSep($.kw_arg),
        ")",
      ),

    kw_arg: ($) =>
      choice(
        seq(field("key", $.identifier), "=", field("value", $.expr)),
        field("value", $.expr),
      ),

    index_expr: ($) =>
      seq(
        field("name", $.identifier),
        "[",
        commaSep($.index_item),
        "]",
      ),

    list_expr: ($) =>
      seq("[", commaSep(choice($.range_expr, $.expr)), "]"),

    // `start : end` range expressions appear inside list literals, e.g.
    // `on = [7 'days : 100 'days, 115 'days : 199 'days]`.
    range_expr: ($) =>
      seq(field("start", $.expr), ":", field("end", $.expr)),

    paren_expr: ($) => seq("(", $.expr, ")"),

    unit_number: ($) =>
      seq(field("value", $.number), field("unit", $.unit_literal)),

    // ── Terminals ────────────────────────────────────────────────────────

    // 'days, 'weeks, 'months, 'years, 'per_day, 'per_week, 'per_month,
    // 'per_year, 'count, 'ratio — matches the lexer's UNIT_IDENT set.
    unit_literal: (_) =>
      token(
        seq(
          "'",
          choice(
            "days", "weeks", "months", "years",
            "per_day", "per_week", "per_month", "per_year",
            "count", "ratio",
          ),
        ),
      ),

    number: (_) =>
      token(
        choice(
          // integer with optional underscore grouping (1_000_000)
          /[0-9]+(_[0-9]+)*/,
          // float
          /[0-9]+(_[0-9]+)*\.[0-9]*/,
          /\.[0-9]+/,
          /[0-9]+(_[0-9]+)*(\.[0-9]*)?[eE][+-]?[0-9]+/,
        ),
      ),

    string: (_) => token(seq('"', /[^"]*/, '"')),

    identifier: (_) => /[a-zA-Z_][a-zA-Z0-9_]*/,

    comment: (_) => token(seq("#", /[^\[].*/)),
  },
});

// ── Helpers ──────────────────────────────────────────────────────────────────

/** Zero or more, comma-separated. */
function commaSep(rule) {
  return optional(commaSep1(rule));
}

/** One or more, comma-separated. */
function commaSep1(rule) {
  return seq(rule, repeat(seq(",", rule)));
}

/** One or more, separated by `sep`. */
function sep1(rule, sep) {
  return seq(rule, repeat(seq(sep, rule)));
}
