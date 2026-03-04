; Keywords
[
  "module"
  "use"
  "as"
  "rule"
  "entity"
  "external"
  "value"
  "enum"
  "context"
  "config"
  "surface"
  "actor"
  "default"
  "variant"
  "let"
  "not"
  "and"
  "or"
  "contract"
  "invariant"
  "implies"
  "deferred"
  "given"
] @keyword

(clause_keyword) @keyword
(annotation_keyword) @keyword

; Types (Declarations)
(rule_declaration name: (identifier) @type)
(entity_declaration name: (identifier) @type)
(external_entity_declaration name: (identifier) @type)
(value_declaration name: (identifier) @type)
(enum_declaration name: (identifier) @type)
(surface_declaration name: (identifier) @type)
(actor_declaration name: (identifier) @type)
(default_declaration type: (identifier) @type)
(variant_declaration name: (identifier) @type)
(contract_declaration name: (identifier) @type)
(invariant_declaration name: (identifier) @type)

; Variables / Fields
(field_assignment key: (identifier) @variable)
(let_binding name: (identifier) @variable)
(named_argument name: (identifier) @variable)

; Functions
(call_expression
  function: (identifier) @function.call)
(call_expression
  function: (member_expression
    property: (identifier) @function.call))

; Literals
(string_literal) @string
(escape_sequence) @string.escape
(string_interpolation
  "{" @punctuation.special
  (identifier) @variable.parameter
  "}" @punctuation.special) @string.special

(number_literal) @number
(duration_literal) @number
(boolean_literal) @boolean
(null_literal) @constant.builtin

; Operators & Punctuation
[
  "="
  "=="
  "!="
  "<"
  ">"
  "<="
  ">="
  "=>"
  "->"
  "+"
  "-"
  "*"
  "/"
  "|"
] @operator

[
  "("
  ")"
  "{"
  "}"
] @punctuation.bracket

[
  ":"
  ","
  "."
] @punctuation.delimiter

"@" @punctuation.special

; Comments
(comment) @comment
