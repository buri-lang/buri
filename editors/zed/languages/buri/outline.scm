; The outline Zed shows in its breadcrumb and symbol picker.

(function_declaration
  "fn" @context
  name: (identifier) @name) @item

(struct_declaration
  "struct" @context
  name: (identifier) @name) @item

(enum_declaration
  "enum" @context
  name: (identifier) @name) @item

(trait_declaration
  "trait" @context
  name: (identifier) @name) @item

(effect_declaration
  "effect" @context
  name: (identifier) @name) @item

(type_alias_declaration
  "type" @context
  name: (identifier) @name) @item

(let_declaration
  "let" @context
  name: (identifier) @name) @item

(context_declaration
  "context" @context
  name: (identifier) @name) @item

(test_declaration
  "test" @context
  name: (string_literal) @name) @item

; An `impl` block holds the methods the outline already lists, and without an
; entry of its own they hang under whatever came before it. The `for` half is
; optional because an inherent `impl` has none.
(impl_declaration
  "impl" @context
  type: (_) @name
  ("for" @context
   trait: (_) @name)?) @item
