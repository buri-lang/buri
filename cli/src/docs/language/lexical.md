## 3. Source text and lexical structure

### 3.1 Encoding

Source files are UTF-8. Identifiers are ASCII in v0.3. String and character
literals may contain any Unicode scalar value.

### 3.2 Whitespace and comments

Whitespace only separates tokens. Buri has no significant newlines, automatic
semicolons, or offside rule.

```buri wrap=body
// line comment
/* block comment /* which nests */ still a comment */
/// doc comment; attaches to the declaration that follows
```

`//!` documents the module itself and is legal only above its first item;
anywhere else it's `module-doc-not-first`.

`//!` text and the `///` on exported declarations are **published**:
`buri docs <module>` renders them, the language server shows them on hover, and
`buri docs search` reads them. `//` and `////` are ordinary comments.

### 3.3 Identifiers and naming

`IDENT` is `[A-Za-z_][A-Za-z0-9_]*` minus keywords and reserved words.

Naming conventions, **not enforced by the grammar**:

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

- `for` appears only in `impl ... for ...` and `derive ... for ...`.
- `self` is legal only as the first parameter of a function in an `impl` block;
  `Self` only inside a trait or `impl`.
- `test` and `context` are never names. A `test` declaration is legal only in
  a test source (Section 11.2), and `context` only where Section 11.3 says.
- `ctx` is legal as the parameter after `self` (Section 10.2), and as a `let`
  binding inside an entry's body, a test source, or a test-only module, where you
  build contexts.
- `const` is used by no production. Writing one gets `const-declaration`, whose fix
  rewrites it to `let`.
- `assert` is **not** a keyword; it's the module `core/testing/assert`
  (Section 11.2.1).

Reserved for future versions and rejected today: `async` `await` `break`
`continue` `do` `in` `is` `loop` `module` `mut` `opaque` `panic` `pub`
`return` `unreachable` `use` `when` `where` `while` `with` `yield`.

### 3.5 Literals

```buri wrap=body
let n = 3;
let ints = [42, 1_000_000, 0xFF, 0o755, 0b1010_0110];
let floats = [3.14, 1.0e-9, 6.02e23];
let strs = ["hello", "tab\there"];
let chars = ['a', '\n'];
let bools = [true, false];
let template = "n = ${n}";
```

A float literal must begin with a digit. `.5` is not a literal; write `0.5`
(`design/grammar-rationale.md` 12.14).

Escapes:

```buri ignore why="a table of spellings rather than a program: every line is a bare literal, which no body may hold as a statement"
"\n"  "\r"  "\t"  "\0"  "\\"  "\""  "\$"   // the ones with names
'\n'  '\r'  '\t'  '\0'  '\\'  '\''         // the same, in a character literal
"\u{1F600}"   '\u{41}'                     // any scalar value, by code point
```

`buri format` writes every C0 control and `DEL` as an escape (by name where it
has one, otherwise `\u{...}`) and every other scalar as itself, so `"\u{41}"`
becomes `"A"`.

Underscores may separate digits anywhere after the first digit.

There are no literal suffixes. A numeric literal takes its type from context,
falling back to `Int` / `Float` (Section 5.1.1).

### 3.6 String interpolation and `Template`

A string literal with a `${ ... }` hole is a `Template`, not a `Str`: fixed
literal fragments plus the evaluated holes. **Building one allocates nothing**, so
`io.println(ctx, "hi ${name}")` needs only the `stdout` effect. Turning it into a
`Str` allocates:

```buri
# from "core/str" import * as str;
# from "platform/effect" import { Allocator };
#
fn greet<C: Allocator>(ctx: C, name: Str): Str {
    str.format(ctx, "Hello, ${name}!")
}
```

A hole has type `Int` or `Float` (any width), `Bool`, `Char`, `Str`, **or a type
whose `Show` is derived**: a `derive Show` type, or arrays and tuples of them, all
the way down. `"${p}"` renders the same text as `"${p.show(ctx)}"`, and costs no
context: `io.println(ctx, "${point}")` still needs only `stdout`.

A hole won't take a **hand-written** `impl Show`, because a `Template` has no
context to pass its `show<C: Allocator>(self, ctx: C)`. Call it yourself:

```buri
# from "core/str" import * as str;
# from "platform/effect" import { Allocator };
#
# enum Suit {
#     Hearts,
#     Spades,
# }
#
# impl Show for Suit {
#     fn show<C: Allocator>(self, ctx: C): Str {
#         match (self) {
#             .Hearts => "hearts",
#             .Spades => "spades",
#         }
#     }
# }
#
fn describe<C: Allocator>(ctx: C, suit: Suit): Str {
    str.format(ctx, "the suit is ${suit.show(ctx)}")
}
```

The same goes for `T: Show` in a generic body, since `T` might have a
hand-written `Show`: write `"${x.show(ctx)}"`, not `"${x}"`.

In argument position a `Str` widens to a `Template`, so `io.println(ctx, "hi")`
type-checks. It's the language's only implicit conversion.

Write `\$` for a literal dollar sign before a brace.

---
