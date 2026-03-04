/**
 * Tree-sitter grammar for the Allium language.
 *
 * Allium is a specification language with block-based declarations (rule,
 * entity, enum, config, given, surface, actor, value) and clause-driven
 * rule bodies (when:, requires:, ensures:).
 */

/// <reference types="tree-sitter-cli/dsl" />
// @ts-check

const PREC = {
  COMMA: 1,
  IMPLIES: 2,
  LAMBDA: 3,
  OR: 4,
  NULLISH_COALESCE: 5,
  AND: 6,
  NOT: 7,
  COMPARE: 8,
  ADD: 9,
  MULTIPLY: 10,
  INFIX: 11,
  PIPE: 12,
  CALL: 13,
  MEMBER: 14,
  PRIMARY: 15,
};

module.exports = grammar({
  name: "allium",

  word: ($) => $.identifier,

  extras: ($) => [$.comment, /[ \t\r\n]+/],

  conflicts: ($) => [
    [$.default_declaration],
    [$._expression, $.infix_predicate_expression],
    [$.annotation],
  ],

  rules: {
    source_file: ($) =>
      seq(optional($.module_declaration), repeat($._top_level_item)),

    // -----------------------------------------------------------------------
    // Comments
    // -----------------------------------------------------------------------

    comment: (_) => token(seq("--", /[^\n]*/)),

    // -----------------------------------------------------------------------
    // Primitives
    // -----------------------------------------------------------------------

    identifier: (_) => /[A-Za-z_][A-Za-z0-9_]*/,

    string_literal: ($) =>
      seq(
        '"',
        repeat(
          choice(
            token.immediate(/[^"\\{\n]+/),
            $.escape_sequence,
            $.string_interpolation,
          ),
        ),
        '"',
      ),

    escape_sequence: (_) => token.immediate(seq("\\", /./)),

    string_interpolation: ($) =>
      seq(token.immediate("{"), $.identifier, token.immediate("}")),

    // duration must be tried before plain number (longer match wins)
    duration_literal: (_) =>
      token(/\d+(\.\d+)?\.(seconds?|minutes?|hours?|days?|weeks?|months?|years?)/),

    number_literal: (_) => /\d+(\.\d+)?/,

    boolean_literal: (_) => choice("true", "false"),

    null_literal: (_) => "null",

    // -----------------------------------------------------------------------
    // Module header
    // -----------------------------------------------------------------------

    module_declaration: ($) => seq("module", field("name", $.identifier)),

    // -----------------------------------------------------------------------
    // Top-level declarations
    // -----------------------------------------------------------------------

    _top_level_item: ($) =>
      choice(
        $.use_declaration,
        $.rule_declaration,
        $.entity_declaration,
        $.external_entity_declaration,
        $.value_declaration,
        $.enum_declaration,
        $.given_block,
        $.config_block,
        $.surface_declaration,
        $.actor_declaration,
        $.default_declaration,
        $.variant_declaration,
        $.deferred_declaration,
        $.contract_declaration,
        $.invariant_declaration,
      ),

    use_declaration: ($) =>
      seq(
        "use",
        field("path", $.string_literal),
        optional(seq("as", field("alias", $.identifier))),
      ),

    rule_declaration: ($) =>
      seq("rule", field("name", $.identifier), field("body", $.block_body)),

    entity_declaration: ($) =>
      seq("entity", field("name", $.identifier), field("body", $.block_body)),

    external_entity_declaration: ($) =>
      seq(
        "external",
        "entity",
        field("name", $.identifier),
        field("body", $.block_body),
      ),

    value_declaration: ($) =>
      seq("value", field("name", $.identifier), field("body", $.block_body)),

    enum_declaration: ($) =>
      seq("enum", field("name", $.identifier), field("body", $.block_body)),

    given_block: ($) => seq("given", field("body", $.block_body)),

    config_block: ($) => seq("config", field("body", $.block_body)),

    surface_declaration: ($) =>
      seq("surface", field("name", $.identifier), field("body", $.block_body)),

    actor_declaration: ($) =>
      seq("actor", field("name", $.identifier), field("body", $.block_body)),

    // "default [TypeName] instanceName = expression"
    default_declaration: ($) =>
      seq(
        "default",
        optional(field("type", $.identifier)),
        field("name", $.identifier),
        "=",
        field("value", $._expression),
      ),

    // "variant Name: sum_type_expression"
    variant_declaration: ($) =>
      seq(
        "variant",
        field("name", $.identifier),
        ":",
        field("type", $._expression),
      ),

    // "deferred path.expression"
    deferred_declaration: ($) =>
      seq("deferred", field("path", $._expression)),

    // "contract Name { ... }"
    contract_declaration: ($) =>
      seq("contract", field("name", $.identifier), field("body", $.block_body)),

    // "invariant Name { expr }" — expression-bearing invariant
    invariant_declaration: ($) =>
      seq("invariant", field("name", $.identifier), "{", field("body", $._expression), "}"),

    // -----------------------------------------------------------------------
    // Block body
    // -----------------------------------------------------------------------

    block_body: ($) => seq("{", repeat(seq($._block_item, optional(","))), "}"),

    _block_item: ($) =>
      choice($.clause, $.field_assignment, $.let_binding, $.open_question, $.annotation),

    // Clause: reserved keyword followed by colon and an expression
    clause: ($) =>
      seq(
        field("keyword", $.clause_keyword),
        ":",
        field("value", choice($._expression, $.tuple_expression)),
      ),

    // Clause keywords are reserved — the `word` rule ensures they cannot be
    // matched as plain identifiers.
    clause_keyword: (_) =>
      choice(
        "when",
        "requires",
        "ensures",
        "trigger",
        "provides",
        "tags",
        "guidance",
        "invariant",
        "becomes",
        "related",
        "exposes",
        "identified_by",
        "facing",
        "transitions_to",
        "guarantee",
        "timeout",
        "within",
        "contracts",
      ),

    // Field assignment: plain identifier followed by colon and an expression
    field_assignment: ($) =>
      seq(
        field("key", $.identifier),
        ":",
        field("value", $._expression),
      ),

    // Let binding: "let name = expression"
    let_binding: ($) =>
      seq(
        "let",
        field("name", $.identifier),
        "=",
        field("value", $._expression),
      ),

    // open_question: 'open question "..."' marker
    open_question: ($) =>
      seq("open", "question", field("text", $.string_literal)),

    // Annotation: @invariant Name, @guidance, @guarantee Name
    annotation: ($) =>
      seq("@", field("kind", $.annotation_keyword), optional(field("name", $.identifier))),

    annotation_keyword: (_) => choice("invariant", "guidance", "guarantee"),

    // -----------------------------------------------------------------------
    // Expressions
    // -----------------------------------------------------------------------

    _expression: ($) =>
      choice(
        $.lambda_expression,
        $.thin_arrow_expression,
        $.implies_expression,
        $.or_expression,
        $.null_coalescing_expression,
        $.and_expression,
        $.not_expression,
        $.comparison_expression,
        $.additive_expression,
        $.multiplicative_expression,
        $.infix_predicate_expression,
        $.pipe_expression,
        $.call_expression,
        $.member_expression,
        $.optional_member_expression,
        $.string_literal,
        $.duration_literal,
        $.number_literal,
        $.boolean_literal,
        $.null_literal,
        $.identifier,
        $.block_expression,
      ),

    block_expression: ($) => $.block_body,

    tuple_expression: ($) =>
      prec.left(
        PREC.COMMA,
        seq(
          field("left", choice($._expression, $.tuple_expression)),
          ",",
          field("right", choice($._expression, $.tuple_expression)),
        ),
      ),

    // Implies: right-associative, below or
    implies_expression: ($) =>
      prec.right(
        PREC.IMPLIES,
        seq(
          field("left", $._expression),
          "implies",
          field("right", $._expression),
        ),
      ),

    // Thin arrow: typed signatures "(value: Any) -> ByteArray"
    thin_arrow_expression: ($) =>
      prec.left(
        PREC.LAMBDA,
        seq(
          field("left", $._expression),
          "->",
          field("right", $._expression),
        ),
      ),

    // Boolean OR
    or_expression: ($) =>
      prec.left(
        PREC.OR,
        seq(
          field("left", $._expression),
          "or",
          field("right", $._expression),
        ),
      ),

    // Boolean AND
    and_expression: ($) =>
      prec.left(
        PREC.AND,
        seq(
          field("left", $._expression),
          "and",
          field("right", $._expression),
        ),
      ),

    // Unary NOT
    not_expression: ($) =>
      prec(PREC.NOT, seq("not", field("operand", $._expression))),

    // Comparisons: = == != < > <= >=
    comparison_expression: ($) =>
      prec.left(
        PREC.COMPARE,
        seq(
          field("left", $._expression),
          field("operator", choice("=", "==", "!=", "<", ">", "<=", ">=")),
          field("right", $._expression),
        ),
      ),

    additive_expression: ($) =>
      prec.left(
        PREC.ADD,
        seq(
          field("left", $._expression),
          field("operator", choice("+", "-")),
          field("right", $._expression),
        ),
      ),

    multiplicative_expression: ($) =>
      prec.left(
        PREC.MULTIPLY,
        seq(
          field("left", $._expression),
          field("operator", choice("*", "/")),
          field("right", $._expression),
        ),
      ),

    // Infix predicate: "expr predicate_name" — two adjacent identifiers used
    // in requires/ensures clauses like "symbol resolves_to_single_definition"
    infix_predicate_expression: ($) =>
      prec.left(
        PREC.INFIX,
        seq(
          field("subject", $._expression),
          field("predicate", $.identifier),
        ),
      ),

    // Pipe / sum-type: "a | b | c"
    pipe_expression: ($) =>
      prec.left(
        PREC.PIPE,
        seq(
          field("left", $._expression),
          "|",
          field("right", $._expression),
        ),
      ),

    // Function / method call: "Name(...)" or "a.Name(...)"
    call_expression: ($) =>
      prec(
        PREC.CALL,
        seq(
          field("function", $._expression),
          "(",
          optional(field("arguments", $.argument_list)),
          ")",
        ),
      ),

    argument_list: ($) =>
      seq(
        $._argument,
        repeat(seq(optional(","), $._argument)),
      ),

    _argument: ($) =>
      choice(
        $.named_argument,
        $._expression,
      ),

    named_argument: ($) =>
      seq(
        field("name", $.identifier),
        ":",
        field("value", $._expression),
      ),

    // Member / dot access: "a.b"
    member_expression: ($) =>
      prec(
        PREC.MEMBER,
        seq(
          field("object", $._expression),
          ".",
          field("property", $.identifier),
        ),
      ),

    // Optional member / dot access: "a?.b"
    optional_member_expression: ($) =>
      prec(
        PREC.MEMBER,
        seq(
          field("object", $._expression),
          "?.",
          field("property", $.identifier),
        ),
      ),

    // Null coalescing: "a ?? b"
    null_coalescing_expression: ($) =>
      prec.left(
        PREC.NULLISH_COALESCE,
        seq(
          field("left", $._expression),
          "??",
          field("right", $._expression),
        ),
      ),

    // Lambda: "params => body"
    lambda_expression: ($) =>
      prec.right(
        PREC.LAMBDA,
        seq(
          field("parameters", $._expression),
          "=>",
          field("body", $._expression),
        ),
      ),
  },
});
