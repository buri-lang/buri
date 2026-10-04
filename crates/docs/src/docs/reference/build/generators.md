# `generators`

A generator is a [tool](./tools.md)'s `generate`, run by the build. Its modules
belong to the rule that declared it, like a `.buri` source.

```textproto schema=build
# lib/wire/BUILD.buri
library {
    generators: [
        { tool: "//tool/units", inputs: ["units.txt"] },
    ]
}
```

Nothing reaches the source tree, so there's no generated file to check in.

| Field | Meaning |
|---|---|
| `tool` | A `//label` naming a `tool` rule with a `generate` entry point, `json` for [types from JSON](../../guides/json.md#generating-types), `proto` for `.proto` schemas, or `textproto` for [a text format file's value](../../guides/textproto.md#generating-its-value). |
| `inputs` | The files handed to the tool, package-relative, no globs. |

You write `generators` by hand, like `visibility` and `outputs`. `buri gen`
can't know which generator owns a file, so it never touches the field.

An input in a language the repository knows is **checked before the tool reads
it**. If the check fails, the entry doesn't run. See [`guides/json.md`](../../guides/json.md) and
[`repo-config.md`](./repo-config.md).

An input counts as declared, like a `sources` entry. Undeclared files are
flagged only if a generator in the package reads their extension: once an entry
names a `.units`, a second `.units` nothing names is
[`unused-source`](../lints/unused-source.md). A stray `README.md` isn't.

## Writing one

```textproto schema=build
# tool/units/BUILD.buri
tool {
    generate {}
}
```

```buri
// tool/units/tool.buri
from "core/buri/ast" import * as ast;
from "core/tool" import { Generated, GenerateRequest };
from "platform/effect" import { Allocator };

/// One module named `units`, holding an `export let` per input.
export fn generate<C: Allocator>(ctx: C, request: GenerateRequest<Str>): Generated {
    let items = request.inputs.map(ctx, fn(input) => width(input.path));
    Generated {
        modules: [("units", ast.Module { items: items, docs: [] })],
        diagnostics: [],
        needs: [],
    }
}

/// The `origin` is where the declaration came from: go-to-definition on the
/// generated `width` lands on those bytes of the input.
fn width(path: Str): ast.Item {
    let int = ast.Name { text: "Int", origin: ast.nowhere() };
    ast.Item {
        kind: .Let(ast.LetDecl {
            name: ast.Name { text: "width", origin: ast.nowhere() },
            ty: ast.Type { kind: .Named(int, []), origin: ast.nowhere() },
            value: ast.Expr { kind: .Int(3), origin: ast.nowhere() },
            exported: true,
            docs: [],
        }),
        origin: ast.origin(path, 0, 5),
    }
}
```

You build modules from `core/buri/ast` nodes, so a generator can't emit a parse
error. The toolchain prints them as text plus anchors, and the compiler parses
that text like any source.

To *read* Buri input, use `ast.parse(ctx, file, source)`. Every node carries an
`Origin` pointing at its bytes, so what you build from it anchors like
hand-built nodes.

A `binary` named as a generator is
[`generator-not-tool`](../errors/generator-not-tool.md): move the package
under `tool/`, rename `main.buri` to `tool.buri`, export `generate`, and declare
`generate {}`.

## The modules it produces

A module named `N` is importable as `<declaring package>/N`. So `//lib/wire`
running the tool above gets `//lib/wire/units`, and its `lib.buri` re-exports
what leaves the library: `from "//lib/wire/units" export { width };`.

The module is internal to the rule. `unused-import` and `dead-code` skip it,
since there's no file to edit.

In your editor:

- Go-to-definition, on a generated name, a re-export, or the import path, opens
  the input at the name's origin.
- Hover shows the generated signature and the comment above that line of the
  input.
- Rename is refused, naming the tool and the input.
- Find-references lists your uses, not the generated declaration, and generated
  names aren't workspace symbols.

Two entries naming one module, or a module named after a `.buri` file in the
package (`lib.buri` included), is
[`generator-duplicate-module`](../errors/generator-duplicate-module.md); the build
compiles whichever came first.

## What a generator says

A generator returns diagnostics alongside modules. One whose `code` has a
catalogue page prints under that code, with the tool's sentence. Any other code
prints under [`tool-diagnostic`](../errors/tool-diagnostic.md), naming the code.
A diagnostic with an origin lands on that span of the input; one without lands
on the `generators` entry.

A tool that doesn't build, exits non-zero, is killed by a signal, or writes
something that isn't an answer is [`tool-failed`](../errors/tool-failed.md),
with the reason and the tail of stderr in the note. A tool built from the
target that declares it is [`circular-generator`](../errors/circular-generator.md).

There's no timeout: the build waits as long as a tool runs.

## The cache

Running a generator is an action keyed on the tool's program and every input's
contents:

```
$ buri build //lib/wire --explain
keyed  generate //lib/wire js 4c1f0b8e2a71
keyed  compile //lib/wire js 07d2690d5c30
```

Editing an input or the tool re-runs it and rebuilds what read its modules, and
`--check-reproducible` covers generators too. See
[hermeticity](./hermeticity.md).

A tool compiles to JavaScript and runs on the build machine. `proto` is one too:
a Buri program calling `std/codegen/proto`'s `emit`, compiled on first use and
cached under `.buri/out/tools`.
