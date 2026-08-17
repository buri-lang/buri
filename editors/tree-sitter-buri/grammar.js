// GENERATED from cli/src/docs/grammar.ebnf — do not edit.
//
// The EBNF is the normative grammar and the only place this language's syntax
// is written down. It carries what tree-sitter needs beyond a context-free
// grammar — node names, hidden rules, field names, the external scanner's
// terminals, and the precedence cascade — as `@` directives in its comments,
// and `cli/src/documentation/grammar.rs` turns them into this file.
//
// To change the grammar, edit the EBNF and run:
//
//   BURI_BLESS=1 cargo test -p buri --test corpus the_tree_sitter_grammar
//
// `src/scanner.c` is hand-written and stays that way: string interpolation and
// nestable block comments need a lexer with state, which no declarative
// grammar can express. The order of `externals` below is the order of the
// enum in that file.

module.exports = grammar({
  name: 'buri',

  word: $ => $.identifier,

  extras: $ => [/[ \t\r\n]+/, $.line_comment, $.doc_comment, $.module_doc, $.block_comment],

  externals: $ => [
    $.string_literal,
    $.template_head,
    $.template_span,
    $.template_tail,
    $.block_comment,
  ],

  conflicts: $ => [
    [$._operand, $.generic_expression],
  ],

  rules: {
    source_file: $ => repeat($._item),

    _item: $ => choice(
      $.import,
      $.re_export,
      $.exportable,
      $.impl_declaration,
      $.derive_declaration,
      $.test_declaration
    ),

    import: $ => seq('from', field('path', $.string_literal), 'import', $._import_clause, ';'),

    _import_clause: $ => choice(seq('{', optional($.import_specifiers), '}'), $.namespace_import),

    namespace_import: $ => seq('*', 'as', field('name', $.identifier)),

    import_specifiers: $ => seq($.import_specifier, repeat(seq(',', $.import_specifier)), optional(',')),

    import_specifier: $ => seq(field('name', $.identifier), optional(seq('as', field('alias', $.identifier)))),

    re_export: $ => seq(
      'from',
      field('path', $.string_literal),
      'export',
      '{',
      optional($.import_specifiers),
      '}',
      ';'
    ),

    exportable: $ => seq(optional('export'), $._exportable_body),

    _exportable_body: $ => choice(
      $.function_declaration,
      $.struct_declaration,
      $.enum_declaration,
      $.type_alias_declaration,
      $.const_declaration,
      $.trait_declaration,
      $.effect_declaration,
      $.context_declaration
    ),

    function_declaration: $ => seq(
      'fn',
      field('name', $.identifier),
      optional($.generic_parameters),
      '(',
      optional($.parameters),
      ')',
      ':',
      field('return_type', $._type),
      choice(field('body', $.block), ';')
    ),

    parameters: $ => choice(
      seq(
        $.self_parameter,
        optional(seq(',', $.ctx_parameter)),
        repeat(seq(',', $.parameter)),
        optional(',')
      ),
      seq($.ctx_parameter, repeat(seq(',', $.parameter)), optional(',')),
      seq($.parameter, repeat(seq(',', $.parameter)), optional(','))
    ),

    self_parameter: $ => seq('self', ':', $._type),

    ctx_parameter: $ => seq('ctx', ':', $._type),

    parameter: $ => seq(field('name', $.identifier), ':', field('type', $._type)),

    generic_parameters: $ => seq('<', $.generic_parameter, repeat(seq(',', $.generic_parameter)), optional(','), '>'),

    generic_parameter: $ => seq(field('name', $.identifier), optional(seq(':', $.bounds))),

    bounds: $ => seq($.named_type, repeat(seq('+', $.named_type))),

    struct_declaration: $ => seq(
      'struct',
      field('name', $.identifier),
      optional($.generic_parameters),
      choice($.field_declarations_block, $.tuple_fields_block)
    ),

    field_declarations_block: $ => seq('{', optional($.field_declarations), '}'),

    tuple_fields_block: $ => seq('(', optional($.tuple_fields), ')', ';'),

    field_declarations: $ => seq($.field_declaration, repeat(seq(',', $.field_declaration)), optional(',')),

    field_declaration: $ => seq(optional('export'), field('name', $.identifier), ':', field('type', $._type)),

    tuple_fields: $ => seq($.tuple_field, repeat(seq(',', $.tuple_field)), optional(',')),

    tuple_field: $ => seq(optional('export'), $._type),

    enum_declaration: $ => seq(
      'enum',
      field('name', $.identifier),
      optional($.generic_parameters),
      '{',
      optional($.variants),
      '}'
    ),

    variants: $ => seq($.variant, repeat(seq(',', $.variant)), optional(',')),

    variant: $ => seq(optional('export'), field('name', $.identifier), optional($._variant_payload)),

    _variant_payload: $ => choice(seq('(', $.types, ')'), seq('{', $.field_declarations, '}')),

    type_alias_declaration: $ => seq(
      'type',
      field('name', $.identifier),
      optional($.generic_parameters),
      '=',
      $._type,
      ';'
    ),

    const_declaration: $ => seq(
      'const',
      field('name', $.identifier),
      ':',
      field('type', $._type),
      '=',
      field('value', $._expression),
      ';'
    ),

    trait_declaration: $ => seq(
      'trait',
      field('name', $.identifier),
      optional($.generic_parameters),
      '{',
      repeat($.function_declaration),
      '}'
    ),

    effect_declaration: $ => seq(
      'effect',
      field('name', $.identifier),
      optional($.generic_parameters),
      '{',
      repeat($.function_declaration),
      '}'
    ),

    impl_declaration: $ => choice(
      seq(
        'impl',
        optional($.generic_parameters),
        field('type', $._type),
        '{',
        repeat($.impl_method),
        '}'
      ),
      seq(
        'impl',
        optional($.generic_parameters),
        field('type', $._type),
        'for',
        field('trait', $._type),
        '{',
        repeat($.function_declaration),
        '}'
      )
    ),

    impl_method: $ => seq(optional('export'), $.function_declaration),

    derive_declaration: $ => seq(
      'derive',
      $.named_type,
      repeat(seq(',', $.named_type)),
      'for',
      field('type', $.named_type),
      ';'
    ),

    context_declaration: $ => seq('context', field('name', $.identifier), $.context_body),

    context_expression: $ => seq('context', $.context_body),

    context_body: $ => seq('{', optional($.spread), optional($.context_bindings), '}'),

    context_bindings: $ => seq($.context_binding, repeat(seq(',', $.context_binding)), optional(',')),

    context_binding: $ => seq(field('effect', $.named_type), ':', field('value', $._expression)),

    test_declaration: $ => seq('test', field('name', $.string_literal), field('body', $.block)),

    _type: $ => choice($.function_type, $._primary_type),

    function_type: $ => seq('fn', '(', optional($.types), ')', '=>', $._type),

    types: $ => seq($._type, repeat(seq(',', $._type)), optional(',')),

    _primary_type: $ => choice(
      $.named_type,
      $.self_type,
      $.unit_type,
      $.tuple_type,
      $.array_type,
      $.grouped_type
    ),

    self_type: $ => 'Self',

    unit_type: $ => seq('(', ')'),

    named_type: $ => seq($.type_path, optional($.type_arguments)),

    type_path: $ => seq($.identifier, repeat(seq('.', $.identifier))),

    type_arguments: $ => seq('<', $._type, repeat(seq(',', $._type)), optional(','), '>'),

    array_type: $ => seq('[', $._type, ']'),

    grouped_type: $ => seq('(', $._type, ')'),

    tuple_type: $ => seq('(', $._type, repeat1(seq(',', $._type)), optional(','), ')'),

    _expression: $ => choice($.lambda, $._operand),

    lambda: $ => seq(
      'fn',
      '(',
      optional($.lambda_parameters),
      ')',
      optional(seq(':', $._type)),
      '=>',
      $._expression
    ),

    lambda_parameters: $ => seq($.lambda_parameter, repeat(seq(',', $.lambda_parameter)), optional(',')),

    lambda_parameter: $ => seq(field('name', $.identifier), optional(seq(':', $._type))),

    _operand: $ => choice(
      $.binary_expression,
      $.unary_expression,
      $._block_like_expression,
      $._postfix_expression
    ),

    binary_expression: $ => choice(
      prec.left(1, seq($._operand, '||', $._operand)),
      prec.right(2, seq($._operand, '??', $._operand)),
      prec.left(3, seq($._operand, '&&', $._operand)),
      prec.left(4, seq($._operand, choice('==', '!=', '<', '<=', '>', '>='), $._operand)),
      prec.left(5, seq($._operand, '|', $._operand)),
      prec.left(6, seq($._operand, '^', $._operand)),
      prec.left(7, seq($._operand, '&', $._operand)),
      prec.left(8, seq($._operand, choice('+', '-'), $._operand)),
      prec.left(9, seq($._operand, choice('*', '/', '%'), $._operand))
    ),

    unary_expression: $ => prec.right(10, seq(choice('-', '!', '~'), $._operand)),

    _postfix_expression: $ => choice(
      $._primary_expression,
      $.field_expression,
      $.tuple_index_expression,
      $.call_expression,
      $.index_expression,
      $.try_expression,
      $.generic_expression,
      $.struct_literal
    ),

    _block_like_expression: $ => choice($.block, $.if_expression, $.match_expression, $.context_expression),

    field_expression: $ => prec(11, seq(field('value', $._postfix_expression), '.', field('field', $.identifier))),

    tuple_index_expression: $ => prec(
      11,
      seq(field('value', $._postfix_expression), '.', field('index', $.integer_literal))
    ),

    call_expression: $ => prec(11, seq(field('function', $._postfix_expression), '(', optional($.arguments), ')')),

    index_expression: $ => prec(11, seq(field('value', $._postfix_expression), '[', $._expression, ']')),

    try_expression: $ => prec(11, seq($._postfix_expression, '?')),

    generic_expression: $ => prec.dynamic(1, seq($._postfix_expression, $.type_arguments)),

    struct_literal: $ => prec(11, seq(field('type', $._postfix_expression), $.struct_literal_body)),

    struct_literal_body: $ => seq('{', optional($.spread), optional($.field_initializers), '}'),

    spread: $ => seq('..', $._expression, optional(',')),

    field_initializers: $ => seq($.field_initializer, repeat(seq(',', $.field_initializer)), optional(',')),

    field_initializer: $ => seq(field('name', $.identifier), optional(seq(':', field('value', $._expression)))),

    arguments: $ => seq($._expression, repeat(seq(',', $._expression)), optional(',')),

    _primary_expression: $ => choice(
      $._literal,
      $.identifier,
      $.self_expression,
      $.ctx_expression,
      $.variant_expression,
      $.unit_expression,
      $.array_expression,
      $.tuple_expression,
      $.grouped_expression
    ),

    self_expression: $ => 'self',

    ctx_expression: $ => 'ctx',

    variant_expression: $ => seq('.', field('name', $.identifier)),

    unit_expression: $ => seq('(', ')'),

    array_expression: $ => seq(
      '[',
      optional(seq($._expression, repeat(seq(',', $._expression)), optional(','))),
      ']'
    ),

    tuple_expression: $ => seq('(', $._expression, repeat1(seq(',', $._expression)), optional(','), ')'),

    grouped_expression: $ => seq('(', $._expression, ')'),

    if_expression: $ => seq(
      'if',
      '(',
      field('condition', $._expression),
      ')',
      field('consequence', $.block),
      'else',
      field('alternative', choice($.block, $.if_expression))
    ),

    match_expression: $ => seq('match', '(', field('value', $._expression), ')', '{', optional($.match_arms), '}'),

    match_arms: $ => seq($.match_arm, repeat(seq(',', $.match_arm)), optional(',')),

    match_arm: $ => seq(
      field('pattern', $._pattern),
      optional($.guard),
      '=>',
      field('value', $._expression)
    ),

    guard: $ => seq('if', $._expression),

    block: $ => seq('{', repeat($._statement), optional($._expression), '}'),

    _statement: $ => choice($.let_statement, $.expression_statement),

    let_statement: $ => choice(
      seq(
        'let',
        field('pattern', $._pattern),
        optional(seq(':', $._type)),
        '=',
        field('value', $._expression),
        ';'
      ),
      seq('let', 'ctx', '=', field('value', $._expression), ';')
    ),

    expression_statement: $ => seq($._expression, ';'),

    _pattern: $ => choice($.or_pattern, $._pattern_no_or),

    or_pattern: $ => prec.left(seq($._pattern_no_or, repeat1(seq('|', $._pattern_no_or)))),

    _pattern_no_or: $ => choice(
      $.bind_pattern,
      $.wildcard_pattern,
      $.literal_pattern,
      $.path_pattern,
      $.unit_pattern,
      $.tuple_pattern,
      $.array_pattern,
      $.grouped_pattern
    ),

    bind_pattern: $ => prec(1, seq(field('name', $.identifier), optional(seq('@', $._pattern_no_or)))),

    wildcard_pattern: $ => '_',

    literal_pattern: $ => choice(
      seq(optional('-'), $.integer_literal),
      seq(optional('-'), $.float_literal),
      $.string_literal,
      $.char_literal,
      $.true,
      $.false
    ),

    path_pattern: $ => prec(
      2,
      choice(
        seq($.identifier, $.pattern_payload),
        seq($.qualified_path, optional($.pattern_payload)),
        seq('.', field('name', $.identifier), optional($.pattern_payload))
      )
    ),

    qualified_path: $ => seq($.identifier, '.', $.identifier, repeat(seq('.', $.identifier))),

    pattern_payload: $ => choice(seq('(', optional($.patterns), ')'), seq('{', optional($.field_patterns), '}')),

    patterns: $ => seq($._pattern, repeat(seq(',', $._pattern)), optional(',')),

    field_patterns: $ => choice(
      seq(
        $.field_pattern,
        repeat(seq(',', $.field_pattern)),
        optional(seq(',', $.rest)),
        optional(',')
      ),
      seq($.rest, optional(','))
    ),

    field_pattern: $ => seq(field('name', $.identifier), optional(seq(':', $._pattern))),

    rest: $ => '..',

    unit_pattern: $ => seq('(', ')'),

    tuple_pattern: $ => seq('(', $._pattern, repeat1(seq(',', $._pattern)), optional(','), ')'),

    grouped_pattern: $ => seq('(', $._pattern, ')'),

    array_pattern: $ => seq('[', optional($._array_pattern_body), ']'),

    _array_pattern_body: $ => choice(
      seq(
        $._pattern,
        repeat(seq(',', $._pattern)),
        optional(seq(',', $.rest_pattern)),
        optional(',')
      ),
      seq($.rest_pattern, optional(','))
    ),

    rest_pattern: $ => seq('..', field('name', optional($.identifier))),

    _literal: $ => choice(
      $.integer_literal,
      $.float_literal,
      $.string_literal,
      $.char_literal,
      $.true,
      $.false,
      $.template_literal
    ),

    true: $ => 'true',

    false: $ => 'false',

    template_literal: $ => seq(
      $.template_head,
      $._expression,
      repeat(seq($.template_span, $._expression)),
      $.template_tail
    ),

    line_comment: $ => /\/\/(?:[^!\/\n][^\n]*|\/\/+[^\n]*|)/,

    doc_comment: $ => /\/\/\/(?:[^\/\n][^\n]*|\/|)/,

    module_doc: $ => /\/\/![^\n]*/,

    identifier: $ => /[A-Za-z_][A-Za-z0-9_]*/,

    integer_literal: $ => /[0-9][0-9_]*|0x[0-9a-fA-F][0-9a-fA-F_]*|0o[0-7][0-7_]*|0b[01][01_]*/,

    float_literal: $ => /[0-9][0-9_]*\.[0-9][0-9_]*(?:[eE][+\-]?[0-9][0-9_]*)?|[0-9][0-9_]*[eE][+\-]?[0-9][0-9_]*/,

    char_literal: $ => /'(?:[^'\\]|\\(?:[nrt0\\"'$]|u\{[0-9a-fA-F]+\}))'/,
  },
});
