; Buri, for tree-sitter. Capture names follow the set Zed and Helix share.

; --- Comments and documentation ---------------------------------------------
(line_comment) @comment
(block_comment) @comment
(doc_comment) @comment.doc
(module_doc) @comment.doc

; --- Literals ----------------------------------------------------------------
(integer_literal) @number
(float_literal) @number
(char_literal) @string.special
(string_literal) @string
(template_head) @string
(template_span) @string
(template_tail) @string
(true) @boolean
(false) @boolean

; --- Keywords ----------------------------------------------------------------
[
  "from" "import" "export" "as"
  "fn" "struct" "enum" "type" "const"
  "trait" "effect" "impl" "derive" "for"
  "let" "test" "context"
] @keyword

["if" "else" "match"] @keyword.control

[(self_expression) (ctx_expression) (self_type)] @variable.builtin

; --- Operators and punctuation ----------------------------------------------
[
  "||" "&&" "??" "==" "!=" "<" "<=" ">" ">="
  "|" "^" "&" "+" "-" "*" "/" "%" "!" "~" "?" "=" "=>" "@" ".."
] @operator

["(" ")" "[" "]" "{" "}"] @punctuation.bracket
["," ";" ":" "."] @punctuation.delimiter

; --- Declarations ------------------------------------------------------------
(function_declaration name: (identifier) @function)
(lambda) @function

(struct_declaration name: (identifier) @type)
(enum_declaration name: (identifier) @type)
(type_alias_declaration name: (identifier) @type)
(trait_declaration name: (identifier) @type.interface)
(effect_declaration name: (identifier) @type.interface)
(context_declaration name: (identifier) @type)

(const_declaration name: (identifier) @constant)

(generic_parameter name: (identifier) @type.parameter)

; A type path's last segment is the type; the ones before it are the module it
; came from, and colouring them the same makes `effects.Alloc` read as one word.
(named_type (type_path (identifier) @type))
(array_type "[" @punctuation.bracket)

(parameter name: (identifier) @variable.parameter)
(lambda_parameter name: (identifier) @variable.parameter)

(field_declaration name: (identifier) @property)
(field_initializer name: (identifier) @property)
(field_pattern name: (identifier) @property)
(field_expression field: (identifier) @property)
(tuple_index_expression index: (integer_literal) @property)

(variant name: (identifier) @constructor)
(variant_expression name: (identifier) @constructor)

; --- Uses --------------------------------------------------------------------
(call_expression function: (identifier) @function.call)
(call_expression function: (field_expression field: (identifier) @function.method))

(import_specifier name: (identifier) @variable)
(import_specifier alias: (identifier) @variable)
(namespace_import name: (identifier) @namespace)

(test_declaration name: (string_literal) @string.special)
