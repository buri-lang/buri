# Editing a diagnostic page

Every code the toolchain emits has one page: `cli/src/docs/errors/<code>.md` for
a compiler or build diagnostic, `cli/src/docs/lints/<code>.md` for a `buri lint`
finding. The page is the whole of what a reader sees. The `---` block at the top
is the wording the diagnostic prints; the markdown below it is the explanation
that prints under the diagnostic the first time the code comes up in a run.

This file is a reference for editing those pages by hand. Nothing reads it: the
catalogs pull each page in by name (`include_str!` per registered code), the
tests walk the registry rather than the directory, and `buri docs examples`
compiles only `buri` and `textproto` fences, of which this file has none. It is
inert, and it sits here because here is where somebody editing a page is looking.

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
| `severity` | no, defaults to `error` | `error` or `warning`. It must match what the emission site used to build. Every page under `lints/` writes `warning` — the catalogue has one severity, and whether a finding fails a run is `REPO.buri`'s question rather than the page's — so on a lint page this key is the one line that is the same on all of them, and a page omitting it would silently default to the wrong one. |
| `message` | yes | The sentence after `error: ` / `warning: `. |
| `label` | no | The phrase printed beside the carets, under the `^^^` span. |
| `note` | no | One `= note:` line of background — *why* the rule exists, not what to do. A call site may push further notes, and they land after this one. |
| `fix` | no in the schema | The concrete edit, printed as `= fix:`. The reject corpus requires every compiler diagnostic to carry one, so a page omits it only when every emission site sets a `fix` of its own. |
| `reproduction` | no | The only value is `none`, and it means no single-file program can provoke this code — it needs a repository, a `BUILD.buri`, a `.proto`, a second module, or a process that runs too long. Any other page must carry a fenced `buri fail code=<code>` block. |
| `adapted-from` | no | Where a body adapted from somebody else's writing came from — source, file and licence. **Never printed in a diagnostic**: a reader looking at their own compile error is owed the explanation, not this repository's paperwork. `buri docs error <code>` and `buri docs lint <code>` render it as the page's last line instead. |

Everything is a scalar. There are no lists and no maps, and an unknown key is an
error rather than something quietly ignored — a misspelled `mesage` fails the
build instead of printing nothing.

A value is bare by default. Backticks and colons are ordinary characters in a
bare value, because nearly every message has both. Quote a value only when it
would otherwise lose its edges — a leading backtick, brace or quote, or leading
and trailing whitespace that matters. `'single'` quotes are literal;
`"double"` quotes take `\n`, `\t`, `\"` and `\\` and reject any other escape.
A full-line `#` is a comment. Nothing may be indented: a value spans one line.

Every error is reported with its line number, and a page that will not parse is
kept out of the catalog rather than half-read — the diagnostic prints without
its wording, and the tests fail with the page named.

## Templating

`{placeholder}` is the one piece of templating.

* **Names are `snake_case`, and spelled out.** `{function}`, never `{fn}` or
  `{fnName}`. A test enforces the shape; the project's no-abbreviations rule
  covers the rest.
* **`{{` and `}}` are the literal braces.** A fix that shows
  `` `impl Eq for ... { ... }` `` writes `{{ ... }}`, or the template would read
  `{ ... }` as a placeholder named `" ... "` and the snake-case test would
  reject it.
* **There are no filters, no conditionals, and no pluralization.** The call site
  binds whole finished phrases. Where the wording varies by more than an
  interpolated name — a fix that reads one way with a near-miss suggestion and
  another way without — the site sets that field itself after its last `bind`,
  and the page leaves it out.
* **Placeholders are allowed in `message`, `label`, `note` and `fix` only.**
  `title` and the body are static.
* **One code is one message.** Two emission sites that need genuinely different
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

Three mistakes panic in a debug build — which is every test run — and degrade in
release to printing the template as written: a code with no page, a placeholder
nothing bound, and a binding no template uses. Each names the code and the
placeholder. Renaming a placeholder is therefore a two-file edit: the page and
every site that binds it.

## How the body prints

Below the frontmatter, the page is freeform markdown, and it is printed under
the diagnostic — wrapped to the terminal width, indented to the `= fix:` column,
and dimmed when colour is on.

* **Once per code per run.** The second `type-mismatch` in a build prints the
  diagnostic and no body. The set is process-wide, so a command that opens
  several sessions still owes the reader one copy.
* **`--dense` suppresses it**, on `build`, `test`, `lint`, `run` and `docs`.
* **`--error-format=json` never carries it.** The JSON is byte-identical to what
  it was before pages held the wording.
* **Four things are dropped before printing**: the `#` title (it is a copy of the
  frontmatter title, which is never printed), the fenced `text` specimen of
  the diagnostic itself (the reader is looking at the real one), the
  `buri fail code=…` reproduction, and any heading those leave empty. A page
  that is frontmatter plus a reproduction therefore prints nothing at all.
* **Inline markdown is flattened.** `` `x` `` prints as `x` and `*x*` prints as
  `*x*` — the asterisks survive, so prefer backticks. A markdown numbered list
  renders as running prose.
* Write the body about the *rule*, not about the page's own example. The body is
  static and the diagnostic above it is not, so a body that names a concrete type
  will sooner or later sit under a diagnostic about a different one.

## How this is enforced

Per catalog (`documentation/errors.rs` and `documentation/lints.rs`, same tests
in each):

| Test | What it holds |
|---|---|
| `every_page_parses` | No page in the catalog fails to parse. |
| `every_page_carries_its_wording` | Every registered code's page opens a `---` block. Without this a page that lost its frontmatter is silent until something provokes the code. |
| `every_migrated_page_is_titled_and_worded` | No empty `title`, no empty `message`. |
| `every_code_is_unique_and_documented` | One row per code, no empty page, and a `code=<code>` reproduction unless the page says `reproduction: none`. |
| `every_placeholder_is_snake_case` | Every `{placeholder}` in every template. |
| `every_see_also_names_a_topic` | Every `see_also` in the registry points at a real topic. |

And across the tree:

| Test | What it holds |
|---|---|
| `docs::documents::every_emitted_code_is_documented` | Every code any Rust source attaches has a page in one of the two catalogs. |
| `docs::documents::every_error_page_is_provoked_by_its_own_example` | Every reproduction still produces the code its page is named after. |
| `conformance::rejected_programs` | Every rejected program's JSON diagnostic carries a `fix`. |

The golden corpora record rendered output: `cli/tests/reject/*/expected.{txt,json}`,
and `expected/*.{txt,json}` under `cli/tests/repositories/`, `cli/tests/failing/`
and `cli/tests/cli/`. Editing a `message`, `label`, `note` or `fix` changes them.

```
BURI_BLESS=1 cargo test -p buri
```

regenerates every one. Read the diff before keeping it — `expected.json` should
move only when a code was split or renamed, or when the data bound to a
placeholder changed. A `.txt` that moves with no `.json` beside it is a body
edit, which is expected; a `.txt` whose `error:` line changed is a rewording,
which should be one you meant.

## The placeholder vocabulary

Every `{placeholder}` on every page, with what it holds and which codes use it.
Values are strings the call site has already finished: it does the quoting, the
pluralization and the joining, and the template supplies the backticks.

| Placeholder | What it holds | Codes |
|---|---|---|
| `{arity}` | The number of elements a tuple or a tuple pattern has. Always the *N* in `{arity}-tuple`. | `no-such-tuple-element`, `pattern-not-a-tuple` |
| `{artifact}` | What a non-native platform produces, as the fix's subject: `JavaScript`, `a page`. | `output-with-an-architecture` |
| `{because}` | The clause saying why a platform withholds a `core/host` grant. | `host-not-granted` |
| `{block}` | Where an unknown build-file field was written, already described: `` a `binary` rule ``, `` a `tag` block ``, `REPO.buri`. | `unknown-field` |
| `{candidates}` | The schemas that could claim an ambiguous proto type name, sorted and joined with `, or `. | `proto-ambiguous-type` |
| `{character}` | The character the lexer could not start a token with, as the source wrote it. | `unexpected-character` |
| `{choices}` | The finished list of the bare words a build-file field accepts. | `not-a-bare-word`, `unknown-bare-word` |
| `{code_point}` | That character's scalar value, in the lexer's own `{:04X}` form. | `unexpected-character` |
| `{construct}` | The proto construct the reader refuses, by name (`service`). | `proto-unsupported` |
| `{container}` | `` a `Result` `` or `` an `Option` ``. Used twice in the one sentence. | `question-mark-mismatch` |
| `{count}` | How many of a thing were declared — a tuple struct's fields, a function's parameters. | `no-such-positional-field`, `too-many-parameters` |
| `{cycle}` | The import stack from the first repeat onwards, already joined with ` -> `. | `circular-import`, `proto-circular-import` |
| `{declaration}` | On the language pages, the whole noun phrase for the thing declared or hidden (`` field `a` ``, `` variant `Yes` of `T` ``). On the proto pages, what the file declared, already described (`` `syntax = "proto3"` ``). See the note below the table. | `duplicate-declaration`, `private-to-module`, `proto-edition`, `proto-syntax-declaration` |
| `{dependency}` | The label of the library in question (`//lib/store`). | `missing-dep` |
| `{depth}` | How many branch bodies enclose the reported one, itself included. | `deep-nesting` |
| `{edition}` | `REQUIRED_EDITION` — the one Protobuf edition this reader implements. | `proto-edition`, `proto-edition-missing`, `proto-syntax-declaration` |
| `{effect}` | The effect's name. | `duplicate-bound`, `effect-and-trait`, `host-not-granted` |
| `{escape}` | The one character after a backslash that is not an escape. | `unknown-escape` |
| `{expected}` | What the declaration, the grammar or the schema says: a rendered type, a decimal count, or a finished noun phrase (`` `;` ``, `a block`, `platform names`). | `argument-count-mismatch`, `field-wrong-kind`, `not-a-bare-word`, `pattern-type-mismatch`, `signature-mismatch`, `type-argument-arity`, `type-argument-count`, `type-argument-mismatch`, `type-mismatch`, `unexpected-token`, `unknown-bare-word`, `wrong-argument-count`, `wrong-matched-value-count`, `wrong-value-count` |
| `{expected_plural}` | The plural of what a bare word should have been (`platforms`, `architectures`), because the fix names the whole set. | `unknown-bare-word` |
| `{exports}` | The names a test's import asked for, quoted and joined (`` `a`, `b` ``), or the phrase `what the test needs`. | `test-internal-import` |
| `{feature}` | The `features.<name>` a schema wrote. | `proto-unknown-feature` |
| `{field}` | A field's name: a build-file field as the schema spells it (`sources`, `arch`), a struct field as the source wrote it, or the `sources`/`proto_sources` a file belongs under. | `binary-field-not-allowed`, `duplicate-field-initializer`, `field-not-callable`, `field-wrong-kind`, `no-such-field`, `no-such-source`, `not-a-bare-word`, `underivable`, `unknown-field`, `unplaceable-source`, `unused-library` |
| `{field_type}` | The type of the field that blocks a derive. | `underivable` |
| `{fields}` | The `diagnostics::names` enumeration of the fields with no value, or with no pattern. | `missing-field-pattern`, `missing-field-value` |
| `{first_origin}` | The first of the two schemas that declare one proto type. | `proto-duplicate-type` |
| `{first_tag}` | The first of the two tags that forbid each other. | `tag-violation` |
| `{first_trait}` | The first of the two bounds declaring one method name, in the order the search met them — the fix quotes it. | `ambiguous-trait-method` |
| `{found}` | What is there instead of `{expected}`. | `argument-count-mismatch`, `field-wrong-kind`, `pattern-type-mismatch`, `signature-mismatch`, `type-argument-mismatch`, `type-mismatch`, `unexpected-token` |
| `{from}` | The error type `?` would propagate. | `error-type-mismatch` |
| `{from_target}` | The label of the package that depends. | `visibility-violation` |
| `{function}` | The called function's name, or the name written to the left of the type arguments. | `type-args-on-a-value`, `wrong-argument-count` |
| `{given}` | How many were given, where `{expected}` is how many are taken. | `type-argument-count`, `wrong-argument-count`, `wrong-value-count` |
| `{importer_file}` | The importing module's own file, so the note can say which rule it belongs to. | `binary-internal-import`, `binary-source-import` |
| `{index}` | The tuple element, or the positional field, that was asked for. | `no-such-positional-field`, `no-such-tuple-element` |
| `{known_features}` | The proto features this reader models, joined with `, `. | `proto-unknown-feature` |
| `{known_fields}` | The fields a build-file block accepts, joined with `, `. | `unknown-field` |
| `{last}` | The highest legal tuple index, which the fix names. | `no-such-tuple-element` |
| `{library_file}` | The path of a library's `lib.buri`, which is where the re-export would go. | `dead-code` |
| `{limit}` | The fixed lint threshold, bound from the constant in `lint.rs` so the number exists in one place. | `deep-nesting`, `oversized-function`, `too-many-parameters` |
| `{lines}` | How many lines a function body spans, opening brace to closing brace inclusive. | `oversized-function` |
| `{literal}` | The literal exactly as the source wrote it — prefix, underscores and sign included. | `float-as-a-tuple-index`, `integer-not-in-base`, `integer-too-wide`, `integer-without-digits`, `literal-out-of-range`, `not-a-float-literal`, `not-a-tuple-index` |
| `{marker}` | Which marker a comment carries: `TODO`, `FIXME` or `HACK`. | `warning-comment` |
| `{matched}` | How many values a pattern matched, where `{expected}` is how many the variant holds. | `wrong-matched-value-count` |
| `{method}` | The method looked up in, or supplied to, a type or a trait. | `ambiguous-trait-method`, `method-supplied-twice`, `no-such-method`, `not-a-trait-method`, `signature-mismatch` |
| `{methods}` | The `diagnostics::names` enumeration of the methods an `impl` is missing. | `incomplete-impl` |
| `{module}` | The module path two `import` statements both name, unquoted — the template supplies the backticks. | `duplicate-import` |
| `{module_file}` | The colliding module's file, from the repository root (`lib/money/cents.buri`). | `package-shadows-a-module` |
| `{name}` | The identifier the diagnostic is about, where no narrower role name applies. See the note below the table. | `ambiguous-free-function`, `context-not-a-value`, `dead-code`, `declaration-without-a-body`, `derive-not-a-trait`, `duplicate-field`, `duplicate-method`, `duplicate-module-declaration`, `duplicate-pattern-binding`, `effect-param-not-ctx`, `host-not-granted`, `impl-fn-without-self`, `impl-outside-its-module`, `lambda-captures-effect`, `lambda-captures-generic`, `method-declared-free`, `method-not-a-value`, `missing-field-value`, `missing-payload-pattern`, `no-such-export`, `no-type-arguments`, `not-a-trait`, `not-an-effect`, `not-on-the-surface`, `oversized-function`, `proto-ambiguous-type`, `proto-duplicate-type`, `proto-unknown-type`, `too-many-parameters`, `trait-not-an-effect`, `trait-used-as-a-type`, `type-not-a-value`, `type-parameter-with-arguments`, `uninhabited`, `unresolved-name`, `unresolved-type`, `unresolved-type-in-pattern`, `unused-import`, `unused-variable`, `wrong-matched-value-count`, `wrong-value-count` |
| `{operations}` | The intrinsic operations a toolchain cannot compile, quoted and joined by `diagnostics::names`. | `networking-not-available` |
| `{operator}` | The operator's source text (`~`, `<<`, `Add`, `Neg`). | `bitwise-on-a-non-integer`, `derive-operator-not-a-newtype`, `derive-operator-not-numeric` |
| `{other}` | The label at the far end of the reported dependency edge. | `dep-cycle` |
| `{owner}` | The label of the library whose surface or internals are being reached (`//lib/money`). | `binary-internal-import`, `binary-source-import`, `internal-import`, `not-on-the-surface`, `test-internal-import` |
| `{owner_path}` | That label with `//` stripped, because the note names `lib/money/lib.buri`. | `binary-internal-import`, `internal-import`, `test-internal-import` |
| `{package}` | The label of the package the build-graph rule is reported against. | `no-main`, `undeclared-testing-surface` |
| `{package_path}` | A package's path from the repository root, with no leading `//` — every use already prefixes it. | `missing-dep`, `package-shadows-a-module`, `package-without-a-rule`, `undeclared-testing-surface`, `unused-library` |
| `{parent_package}` | The package that holds the colliding module. | `package-shadows-a-module` |
| `{path}` | The module path an import wrote, or the schema path an `import` line spells. | `binary-entry-import`, `binary-internal-import`, `binary-source-import`, `circular-import`, `internal-import`, `module-outside-repository`, `no-such-export`, `no-such-module`, `proto-circular-import`, `proto-import-not-found`, `relative-import`, `test-source-import` |
| `{platform}` | The platform, spelled as the sentence wants it — `Platform::slug()` (`js`, `linux`) in prose, `Platform::proto()` (`JS`) where the sentence quotes a build file. | `host-not-granted`, `output-with-an-architecture`, `platform-not-implemented`, `platform-violation` |
| `{platform_in_build_file}` | `Platform::proto()` — the spelling `test.platforms` uses (`JS`, `LINUX`). Two placeholders rather than one because the sentence and the build file disagree about case. | `platform-not-implemented` |
| `{platforms}` | The phrase listing the platforms that *do* grant the host effect. | `host-not-granted` |
| `{position}` | Where `self` may appear, as a phrase: `a function's first parameter`, `the first parameter`. | `self-not-first` |
| `{problem}` | The whole message sentence, supplied by the call site. See “When a page binds its whole sentence”. | `build-file-syntax`, `module-not-found`, `proto-schema`, `style-not-static`, `unknown-visibility` |
| `{quoted_title}` | A test's title **with its quotes**, bound as `format!("{name:?}")`, so a title holding a quote or a backslash escapes exactly as it did. | `duplicate-test-name`, `test-title-newline`, `test-without-assertion` |
| `{radix}` | The base an integer prefix names, as a decimal number (`2`, `8`, `16`). | `integer-not-in-base` |
| `{reached}` | Which way reachability went, as a finished phrase: `` both `lib.buri` and `main.buri` `` or `` neither `lib.buri` nor `main.buri` ``. | `unplaceable-source` |
| `{reaches}` | How the use was found: `imports` when an import names the library, `uses` when only method resolution reaches it. | `missing-dep` |
| `{reason}` | Why a proto construct is refused. | `proto-unsupported` |
| `{remedy}` | The whole fix sentence, supplied by the call site. | `build-file-syntax`, `proto-schema`, `proto-unsupported` |
| `{requirement}` | One of `main`'s three requirements, as a phrase (`takes no parameters`). | `main-signature` |
| `{roots}` | `standard_library::roots_phrase()` — the reserved module roots as a finished phrase. | `no-such-module` |
| `{rule}` | The rule kind that owns the empty `test` block: `library` or `binary`. | `empty-test-suite` |
| `{second_origin}` | The second of the two schemas that declare one proto type. | `proto-duplicate-type` |
| `{second_tag}` | The second of the two tags that forbid each other. | `tag-violation` |
| `{second_trait}` | The second of the two bounds declaring one method name. | `ambiguous-trait-method` |
| `{seconds}` | A suite's declared `timeout_seconds`, or `0` when it declares none. Bound as a string; the `s` suffix is in the page. | `test-timeout` |
| `{source}` | A source file as the rule, or the directory walk, spells it — relative to its package. | `duplicate-source`, `entry-point-listed`, `no-such-source`, `proto-source-not-a-schema`, `unplaceable-source`, `unused-library` |
| `{tag}` | A tag's name as `REPO.buri` or a `tags` list wrote it. | `duplicate-tag`, `unknown-tag` |
| `{target}` | The label of the target the rule is reported against. | `dep-cycle`, `platform-violation`, `tag-violation`, `test-timeout`, `unsatisfiable-target` |
| `{test_source}` | The importing test source's file. | `test-internal-import` |
| `{to}` | The error type the function returns. | `error-type-mismatch` |
| `{to_package_path}` | The dependency's package path, for the `BUILD.buri` to edit. | `visibility-violation` |
| `{to_target}` | The label of the dependency that is not visible. | `visibility-violation` |
| `{trait}` | The trait, or the effect, the diagnostic is about — without backticks, which the templates carry. | `derive-only-trait`, `duplicate-implementation`, `effect-and-trait`, `effect-carrying-bound`, `incomplete-impl`, `missing-conformance`, `no-structural-derive`, `not-a-trait-method`, `signature-mismatch`, `trait-not-derivable`, `type-arguments-required`, `underivable`, `unsatisfied-bound` |
| `{type}` | The rendered type, without backticks. | `bitwise-on-a-non-integer`, `context-spread-operand`, `derive-only-trait`, `derive-operator-not-a-newtype`, `derive-operator-not-numeric`, `duplicate-field`, `duplicate-implementation`, `duplicate-method`, `effect-and-trait`, `effect-carrying-bound`, `enum-without-a-variant`, `field-not-callable`, `incomplete-impl`, `lambda-captures-generic`, `literal-out-of-range`, `missing-conformance`, `no-such-field`, `no-such-method`, `no-such-positional-field`, `no-such-variant`, `not-a-tuple`, `not-an-enum`, `not-callable`, `not-indexable`, `not-interpolatable`, `pattern-not-a-tuple`, `pattern-not-an-array`, `question-mark-mismatch`, `statement-not-unit`, `try-operand`, `type-argument-arity`, `type-argument-count`, `type-has-no-methods`, `underivable`, `unsatisfied-bound` |
| `{user}` | Whoever needs the dependency: the importing file's path at the import site, the package's label at the resolution site. | `missing-dep` |
| `{value}` | What the file wrote where a closed set of words, or a known feature value, was expected. | `proto-unknown-feature`, `unknown-bare-word` |
| `{variant}` | The variant name after the dot, without the dot. | `no-such-variant`, `unannotated-variant` |
| `{visible_to}` | `Workspace::visibility_list` — the finished list of who the dependency *is* visible to. | `visibility-violation` |
| `{witness}` | The rendered uncovered pattern. | `match-not-exhaustive` |
| `{word}` | The reserved word as written. | `reserved-word` |
| `{wrapped}` | The rendered type of the newtype's single field, which is not a number. | `derive-operator-not-numeric` |

### Names that carry a role, and the one that does not

`{name}` is the fallback: the identifier the diagnostic is about, where no
narrower word fits. Where the thing has a role in the sentence the vocabulary
names it — `{function}`, `{method}`, `{trait}`, `{type}`, `{variant}`,
`{field}`, `{effect}`, `{tag}`, `{module}`. The line is not perfectly drawn
(`ambiguous-free-function` binds `{name}` to a function), and it is not worth
redrawing: both names mean "an identifier, already unbackticked", so a page can
be moved from one to the other whenever its sentence reads better for it.

Three families are deliberately parallel rather than unified, because their
sentences are:

* `{expected}` / `{found}` — `expected …, found …`.
* `{expected}` / `{given}` — `takes …, but … were given`.
* `{expected}` / `{matched}` — the same, for a pattern.

`{arity}` is always the *N* in `{arity}-tuple`; `{count}` is a number in prose.
`{package_path}`, `{owner_path}` and `{to_package_path}` are paths with no
leading `//`; `{package}`, `{owner}`, `{target}`, `{dependency}`, `{from_target}`
and `{to_target}` are labels that keep it.

**One name still carries two meanings.** `{declaration}` is the noun phrase for
a thing declared on `duplicate-declaration` and `private-to-module`, and the
text of a schema's own declaration on `proto-edition` and
`proto-syntax-declaration`. The two never meet on one page, so nothing is
ambiguous at a call site, but the second pair would read better as
`{declared_as}` if those pages are ever revised.

### When a page binds its whole sentence

Five pages are `message: {problem}`, and one of those also has `fix: {remedy}`.
Each is a helper with a dozen or more callers whose sentences are all the same
rule: `build-file-syntax` and `proto-schema` are parsers ("this file does not
parse"), `module-not-found` is one rule stated six ways, `unknown-visibility`
is one rule stated six ways, and `style-not-static` says in its own source
comment why it is one code. They are the exception, not a pattern to copy: a
page whose message is a placeholder is a page with nothing on it to edit.

## Adding a code

1. Write `cli/src/docs/errors/<code>.md` — frontmatter, then the
   fenced `buri fail code=<code>` reproduction, or `reproduction: none`.
2. Register it in `cli/src/documentation/errors.rs` (or `lints.rs`), in the
   sorted list, with the page's title repeated verbatim and a `see_also` only
   where a chapter sets the rule out at length.
3. Emit it with `Diagnostic::templated`, binding exactly the placeholders the
   page uses.
4. `BURI_BLESS=1 cargo test -p buri`, and read the diff.
