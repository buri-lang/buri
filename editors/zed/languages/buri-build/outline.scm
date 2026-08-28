; The outline Zed shows in its breadcrumb and symbol picker: one entry per
; block.
(block name: (identifier) @name) @item

; A `tag` block is one of several with the same name, and the `name:` inside it
; is the only thing that tells them apart — so that field is an entry of its
; own, nested under the block by the range it sits in.
((field
   name: (identifier) @context
   value: (string) @name) @item
 (#eq? @context "name"))
