; ── Scopes ────────────────────────────────────────────────────────────────────

; Each top-level block is its own scope
(compartments_block)  @local.scope
(parameters_block)    @local.scope
(tables_block)        @local.scope
(functions_block)     @local.scope
(forcing_block)       @local.scope
(transitions_block)   @local.scope
(observations_block)  @local.scope
(interventions_block) @local.scope
(events_block)        @local.scope
(init_block)          @local.scope
(scenarios_block)     @local.scope
(balance_block)       @local.scope
(dimensions_block)    @local.scope

; let and index_bindings introduce variables
(let_decl name: (identifier) @local.definition)

(index_binding var: (identifier)  @local.definition)
(index_binding next: (identifier) @local.definition)

; Compartment / parameter / table names are file-scope definitions
(compartment_decl name: (identifier) @local.definition)
(parameter_decl   name: (identifier) @local.definition)
(table_decl       name: (identifier) @local.definition)
(function_decl    name: (identifier) @local.definition)
(transition_decl  name: (identifier) @local.definition)
(obs_decl         name: (identifier) @local.definition)
(intervention_decl name: (identifier) @local.definition)
(timepoint_decl   name: (identifier) @local.definition)
(dim_entry        name: (identifier) @local.definition)
(scenario_block   name: (identifier) @local.definition)

; References
(identifier) @local.reference
