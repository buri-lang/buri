; Buri build files, for tree-sitter. Capture names follow the set Zed and
; Helix share.

(comment) @comment

; --- Names -------------------------------------------------------------------
; A block names a message in the schema and a field names one of its scalars,
; so the two are coloured apart: `library` is a kind of thing, `sources` is a
; property of one.
(block name: (identifier) @type)
(field name: (identifier) @property)

; --- Values ------------------------------------------------------------------
(string) @string
(number) @number
; A bare word is an enum constant or a bool. The reader spells both the same
; way, and both are constants the schema names.
(constant) @constant

; --- Punctuation -------------------------------------------------------------
["{" "}" "[" "]"] @punctuation.bracket
[":" ","] @punctuation.delimiter
