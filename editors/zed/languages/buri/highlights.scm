; Buri, for tree-sitter. Capture names follow the set Zed and Helix share.
;
; Two patterns may capture the same identifier: a general rule that says what a
; word shaped like this usually is, and a specific rule that says what this one
; is. The specific rule is written later, because that is the one Zed keeps.

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
  "fn" "struct" "enum" "type"
  "trait" "effect" "impl" "derive" "for"
  "let" "test" "context"
] @keyword

["if" "else" "match"] @keyword.control

; `self` and `ctx` are words the language gives a meaning to, in every position
; they are legal: a parameter, a binding, a type, an expression.
[
  (self_expression)
  (ctx_expression)
  (self_type)
  (self_parameter)
] @variable.builtin
(ctx_parameter "ctx" @variable.builtin)
(let_statement "ctx" @variable.builtin)

; --- Operators and punctuation ----------------------------------------------
[
  "||" "&&" "??" "==" "!=" "<" "<=" ">" ">="
  "|" "^" "&" "+" "-" "*" "/" "%" "!" "~" "?" "=" "=>" "@" ".."
] @operator

; `..` in `User { ..u }` and in `[a, ..rest]` is a token of its own rule, so the
; list above does not reach it.
(rest) @operator

["(" ")" "[" "]" "{" "}"] @punctuation.bracket
["," ";" ":" "."] @punctuation.delimiter

; --- What a word's shape says ------------------------------------------------
; A grammar cannot tell a type name from a value name, so these two are the
; naming conventions of `cli/src/docs/lang/lexical.md` read as colour. They
; carry the identifiers no pattern below reaches — a generic argument, a
; qualified path in a pattern, a constant used in an expression — and every
; identifier a pattern below does reach is recoloured by it.
((identifier) @type
  (#match? @type "^[A-Z]"))

; A constant is the one screaming shape. The underscore is required so that a
; generic parameter named `T` stays a type.
((identifier) @constant
  (#match? @constant "^[A-Z][A-Z0-9]*(_[A-Z0-9]+)+$"))

; --- Declarations ------------------------------------------------------------
(function_declaration name: (identifier) @function)

(struct_declaration name: (identifier) @type)
(enum_declaration name: (identifier) @type)
(type_alias_declaration name: (identifier) @type)
(trait_declaration name: (identifier) @type.interface)
(effect_declaration name: (identifier) @type.interface)
(context_declaration name: (identifier) @type)

(let_declaration name: (identifier) @constant)

(generic_parameter name: (identifier) @type.parameter)

; A type path's last segment is the type; the ones before it are the module it
; came from, and colouring them the same makes `effects.Alloc` read as one word.
(named_type (type_path (identifier) @type))
(array_type "[" @punctuation.bracket)

(parameter name: (identifier) @variable.parameter)
(lambda_parameter name: (identifier) @variable.parameter)

(field_declaration name: (identifier) @property)
(variant_field name: (identifier) @property)

(variant name: (identifier) @constructor)

; --- Uses --------------------------------------------------------------------
; A struct literal names a type, and `Stock.OnHand { .. }` names a variant of
; one. The server's own colouring makes the same two answers.
(struct_literal type: (identifier) @type)
(struct_literal type: (field_expression field: (identifier) @constructor))
(struct_literal type: (generic_expression (identifier) @type))

(field_initializer name: (identifier) @property)
(field_shorthand name: (identifier) @property)
(field_expression field: (identifier) @property)
(tuple_index_expression index: (integer_literal) @property)

; A capitalized member is a variant rather than a field: `Stock.Out`.
((field_expression field: (identifier) @constructor)
  (#match? @constructor "^[A-Z]"))

(variant_expression name: (identifier) @constructor)

; The callee, in each of the four shapes it is written in. A capitalized one
; constructs rather than calls — `Meters(9.8)` is a tuple struct's name.
(call_expression function: (identifier) @function.call)
((call_expression function: (identifier) @constructor)
  (#match? @constructor "^[A-Z]"))
(call_expression function: (field_expression field: (identifier) @function.method))
(call_expression function: (generic_expression (identifier) @function.call))
(call_expression
  function: (generic_expression (field_expression field: (identifier) @function.method)))

; The value a functional update copies from: `User { ..u, name: "Ada L." }`.
(spread (identifier) @variable)

; --- Patterns ----------------------------------------------------------------
(bind_pattern name: (identifier) @variable)
(wildcard_pattern) @variable
(rest_pattern name: (identifier) @variable)
(field_pattern name: (identifier) @property)

; `User { .. }`, `Option.Some(x)` and `.Some(x)`, in that order: a bare path is
; the type, a qualified one ends in a variant, and a leading dot is one.
(path_pattern (identifier) @type)
(qualified_path (identifier) @type)
(path_pattern name: (identifier) @constructor)

; --- Imports -----------------------------------------------------------------
; A specifier names whatever the module exported, and its shape is the only
; evidence a grammar has: `Alloc` is a type, `map` is a function.
(import_specifier name: (identifier) @function)
(import_specifier alias: (identifier) @function)
((import_specifier name: (identifier) @type)
  (#match? @type "^[A-Z]"))
((import_specifier alias: (identifier) @type)
  (#match? @type "^[A-Z]"))
(namespace_import name: (identifier) @namespace)

(test_declaration name: (string_literal) @string.special)
