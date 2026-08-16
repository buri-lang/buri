; Everything brace- or bracket-delimited indents its contents by one level.
; Buri is not newline-sensitive, so there is nothing subtler to say.

[
  (block)
  (field_declarations_block)
  (tuple_fields_block)
  (context_body)
  (struct_literal_body)
  (array_expression)
  (arguments)
  (parameters)
  (match_expression)
  (enum_declaration)
  (trait_declaration)
  (effect_declaration)
  (impl_declaration)
] @indent

["}" ")" "]"] @outdent
