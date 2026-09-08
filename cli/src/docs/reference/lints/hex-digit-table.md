---
title: The hexadecimal digits are not yours to keep
severity: warning
message: this is a table of the sixteen hexadecimal digits
note: "`character.fromDigit(n, 16)` is a digit and `character.toDigit(16)` reads one back, so a table of them is a copy of something the standard library keeps"
fix: delete the table, and reach for `character.fromDigit`, `number.toHex` or `bytes.toHex`
---
A table of digits never arrives on its own. With it come a `nibble` helper, a
shift, a mask, an index, and — sooner or later — a sixteen-call unrolled
renderer for one 64-bit value.

The library keeps the digits once:

- **One digit** — `character.fromDigit(n, radix)` is the character, and
  `character.toDigit(radix)` is its inverse. Both reach base 36, so the same
  pair answers hexadecimal, octal and base 32.
- **Is this one?** — `character.isHexDigit()`, rather than comparing a `toDigit`
  against `.None`.
- **A whole number** — `number.toHex(ctx, x, width)`, lowercase and zero-padded to
  a width you name.
- **A whole byte string** — `bytes.toHex(ctx, b)` and `bytes.fromHex(ctx, s)`.
- **Text back to a number** — `str.toRadix(text, radix)`, which answers `.None`
  rather than a value the `Int` cannot hold.

If the table exists because the *output* has to differ, say that instead of
rebuilding the encoder underneath it. Uppercase is `toUpper` over the result. A
separator is a `join`. A genuinely different alphabet is worth a comment saying
which one it is.

This rule only fires on the sixteen digits in order, as a string or as a run of
character literals.
