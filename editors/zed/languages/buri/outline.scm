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

(const_declaration
  "const" @context
  name: (identifier) @name) @item

(context_declaration
  "context" @context
  name: (identifier) @name) @item

(test_declaration
  "test" @context
  name: (string_literal) @name) @item
