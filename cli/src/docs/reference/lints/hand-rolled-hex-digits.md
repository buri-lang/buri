---
title: The standard library keeps the hex digits
severity: warning
message: this is a table of the sixteen hexadecimal digits
note: "`character.fromDigit(n, 16)` is a digit and `character.toDigit(16)` reads one back, so a table of them is a copy of something the standard library keeps"
fix: delete the table, and reach for `character.fromDigit`, `number.toHex` or `bytes.toHex`
---
A digit table brings a `nibble` helper, shifts, masks and eventually a
sixteen-call unrolled renderer with it. The library already has:

- **One digit**: `character.fromDigit(n, radix)` and its inverse
  `character.toDigit(radix)`, up to base 36.
- **Is this one?**: `character.isHexDigit()`.
- **A whole number**: `number.toHex(ctx, x, width)`, lowercase and zero-padded.
- **A byte string**: `bytes.toHex(ctx, b)` and `bytes.fromHex(ctx, s)`.
- **Text to a number**: `str.toRadix(text, radix)`, which answers `.None`
  rather than overflow an `Int`.

If the output needs to differ, adjust the result: `toUpper` for uppercase,
`join` for a separator. A genuinely different alphabet deserves a comment naming
it.

The rule only fires on the sixteen digits in order, as a string or as a run of
character literals.
