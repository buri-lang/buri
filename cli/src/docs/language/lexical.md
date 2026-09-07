## 3. Source text and lexical structure

### 3.1 Encoding

Source files are UTF-8. Identifiers are ASCII in v0.3. String and character
literals may contain any Unicode scalar value.

### 3.2 Whitespace and comments

Buri is not newline-sensitive. There is no automatic semicolon insertion and no
offside rule. Whitespace separates tokens and means nothing else.

```buri wrap=body
// line comment
/* block comment /* which nests */ still a comment */
/// doc comment; attaches to the declaration that follows
```

A fourth form documents the *file*: a `//!` line, legal above the first item of a
module and nowhere else. `///` attaches down to the declaration under it, `//!`
up to the module around it. A `//!` further down the file is
`module-doc-not-first`.

The third slash decides whether prose is **published**. `buri docs <module>`
renders a module's `//!` text and every exported declaration's `///`, the
language server shows it on hover, and `buri docs search` reads it. A `//`
reaches none of them, and four slashes — `////` — is an ordinary comment again.

### 3.3 Identifiers and naming

`IDENT` is `[A-Za-z_][A-Za-z0-9_]*` minus keywords and reserved words.

The conventions below are **not enforced by the grammar**.

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
only inside a trait or `impl`. `test` and `context` are reserved everywhere, so
no function may be named either, but a `test` declaration is legal only in a test
source (Section 11.2) and a `context` declaration or expression only where
Section 11.3 says.

`ctx` is legal as the parameter after `self` (Section 10.2). It is also legal as
a `let` binding name inside an entry's body, a test source, or a test-only module,
because that is where you build contexts. Nowhere else.

`const` is a keyword no production uses. It stays reserved so that source still
carrying one gets `const-declaration`, which names `let` and carries the edit,
rather than reading as a name and failing later.

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

The escapes a string or character literal takes:

```buri ignore why="a table of spellings rather than a program: every line is a bare literal, which no body may hold as a statement"
"\n"  "\r"  "\t"  "\0"  "\\"  "\""  "\$"   // the ones with names
'\n'  '\r'  '\t'  '\0'  '\\'  '\''         // the same, in a character literal
"\u{1F600}"   '\u{41}'                     // any scalar value, by code point
```

`buri format` prints a literal back as the escapes it denotes: every C0 control
and `DEL` as an escape — the named ones by name, the rest as `\u{...}` — and
every other scalar as itself. So a literal never comes back holding a raw
control character, and an escape you wrote for a printable character normalises
to that character: `"\u{41}"` is reprinted as `"A"`.

Underscores are permitted as digit separators anywhere after the first digit.

There are no literal suffixes. A numeric literal takes its type from context and
falls back to `Int` / `Float`; see Section 5.1.1.

### 3.6 String interpolation and `Template`

A string literal containing at least one `${ ... }` hole has type `Template`, not
`Str`. A `Template` is a fixed-size value: a statically known array of literal
fragments plus the evaluated holes. **Constructing one allocates nothing**, so
`io.println(ctx, "hi ${name}")` needs only the `stdout` effect.

To turn a `Template` into a `Str` you must allocate:

```buri ignore why="not yet converted to a compiled example: it references names the document never declares, so it needs a preamble before the harness can check it"
let greeting: Str = str.format(ctx, "Hello, ${name}!");
```

A hole expression must have type `Int` (any width), `Float` (any width), `Bool`,
`Char`, `Str`, **or a type whose `Show` is derived** — a `derive Show` type, an
array of one, or a tuple of them, all the way down. Such a hole renders as
`derive Show` renders it, so `"${p}"` and `"${p.show(ctx)}"` produce the same
text.

A derived hole costs the call site nothing: a derived `Show` is a fold the run
time performs, so a derived `x.show(ctx)` drops its context and
`io.println(ctx, "${point}")` still needs only the `stdout` effect.

A hole will not take a **hand-written** `impl Show`. A `Template` names no
context to call its `show<C: Alloc>(self, ctx: C)` with, so write the conversion
yourself:

```buri ignore why="not yet converted to a compiled example: it references names the document never declares, so it needs a preamble before the harness can check it"
let line: Str = str.format(ctx, "the suit is ${suit.show(ctx)}");
```

A bounded type parameter works the same way: a `T: Show` may be instantiated at a
type whose `Show` is hand-written, so the compiler rejects `"${x}"` in a generic
body. Write `"${x.show(ctx)}"`.

In argument position, a `Str` widens to a `Template`. This is the only implicit
conversion in the language, and it is what makes `io.println(ctx, "hi")` and
`io.println(ctx, "hi ${name}")` both well-typed.

Escape `\$` to write a literal dollar sign before a brace.

---
