; ── Top-level block keywords ─────────────────────────────────────────────────

[
  "time_unit"
  "description"
  "origin"
  "dimensions"
  "compartments"
  "parameters"
  "tables"
  "functions"
  "forcing"
  "transitions"
  "observations"
  "interventions"
  "events"
  "ode"
  "output"
  "simulate"
  "init"
  "timepoints"
  "stratify"
  "let"
  "scenarios"
  "balance"
] @keyword

; ── Scenario verbs / intervention verbs / schedule keywords ──────────────────

[
  "from"
  "to"
  "where"
  "in"
  "by"
  "values"
  "only"
  "at"
  "at_day"
  "every"
  "until"
  "tag"
  "transfer"
  "add"
  "consecutive"
  "extends"
  "set"
  "scale"
  "enable"
  "disable"
  "compose"
  "label"
  "likelihood"
] @keyword.operator

; ── Conditionals ─────────────────────────────────────────────────────────────

[
  "if"
  "then"
  "else"
] @keyword.conditional

[
  "and"
  "or"
] @keyword.operator

"sum" @keyword.function

; ── Attributes — `#[lineage]` and any future #[…] ─────────────────────────────

(attribute name: (identifier) @attribute)
(hash_lbracket) @punctuation.special

; ── Types / kinds ─────────────────────────────────────────────────────────────

(param_kind) @type.builtin

[
  "real"
  "integer"
] @type.builtin

(dim_literal) @type            ; `[1]`, `[P]`, `[T^-1]`, `[P*T^-1]`, etc.

; ── Operators ─────────────────────────────────────────────────────────────────

[
  "-->"
  "@"
  "~"
  "=="
  "!="
  "<"
  ">"
  "<="
  ">="
  "+"
  "-"
  "*"
  "/"
  "×"
  "^"
  "="
] @operator

; ── Punctuation ───────────────────────────────────────────────────────────────

[ "{" "}" ] @punctuation.bracket
[ "[" "]" ] @punctuation.bracket
[ "(" ")" ] @punctuation.bracket
[ "," ":" ] @punctuation.delimiter

; ── Literals ──────────────────────────────────────────────────────────────────

(number) @number
(unit_number value: (number) @number)
(unit_literal) @attribute           ; 'days, 'per_day, 'count, 'ratio, etc.
(string) @string
"null" @constant.builtin

; The ISO date string in `origin = date("YYYY-MM-DD")` gets a more specific
; tag so themes can highlight it distinctly from generic strings.
(origin_decl iso_date: (string) @string.special)

; ── Declarations — names ──────────────────────────────────────────────────────

(compartment_decl name: (identifier) @variable.parameter)
(parameter_decl   name: (identifier) @variable.parameter)
(table_decl       name: (identifier) @variable.parameter)
(function_decl    name: (identifier) @function)
(ode_decl         comp: (identifier) @variable.parameter)
(let_decl         name: (identifier) @variable.parameter)
(timepoint_decl   name: (identifier) @variable.parameter)
(dim_entry        name: (identifier) @type)
(scenario_block   name: (identifier) @variable.parameter)
(balance_block    comp: (identifier) @variable.parameter)

(transition_decl  name: (identifier) @function)
(branch_entry     name: (identifier) @variable.parameter)

(obs_decl          name: (identifier) @function)
(intervention_decl name: (identifier) @function)

; ── Index bindings ────────────────────────────────────────────────────────────

(index_binding var:  (identifier) @variable)
(index_binding dim:  (identifier) @type)
(index_binding next: (identifier) @variable)

(table_dim_entry dim:        (identifier) @type)

(param_index dim: (identifier) @type)
(param_prior dist: (identifier) @function.builtin)
(param_prior pool_over: (identifier) @type)

(dim_inline level: (identifier) @constant)

; ── Expressions — identifiers ─────────────────────────────────────────────────

; Generic identifier (fallback — lower priority than named fields above)
(identifier) @variable

(call_expr func: (identifier) @function.call)
(index_expr name: (identifier) @variable)
(sum_expr   var: (identifier)  @variable)
(sum_expr   dim: (identifier)  @type)

; Known built-in functions — these get the .builtin variant which themes
; can color distinctly from user functions.
((call_expr func: (identifier) @function.builtin)
  (#match? @function.builtin
   "^(date|add_calendar_days|add_calendar_weeks|add_calendar_months|add_calendar_years|date_range|read|read_levels|read_long|defines|incidence|cumulative|prevalence|overdispersed|deterministic|exp|log|min|max|mod|abs|sqrt|floor|ceil|round)$"))

; Distribution names (in priors and likelihoods).
((call_expr func: (identifier) @function.builtin)
  (#match? @function.builtin
   "^(poisson|neg_binomial|normal|binomial|beta_binomial|bernoulli|log_normal|half_normal|beta|gamma|exponential|uniform|diagnostic_test)$"))

; ── Stoich refs ───────────────────────────────────────────────────────────────

(stoich_ref name: (identifier) @variable.parameter)

; ── Guard expressions ─────────────────────────────────────────────────────────

(guard_atom left:  (identifier) @variable)
(guard_atom right: (identifier) @variable)

; ── Stratify ──────────────────────────────────────────────────────────────────

(stratify_kv (identifier) @type)

; ── Scenario contents ────────────────────────────────────────────────────────

(scenario_field ref: (identifier) @function)             ; enable = [iv_name]
(scenario_kv_item name: (identifier) @variable.parameter)

; ── Comments ──────────────────────────────────────────────────────────────────

(comment) @comment
