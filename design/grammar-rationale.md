## Why the grammar is context-free and unambiguous

A maintainer document, no longer part of the specification. Each item below is a
deliberate design decision, with what it cost.

**12.1 `if` and `match` subjects are parenthesized.**
`if (c) { ... }` closes the condition at `)`, so the `{` that follows is always a
block. Rust bans struct literals in condition position to escape the same
ambiguity. *Cost:* two characters, and it looks like TypeScript anyway.

**12.2 There are no expression statements.**
A block is `let`s followed by a result expression, so nothing can sit next to a
`{`-initial expression and compete with it. *Cost:* you have to bind a call you
make only for its effect, and consume its `Result` too:
`let _ = io.println(ctx, "hi").ignore();`.

A **test source** may use one, as long as the expression has type `()` (Section
11.2): a call, and equally a `match`, an `if` or a block that produces `()`. The
grammar admits `Expr ";"` as a statement and stays LR(1), because after an
expression a `;` means statement and a `}` means result. The property this rule
protects survives, since `Result` is not `()`. The `;` is required even after a
`{`-initial expression: a block-like statement allowed to drop it would compete
with the result expression at the one place a block ends, and
`{ match (c) { … } }` would have two readings.

**12.3 There are no records, so a `{` that could open a block does.**
Structural records made `{ x }` ambiguous: a record with a shorthand field, or a
block whose result expression is `x`. Removing records (Section 5.5) killed the
ambiguity at its source, so `Point { x, y }` shorthand works and the grammar got
smaller rather than more careful. *Cost:* every product type needs a name.

A `{` after a path is always a struct literal. A bare `{` opens a block —
`{ Stmt* Expr? }` — unless the two tokens after it are ones no statement and no
expression can begin with, which is a `..` or a `name :`. Then it is an
*anonymous* struct literal, and its type comes from what the expression is
checked against (Section 5.6). That is the whole rule: `{ }`, `{ name }` and
`{ name, ... }` are all blocks, because none of them opens with either.

Shorthand *after* the first field is free, because by then the `{` is settled:
`{ hi: hi, hello }` is a literal. Only the first field is held to `name :`.
A leading shorthand is tempting — `{ name, }` is unambiguous and could be a
literal — but taking it would make two strings a comma apart mean different
things, and the formatter would have to preserve a trailing comma to preserve a
meaning.

So the decision takes two tokens of lookahead with no backtracking, the grammar
stays LR(1), and the generated tree-sitter grammar needs no second declared
conflict. *Cost:* `World {}` and `World { hi }` keep their type name where
`{ hi: "hi" }` does not, and a reader meets that seam before the rule explains
itself.

**12.4 Type arguments in expressions are written `f<T>(x)`, with no `::`.**
`f<a>(b)` and `(f < a) > (b)` are the same tokens. A rule that was already there
for its own sake settles them: comparison is non-associative (Section 6.1), so
`a < b > c` is not a program under *either* reading, and no source exists that
both readings accept and disagree about. The parser looks ahead for the `>` that
would close the list and backtracks if it does not find one; the generated
tree-sitter grammar carries the same decision as its one declared conflict.
*Cost:* the grammar stops being LR(1) at exactly one production, and
`a < b > (c)` is a call. The turbofish `::<T>` is a parse error whose message
carries the edit that removes the `::`.

**12.5 There is no cast operator.**
`as`, `as?`, and `as%` were three tokens and a precedence level doing work that
methods do with none: the receiver's type resolves a conversion, which is the
same lookup either way (Section 6.2.1). Dropping them also let the fallible
conversions return `Result` instead of encoding failure in the choice of
operator. *Cost:* `core/number` carries one method per source-and-target pair.

**12.6 There is no `<<` or `>>` token.**
Longest-match lexing would turn `Map<Str, [Int]>>` into a shift. Dropping the
operators fixes that at the source, rather than papering over it with a token
splitter that makes the lexer position-dependent. *Cost:* `bits.shl(x, n)`.

**12.7 Enum variants in patterns must be qualified or dot-prefixed.**
Otherwise `None` is a binding or a variant depending on what is in scope, and the
parser ends up depending on name resolution. A bare `IDENT` in a pattern is
always a binding; `IDENT(` and `IDENT{` are struct patterns; `A.B` is a path. One
token of lookahead decides it. *Cost:* `.None` instead of `None`.

**12.8 Tuples have arity ≥ 2 and unit is `()`.**
`(e)` is grouping, full stop — no `(e,)` special case, no zero-tuple competing
with the empty record. *Cost:* none worth mentioning.

**12.9 Lambdas begin with `fn`.**
`(x) => ...` forces a choice between a parenthesized expression and a parameter
list, which is a reduce/reduce conflict at `)`. A leading `fn` settles it at
token one. *Cost:* two characters; it also makes lambdas and function types read
the same.

**12.10 `else` is mandatory and branches are blocks.**
Kills the dangling-else ambiguity, and stops `if (c) { a } else { b } + 1` from
having two parses. *Cost:* you must say what the other case is.

**12.11 A lambda is a top-level-only expression.**
Its body extends as far right as it can, so allowing one as an operand would make
`2 * fn(x) => x + 1` ambiguous. Block-like expressions (`{}`, `if`, `match`)
*are* allowed as operands, because braces terminate them and they delimit
themselves. *Cost:* parentheses around a lambda used as an operand.

**12.12 Match arms are comma-separated, always.**
Without a required separator, `A => x` followed by an arm starting `-1 =>` would
greedily parse as `x - 1`. *Cost:* a comma after `}`.

**12.13 Block-like expressions cannot head a postfix chain.**
`match (x) { ... }.field` is a parse error; parenthesize. That stops
`if (c) { a } else { b } { x: 1 }` from parsing two ways once struct literals
become a postfix form. *Cost:* occasional parentheses.

**12.14 Float literals must start with a digit.**
So `pair.0` lexes as three tokens. *Cost:* write `0.5`.

**12.15 Every declaration starts with a distinct keyword.**
`from` `export` `fn` `struct` `enum` `type` `let` `trait` `effect` `context`
`impl` `derive` `test`. Top-level parsing is a switch on one token. Putting
`from` first on an import means the parser knows the module path before it reads
the specifier list, which is what makes completion inside the braces work. `let`
is the one keyword shared with a statement, and the two never compete: a module
holds declarations and a block holds statements.

**12.16 Method calls reuse `.` rather than taking a token of their own.**
`sq.area()` needs no new production: it is the existing `PostfixExpr "." IDENT`
followed by a call, and name resolution decides whether `area` is a field or a
method. The alternative, `sq:area()`, would visibly separate data from
computation — but a `:` after an expression breaks the one-token lookahead that
tells `{ foo: bar }` (record literal) from `{ foo:bar() }` (block whose result is
a method call), the very disambiguation 12.3 is about. *Cost:* `.` now carries
four meanings — field, tuple index, module member, method — all resolved after
parsing, and a method may not share a name with a field of the same type.

**12.17 A method is declared by an `impl` block, and `self` is a keyword in a
fixed position, written without a type.**
Where the declaration sits answers "is this a method?", and a keyword answers
"what is the receiver?" Neither question needs name resolution. Nor does the
receiver's *type*: the `impl` head sits above the declaration and a trait
signature means `Self`, so a `self` annotation could only repeat what is already
written or contradict it. An `impl` block's two forms differ by one token of
lookahead — `for` after the first type makes it a conformance declaration, and
its absence makes it the type's own methods.

**12.18 `context` is a keyword, and its two forms differ at one token.**
`context Name { ... }` is a declaration and `context { ... }` is an expression.
Whether an IDENT or a `{` follows tells them apart: one token of lookahead, no
name resolution. Braces terminate the expression and it delimits itself, so it
joins `{}`, `if`, and `match` as a block-like operand under 12.11 and 12.13.
Reusing struct-literal syntax (`Ctx { Alloc: ... }`) would have needed a declared
type to name, and the whole point is that nobody ever writes a context's type
(Section 11.3). *Cost:* one keyword, which no program could have used as an
identifier anyway once contexts existed.

Two lexical warts remain, and both stay:

- `t.0.1` lexes as `t` `.` `0.1`. Write `(t.0).1`.
- `Foo<Bar<Int>>= x` lexes `>` `>=`. Write a space: `Foo<Bar<Int>> = x`.

---
