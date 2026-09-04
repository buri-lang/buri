## 3. Source text and lexical structure

### 3.1 Encoding

Source files are UTF-8. Identifiers are ASCII in v0.3. String and character
literals may contain any Unicode scalar value.

### 3.2 Whitespace and comments

Buri is not newline-sensitive; there is no automatic semicolon insertion and no
offside rule. Whitespace separates tokens and is otherwise meaningless.

```buri wrap=body
// line comment
/* block comment /* which nests */ still a comment */
/// doc comment; attaches to the declaration that follows
```

A fourth form documents the *file*: a `//!` line, legal above the first item of
a module and nowhere else. `///` attaches downward to the declaration under it
and `//!` attaches upward to the module around it, which is the whole of the
difference — one lower down the file is `module-doc-not-first`, because at that
point it is a mistyped `///` far more often than it is a note about the module.

The third slash is what decides whether prose is **published**. `buri docs
<module>` renders a module's `//!` text and every exported declaration's `///`,
the language server shows the same text on hover, and `buri docs search` reads
it; a `//` reaches none of them, however carefully it is written. A run longer
than three — `////` — is deliberately an ordinary comment again, so a banner
rule publishes nothing.

### 3.3 Identifiers and naming

`IDENT` is `[A-Za-z_][A-Za-z0-9_]*` minus keywords and reserved words.

Naming conventions — **not enforced by the grammar, by design**, since a parser
that depends on capitalization is a parser that depends on convention:

| Kind | Convention | Example |
|---|---|---|
| Types, structs, enums, variants | `UpperCamelCase` | `UserId`, `Some` |
| Functions, parameters, bindings | `lowerCamelCase` | `readConfig`, `ctx` |
| Constants | `SCREAMING_SNAKE_CASE` | `MAX_RETRIES` |
| Modules | `lowercase` | `list`, `str` |

### 3.4 Keywords

`as` `const` `context` `ctx` `derive` `effect` `else` `enum` `export`
`false` `fn` `for` `from` `if` `impl` `import` `let` `match` `self` `Self`
`struct` `test` `trait` `true` `type`

`for` appears only in `impl ... for ...` and `derive ... for ...`. `self` is
legal only as the first parameter of a function inside an `impl` block; `Self`
only inside a trait or `impl`.
`test` is reserved everywhere, so no function may be named `test`, but a `test`
declaration is legal only in a test source (Section 11.2). `context` is likewise
reserved everywhere, but a `context` declaration or expression is legal only
where Section 11.3 says.

`ctx` is legal as the parameter after `self` (Section 10.2), and — because that
is where contexts are built — as a `let` binding name inside `main`'s body, a
test source, or a test-only module. Nowhere else.

`const` is a keyword no production uses. It was how a module-level binding was
spelled before `let` was the only binding keyword, and it stays reserved so that
source which still has it is answered with `const-declaration` — which names
`let` and carries the edit — rather than read as a name and failing later.

`assert` is **not** a keyword; assertions are the ordinary module
`core/testing/assert` (Section 11.2.1).

Reserved for future versions and rejected today: `async` `await` `break`
`continue` `do` `in` `is` `loop` `module` `mut` `opaque` `panic` `pub`
`return` `unreachable` `use` `when` `where` `while` `with` `yield`.

### 3.5 Literals

```buri ignore why="not yet converted to a compiled example: it references names the document never declares, so it needs a preamble before the harness can check it"
42          1_000_000     0xFF     0o755     0b1010_0110      // INT
3.14        1.0e-9        6.02e23                             // FLOAT
"hello"     "tab\there"   "\u{1F600}"                         // STRING -> Str
'a'         '\n'          '\u{41}'                            // CHAR
true        false                                             // BOOL
"n = ${n}"                                                    // TEMPLATE
```

A float literal must begin with a digit. `.5` is not a literal; write `0.5`
(`design/grammar-rationale.md` 12.14).

Underscores are permitted as digit separators anywhere after the first digit.

There are no literal suffixes. A numeric literal takes its type from context and
falls back to `Int` / `Float`; see Section 5.1.1.

### 3.6 String interpolation and `Template`

A string literal containing at least one `${ ... }` hole has type `Template`, not
`Str`. A `Template` is a fixed-size value: a statically known array of literal
fragments plus the evaluated holes. **Constructing a `Template` allocates
nothing**, which is why `io.println(ctx, "hi ${name}")` needs only the `stdout`
effect and can stream directly to the sink.

To turn a `Template` into a `Str` you must allocate:

```buri ignore why="not yet converted to a compiled example: it references names the document never declares, so it needs a preamble before the harness can check it"
let greeting: Str = str.format(ctx, "Hello, ${name}!");
```

A hole expression must have type `Int` (any width), `Float` (any width), `Bool`,
`Char`, `Str`, **or a type whose `Show` is derived** — a `derive Show` type, an
array of one, or a tuple of them, all the way down. Such a hole renders exactly
as `derive Show` renders it, so `"${p}"` and `"${p.show(ctx)}"` produce the same
text.

A derived hole costs the call site nothing. A derived `Show` is a fold over the
type's shape that the run time performs — it is why a derived `x.show(ctx)`
drops its context — so `io.println(ctx, "${point}")` still needs only the
`stdout` effect, and the paragraph above still holds.

A **hand-written** `impl Show` is not admitted in a hole. Its
`show<C: Alloc>(self, ctx: C)` has to be called, and a `Template` names no
context to call it with, so the conversion stays the author's:

```buri ignore why="not yet converted to a compiled example: it references names the document never declares, so it needs a preamble before the harness can check it"
let line: Str = str.format(ctx, "the suit is ${suit.show(ctx)}");
```

The same is true of a bounded type parameter: a `T: Show` may be instantiated at
a type whose `Show` is written by hand, so `"${x}"` in a generic body is
rejected and `"${x.show(ctx)}"` is what is written instead.

`Str` is implicitly widened to `Template` in argument position. This is the only
implicit conversion in the language, and it exists so that `io.println(ctx, "hi")`
and `io.println(ctx, "hi ${name}")` are both well-typed.

Escape `\$` to write a literal dollar sign before a brace.

---
