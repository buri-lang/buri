# Editing a diagnostic page

Every code the toolchain emits has one page: `cli/src/docs/errors/<code>.md` for
a compiler or build diagnostic, `cli/src/docs/lints/<code>.md` for a `buri lint`
finding. The `---` block at the top is the wording the diagnostic prints. The
markdown below it is the explanation that prints under the diagnostic the first
time the code comes up in a run.

## The frontmatter

```yaml
---
title: Type arguments qualify a function, not a value
severity: warning
message: explicit type arguments qualify a function or a call
label: this is not a function
note: comparison operators do not chain
fix: attach the type arguments to the call, as in `{function}<Str>(x)`
reproduction: none
adapted-from: some-guide (https://example.invalid/some-guide) guides/the-file.md, © 2026 An Author, used under the MIT license
---
```

| Key | Required | What it is |
|---|---|---|
| `title` | yes | The line the docs index shows. **Never printed in a diagnostic.** A declarative sentence stating the rule — "A name is declared once", not "Duplicate declaration". |
| `severity` | no, defaults to `error` | `error` or `warning`, matching what the emission site built. Every page under `lints/` writes `warning`, because a page omitting it would silently default to the wrong one. |
| `message` | yes | The sentence after `error: ` / `warning: `. |
| `label` | no | The phrase printed beside the carets, under the `^^^` span. |
| `note` | no | One `= note:` line of background — *why* the rule exists, not what to do. A call site may push further notes, and they land after this one. |
| `fix` | no in the schema | The concrete edit, printed as `= fix:`. The reject corpus requires every compiler diagnostic to carry one, so a page omits it only when every emission site sets a `fix` of its own. |
| `reproduction` | no | The only value is `none`, and it means no single-file program can provoke this code — it needs a repository, a `BUILD.buri`, a `.proto`, a second module, or a process that runs too long. Any other page must carry a fenced `buri fail code=<code>` block. |
| `adapted-from` | no | Where a body adapted from somebody else's writing came from — source, file and licence. **Never printed in a diagnostic**; `buri docs error <code>` and `buri docs lint <code>` render it as the page's last line. |

Everything is a scalar: no lists, no maps. An unknown key is an error, so a
misspelled `mesage` fails the build instead of printing nothing.

A value is bare by default, and backticks and colons are ordinary characters in
one. Quote a value only when it would otherwise lose its edges: a leading
backtick, brace or quote, or leading and trailing whitespace that matters.
`'single'` quotes are literal.
`"double"` quotes take `\n`, `\t`, `\"` and `\\`, and reject any other escape.
A full-line `#` is a comment. Nothing may be indented, and a value spans one
line.

A page that will not parse stays out of the catalog rather than being half-read,
so the diagnostic prints without its wording and the tests fail with the page
and the line named.

## Templating

`{placeholder}` is the one piece of templating.

- **Names are `snake_case`, and spelled out.** `{function}`, never `{fn}` or
  `{fnName}`. A test enforces the shape.
- **`{{` and `}}` are the literal braces.** A fix that shows
  `` `impl Eq for ... { ... }` `` writes `{{ ... }}`, or the template would read
  `{ ... }` as a placeholder named `" ... "`.
- **There are no filters, no conditionals, and no pluralization.** The call site
  binds whole finished phrases. Where the wording varies by more than an
  interpolated name, the site sets that field itself after its last `bind`, and
  the page leaves it out.
- **Placeholders are allowed in `message`, `label`, `note` and `fix` only.**
  `title` and the body are static.
- **One code is one message.** Two emission sites that need genuinely different
  sentences are two rules and want two codes. Two sites whose sentences differ
  only in an interpolated noun are one code with a placeholder.

The emission site looks like this:

```rust
self.templated("type-args-on-a-value", span).bind("function", name);
// or, off a Diagnostic of your own:
Diagnostic::templated(code, span).with_bind("function", name)
```

`bind` re-renders the message, label, note and fix from the page every time, so
a `fix` a call site sets *before* a `bind` is erased. Set it after.

Three mistakes panic in a debug build — which is every test run — and in release
degrade to printing the template as written: a code with no page, a placeholder
nothing bound, and a binding no template uses. So renaming a placeholder is a
two-file edit: the page, and every site that binds it.

## How the body prints

Below the frontmatter, the page is freeform markdown. The toolchain prints it
under the diagnostic, wrapped to the terminal width, indented to the `= fix:`
column, and dimmed when colour is on. It never repeats what the frontmatter
already says: the `note` and the `fix` print immediately above it.

- **Once per code per run.** The set is process-wide, so a command that opens
  several sessions still owes the reader one copy.
- **`--dense` suppresses it**, on `build`, `test`, `lint`, `run` and `docs`.
- **`--error-format=json` never carries it.**
- **Four things are dropped before printing**: the `#` title, the fenced `text`
  specimen of the diagnostic itself, the `buri fail code=…` reproduction, and
  any heading those leave empty. A page that is frontmatter plus a reproduction
  therefore prints nothing at all.
- **Inline markdown is flattened.** `` `x` `` prints as `x` and `*x*` prints as
  `*x*` — the asterisks survive, so prefer backticks. A markdown numbered list
  renders as running prose.
- Write the body about the *rule*, not about the page's own example. The body is
  static and the diagnostic above it is not, so a body naming a concrete type
  will sooner or later sit under a diagnostic about a different one.

## How this is enforced

`documentation/errors.rs` and `documentation/lints.rs` carry the same tests:
every page parses, opens a `---` block, has a non-empty `title` and `message`,
appears once, carries a `code=<code>` reproduction unless it says
`reproduction: none`, spells every `{placeholder}` in `snake_case`, and points
every `see_also` at a real topic. Across the tree, every code a Rust source
attaches has a page, every reproduction still produces the code its page is
named after, and every rejected program's JSON diagnostic carries a `fix`.

The golden corpora record rendered output: `cli/tests/reject/*/expected.{txt,json}`,
and `expected/*.{txt,json}` under `cli/tests/repositories/`, `cli/tests/failing/`
and `cli/tests/cli/`. Editing a `message`, `label`, `note` or `fix` changes them.

```
BURI_BLESS=1 cargo test -p buri
```

regenerates every one. Read the diff before you keep it. `expected.json` should
move only when a code was split or renamed, or when the data bound to a
placeholder changed. A `.txt` that moves with no `.json` beside it is a body
edit, which is expected. A `.txt` whose `error:` line changed is a rewording,
and it should be one you meant.

## The placeholder vocabulary

Every `{placeholder}` on every page, with what it holds. Values are strings the
call site has already finished. It does the quoting, the pluralization and the
joining; the template supplies the backticks.

| Placeholder | What it holds |
|---|---|
| `{arity}` | The number of elements a tuple or a tuple pattern has. Always the *N* in `{arity}-tuple`. |
| `{artifact}` | What a non-native platform produces, as the fix's subject: `JavaScript`, `a page`. |
| `{because}` | The clause saying why a platform withholds a `core/host` grant. |
| `{block}` | Where an unknown build-file field was written, already described: `` a `binary` rule ``, `` a `tag` block ``, `REPO.buri`. |
| `{candidates}` | The schemas that could claim an ambiguous proto type name, sorted and joined with `, or `. |
| `{character}` | The character the lexer could not start a token with, as the source wrote it. |
| `{choices}` | The finished list of the bare words a build-file field accepts. |
| `{code}` | The code a generator asked to report under, which this toolchain has no page for. |
| `{code_point}` | That character's scalar value, in the lexer's own `{:04X}` form. |
| `{construct}` | The proto construct the reader refuses, by name (`service`). |
| `{container}` | `` a `Result` `` or `` an `Option` ``. Used twice in the one sentence. |
| `{count}` | How many of a thing were declared — a tuple struct's fields, a function's parameters. |
| `{cycle}` | The import stack from the first repeat onwards, already joined with ` -> `. |
| `{declaration}` | On the language pages, the whole noun phrase for the thing declared or hidden (`` field `a` ``, `` variant `Yes` of `T` ``). On the proto pages, what the file declared, already described (`` `syntax = "proto3"` ``). See the note below the table. |
| `{dependency}` | The label of the library in question (`//lib/store`). |
| `{depth}` | How many branch bodies enclose the reported one, itself included. |
| `{edition}` | `REQUIRED_EDITION` — the one Protobuf edition this reader implements. |
| `{effect}` | The effect's name. |
| `{escape}` | The one character after a backslash that is not an escape. |
| `{expected}` | What the declaration, the grammar or the schema says: a rendered type, a decimal count, or a finished noun phrase (`` `;` ``, `a block`, `platform names`). |
| `{expected_plural}` | The plural of what a bare word should have been (`platforms`, `architectures`), because the fix names the whole set. |
| `{exports}` | The names a test's import asked for, quoted and joined (`` `a`, `b` ``), or the phrase `what the test needs`. |
| `{feature}` | The `features.<name>` a schema wrote. |
| `{field}` | A field's name: a build-file field as the schema spells it (`sources`, `arch`), a struct field as the source wrote it, or the `sources`/`proto_sources` a file belongs under. |
| `{field_type}` | The type of the field that blocks a derive. |
| `{fields}` | The `diagnostics::names` enumeration of the fields with no value, or with no pattern. |
| `{first_origin}` | The first of the two schemas that declare one proto type. |
| `{first_tag}` | The first of the two tags that forbid each other. |
| `{first_trait}` | The first of the two bounds declaring one method name, in the order the search met them — the fix quotes it. |
| `{found}` | What is there instead of `{expected}`. |
| `{from}` | The error type `?` would propagate. |
| `{from_target}` | The label of the package that depends. |
| `{function}` | The called function's name, or the name written to the left of the type arguments. |
| `{given}` | How many were given, where `{expected}` is how many are taken. |
| `{importer_file}` | The importing module's own file, so the note can say which rule it belongs to. |
| `{index}` | The tuple element, or the positional field, that was asked for. |
| `{known_features}` | The proto features this reader models, joined with `, `. |
| `{known_fields}` | The fields a build-file block accepts, joined with `, `. |
| `{last}` | The highest legal tuple index, which the fix names. |
| `{library_file}` | The path of a library's `lib.buri`, which is where the re-export would go. |
| `{limit}` | The fixed lint threshold, bound from the constant in `lint.rs` so the number exists in one place. |
| `{lines}` | How many lines a function body spans, opening brace to closing brace inclusive. |
| `{literal}` | The literal exactly as the source wrote it — prefix, underscores and sign included. |
| `{marker}` | Which marker a comment carries: `TODO`, `FIXME` or `HACK`. |
| `{matched}` | How many values a pattern matched, where `{expected}` is how many the variant holds. |
| `{message}` | A whole finished sentence somebody else wrote — today, a generator's own. |
| `{method}` | The method looked up in, or supplied to, a type or a trait. |
| `{methods}` | The `diagnostics::names` enumeration of the methods an `impl` is missing. |
| `{module}` | The module path two `import` statements both name, unquoted — the template supplies the backticks. |
| `{module_file}` | The colliding module's file, from the repository root (`lib/money/cents.buri`). |
| `{name}` | The identifier the diagnostic is about, where no narrower role name applies. See the note below the table. |
| `{operations}` | The intrinsic operations a toolchain cannot compile, quoted and joined by `diagnostics::names`. |
| `{output}` | A declared output, spelled the way a build file and `--output` spell it — `linux/x86_64`, or `macos` where the output named no architecture. **Not a target triple**: a triple carries the host's own architecture whenever the output named none, so a recorded diagnostic holding one pins the runner rather than the product. |
| `{operator}` | The operator's source text (`~`, `<<`, `Add`, `Neg`). |
| `{other}` | The label at the far end of the reported dependency edge. |
| `{owner}` | The label of the library whose surface or internals are being reached (`//lib/money`). |
| `{owner_path}` | That label with `//` stripped, because the note names `lib/money/lib.buri`. |
| `{package}` | The label of the package the build-graph rule is reported against. |
| `{package_path}` | A package's path from the repository root, with no leading `//` — every use already prefixes it. |
| `{parent_package}` | The package that holds the colliding module. |
| `{path}` | The module path an import wrote, the schema path an `import` line spells, or the labels a generator's tool reaches its own target through, joined with ` -> `. |
| `{platform}` | The platform, spelled as the sentence wants it — `Platform::slug()` (`js`, `linux`) in prose, `Platform::proto()` (`JS`) where the sentence quotes a build file. |
| `{platform_in_build_file}` | `Platform::proto()` — the spelling `test.platforms` uses (`JS`, `LINUX`). Two placeholders rather than one because the sentence and the build file disagree about case. |
| `{platforms}` | The platforms a host effect is *not* allowed on, named inside the sentence — `Platform::sentence_phrase`, which writes the article and the plural (`the WEB platform`, `the MACOS and JS platforms`). |
| `{position}` | Where `self` may appear, as a phrase: `a function's first parameter`, `the first parameter`. |
| `{problem}` | The whole message sentence, supplied by the call site. See “When a page binds its whole sentence”. |
| `{profile}` | The build profile the invocation asked for — `BuildMode::name()`, `debug` or `release`. The two have different requirements, so the sentence about a missing one has to say which was asked for. |
| `{quoted_title}` | A test's title **with its quotes**, bound as `format!("{name:?}")`, so a title holding a quote or a backslash escapes exactly as it did. |
| `{radix}` | The base an integer prefix names, as a decimal number (`2`, `8`, `16`). |
| `{reached}` | Which way reachability went, as a finished phrase: `` both `lib.buri` and `main.buri` `` or `` neither `lib.buri` nor `main.buri` ``. |
| `{reaches}` | How the use was found: `imports` when an import names the library, `uses` when only method resolution reaches it. |
| `{reason}` | Why a proto construct is refused, or which of `build/actions.rs`'s three native gaps was hit (`NativeGap::reason`). |
| `{fix}` | The whole fix sentence, supplied by the call site, where the fix depends on which of several causes the diagnostic found (`NativeGap::fix`). |
| `{remedy}` | The whole fix sentence, supplied by the call site. |
| `{requirement}` | One of `main`'s three requirements, as a phrase (`takes no parameters`). |
| `{roots}` | `standard_library::roots_phrase()` — the reserved module roots as a finished phrase. |
| `{rule}` | The rule kind that owns the empty `test` block: `library` or `binary`. |
| `{second_origin}` | The second of the two schemas that declare one proto type. |
| `{second_tag}` | The second of the two tags that forbid each other. |
| `{second_trait}` | The second of the two bounds declaring one method name. |
| `{seconds}` | A suite's declared `timeout_seconds`, or `0` when it declares none. Bound as a string; the `s` suffix is in the page. |
| `{source}` | A source file as the rule, or the directory walk, spells it — relative to its package. |
| `{tag}` | A tag's name as `REPO.buri` or a `tags` list wrote it. |
| `{target}` | The label of the target the rule is reported against. |
| `{test_source}` | The importing test source's file. |
| `{to}` | The error type the function returns. |
| `{to_package_path}` | The dependency's package path, for the `BUILD.buri` to edit. |
| `{to_target}` | The label of the dependency that is not visible. |
| `{tool}` | A `generators` entry's `tool`, exactly as the build file wrote it. |
| `{trait}` | The trait, or the effect, the diagnostic is about — without backticks, which the templates carry. |
| `{type}` | The rendered type, without backticks. |
| `{user}` | Whoever needs the dependency: the importing file's path at the import site, the package's label at the resolution site. |
| `{value}` | What the file wrote where a closed set of words, or a known feature value, was expected. |
| `{variant}` | The variant name after the dot, without the dot. |
| `{visible_to}` | `Workspace::visibility_list` — the finished list of who the dependency *is* visible to. |
| `{witness}` | The rendered uncovered pattern. |
| `{word}` | The reserved word as written. |
| `{wrapped}` | The rendered type of the newtype's single field, which is not a number. |
`{name}` is the fallback, for an identifier where no narrower word fits. Where
the thing has a role in the sentence, the vocabulary names it — `{function}`,
`{method}`, `{trait}`, `{type}`, `{variant}`, `{field}`, `{effect}`, `{tag}`,
`{module}`. Both mean "an identifier, already unbackticked", so move a page from
one to the other whenever its sentence reads better for it.

Three families stay parallel rather than unified, because their sentences are:
`{expected}` / `{found}`, `{expected}` / `{given}`, and `{expected}` /
`{matched}`.

`{arity}` is always the *N* in `{arity}-tuple`; `{count}` is a number in prose.
`{package_path}`, `{owner_path}` and `{to_package_path}` are paths with no
leading `//`; `{package}`, `{owner}`, `{target}`, `{dependency}`, `{from_target}`
and `{to_target}` are labels that keep it.

`{declaration}` carries two meanings: the noun phrase for a thing declared on
`duplicate-declaration` and `private-to-module`, and the text of a schema's own
declaration on `proto-edition` and `proto-syntax-declaration`. The two never
meet on one page.

A handful of pages are `message: {problem}`, each a helper whose dozen callers
all state the same rule. They are the exception, not a pattern to copy: a page
whose message is a placeholder is a page with nothing on it to edit.

## Adding a code

1. Write `cli/src/docs/errors/<code>.md` — frontmatter, then the
   fenced `buri fail code=<code>` reproduction, or `reproduction: none`.
2. Register it in `cli/src/documentation/errors.rs` (or `lints.rs`), in the
   sorted list, with the page's title repeated verbatim and a `see_also` only
   where a chapter sets the rule out at length.
3. Emit it with `Diagnostic::templated`, binding exactly the placeholders the
   page uses.
4. `BURI_BLESS=1 cargo test -p buri`, and read the diff.
