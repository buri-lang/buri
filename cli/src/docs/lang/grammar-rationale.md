## 12. Why the grammar is context-free and unambiguous

Each item below is a deliberate design decision, listed with what it costs.

**12.1 `if` and `match` subjects are parenthesized.**
`if (c) { ... }` closes the condition at `)`, so the `{` that follows is always a
block. This is the ambiguity that forces Rust to ban struct literals in condition
position. *Cost:* two characters, and it looks like TypeScript anyway.

**12.2 There are no expression statements.**
A block is `let`s followed by a result expression. Nothing can sit next to a
`{`-initial expression and compete with it. *Cost:* a call performed only for its
effect must be bound: `let _ = io.println(ctx, "hi");`. Arguably a feature.

A **test source** may use one, restricted to calls whose type is `()`
(Section 11.2). The grammar admits `Expr ";"` as a statement, which stays LR(1)
— after an expression, a `;` means statement and a `}` means result — and the
property this rule is protecting is unharmed, since `Result` is not `()`. Every
other module still has `let` as its only statement.

**12.3 There are no records, so `{` at the start of an expression is always a
block.**
Structural records made `{ x }` ambiguous — a record with a shorthand field, or a
block whose result expression is `x` — and an earlier draft paid for that by
banning field shorthand in literals. Removing records (Section 5.5) removed the
ambiguity at its source: a bare `{` opens a block, and a `{` after a path opens a
struct literal. Nothing competes, so `Point { x, y }` shorthand works and the
grammar got smaller rather than more careful. *Cost:* every product type needs a
name.

**12.4 Type arguments in expressions use the turbofish `::<T>`.**
`f<a>(b)` is genuinely ambiguous with two comparisons. Types have no comparison
operators, so `<` is unambiguous inside a type; expressions get `::<`.
*Cost:* the turbofish is ugly. It is also rare.

**12.5 There is no cast operator.**
`as`, `as?`, and `as%` were three tokens and a precedence level doing work that
methods do with none: a conversion is resolved by its receiver's type, which is
the same lookup either way (Section 6.2.1). Removing them also let the fallible
conversions return `Result` instead of encoding failure in the choice of
operator. *Cost:* `core/num` has one method per source-and-target pair.

**12.6 There is no `<<` or `>>` token.**
Longest-match lexing would turn `Map<Str, [Int]>>` into a shift. Removing the
operators removes the problem, rather than papering over it with a token splitter
that makes the lexer position-dependent. *Cost:* `bits.shl(x, n)`.

**12.7 Enum variants in patterns must be qualified or dot-prefixed.**
Otherwise `None` is a binding or a variant depending on what is in scope, which
makes the parser depend on name resolution. A bare `IDENT` in a pattern is always
a binding; `IDENT(` and `IDENT{` are struct patterns; `A.B` is a path. Decided by
one token of lookahead. *Cost:* `.None` instead of `None`.

**12.8 Tuples have arity ≥ 2 and unit is `()`.**
`(e)` is grouping, full stop — no `(e,)` special case, no zero-tuple competing
with the empty record. *Cost:* none worth mentioning.

**12.9 Lambdas begin with `fn`.**
`(x) => ...` requires deciding whether `(x)` is a parenthesized expression or a
parameter list, which is a reduce/reduce conflict at `)`. A leading `fn` decides
it at token one. *Cost:* two characters; it also makes lambdas and function types
read the same.

**12.10 `else` is mandatory and branches are blocks.**
Kills the dangling-else ambiguity, and prevents `if (c) { a } else { b } + 1`
from having two parses. *Cost:* you must say what the other case is.

**12.11 A lambda is a top-level-only expression.**
Its body extends maximally to the right, so allowing one as an operand would
make `2 * fn(x) => x + 1` ambiguous. Block-like expressions (`{}`, `if`, `match`)
*are* allowed as operands, because they are brace-terminated and self-delimiting.
*Cost:* parentheses around a lambda used as an operand.

**12.12 Match arms are comma-separated, always.**
Without a required separator, `A => x` followed by an arm starting `-1 =>` would
greedily parse as `x - 1`. *Cost:* a comma after `}`.

**12.13 Block-like expressions cannot head a postfix chain.**
`match (x) { ... }.field` is a parse error; parenthesize. This is what stops
`if (c) { a } else { b } { x: 1 }` from parsing two ways once struct literals are
a postfix form. *Cost:* occasional parentheses.

**12.14 Float literals must start with a digit.**
So `pair.0` lexes as three tokens. *Cost:* write `0.5`.

**12.15 Every declaration starts with a distinct keyword.**
`from` `export` `fn` `struct` `enum` `type` `const` `opaque`. Top-level parsing
is a switch on one token — and putting `from` first on an import means the module
path is known before the specifier list is parsed, which is what makes
completion inside the braces possible.

**12.16 Method calls reuse `.` rather than taking a token of their own.**
`sq.area()` needs no new production: it is the existing `PostfixExpr "." IDENT`
followed by a call, and name resolution decides whether `area` is a field or a
method. The alternative, `sq:area()`, would visibly separate data from
computation — but `:` after an expression breaks the one-token lookahead that
tells `{ foo: bar }` (record literal) from `{ foo:bar() }` (block whose result is
a method call), which is the very disambiguation that cost record literals their
field shorthand in 12.3. Buying `:` back would mean moving record literals to
`{ x = 1 }`. *Cost:* `.` now carries four meanings — field, tuple index, module
member, method — all resolved after parsing, and a method may not share a name
with a field of the same type.

**12.17 A method is declared by an `impl` block, and `self` is a keyword in a
fixed position.**
"Is this a method?" is answered by where the declaration sits, and "what is the
receiver?" by a keyword rather than by comparing types against a rule about
argument order. Neither question needs name resolution. An `impl` block's two
forms differ by one token of lookahead — `for` after the first type makes it a
conformance declaration, and its absence makes it the type's own methods — and
`derive` and `trait` likewise each begin with a distinct keyword, keeping
top-level parsing a switch on one token.

**12.18 `context` is a keyword, and its two forms differ at one token.**
`context Name { ... }` is a declaration and `context { ... }` is an expression,
told apart by whether an IDENT or a `{` follows — one token of lookahead, no
name resolution. The expression is brace-terminated and self-delimiting, so it
joins `{}`, `if`, and `match` as a block-like operand under 12.11 and 12.13.
Reusing struct-literal syntax (`Ctx { Alloc: ... }`) instead would have needed a
declared type to name, and the whole point is that a context's type is never
written (Section 11.3). *Cost:* one keyword, which no program could have used as
an identifier anyway once contexts existed.

Two lexical warts remain and are accepted:

- `t.0.1` lexes as `t` `.` `0.1`. Write `(t.0).1`.
- `Foo<Bar<Int>>= x` lexes `>` `>=`. Write a space: `Foo<Bar<Int>> = x`.

---
