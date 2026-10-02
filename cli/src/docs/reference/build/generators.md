# `generators`

A generator is a [tool](./tools.md)'s `generate`, run by the build. Every module
it answers with belongs to the rule that declared it, exactly as a `.buri`
source does.

```textproto schema=build
# lib/wire/BUILD.buri
library {
    generators: [
        { tool: "//tool/units", inputs: ["units.txt"] },
    ]
}
```

Nothing reaches the source tree. There is no generated file to check in and no
step to forget to run.

| Field | Meaning |
|---|---|
| `tool` | A `//label` naming a `tool` rule with a `generate` entry point, `std/json` for [types from JSON](../../guides/json.md#generating-types), `std/proto` for `.proto` schemas, or `std/textproto` for [a text format file's value](../../guides/textproto.md#generating-its-value). |
| `inputs` | The files handed to the tool, package-relative, no globs. |

`generators` is hand-authored, like `visibility` and `outputs`. `buri gen`
cannot know which generator owns a file, so it never writes the field and never
touches an entry's `inputs`.

An input in a language the repository knows is **checked before the tool reads
it**, by its language's `check`. One that fails is reported where the mistake
is, and the entry does not run. See [`guides/json.md`](../../guides/json.md) and
[`repo-config.md`](./repo-config.md).

An input counts as declared, the way a `sources` entry does. The question is
asked back, too, but only of files wearing an extension a generator in that
package already reads: once an entry names a `.units`, a second `.units` nothing
names is [`unused-library`](../lints/unused-library.md). A generator reads
whatever it likes, so a `README.md` is still nobody's.

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
from "core/effect" import { Allocator };
from "core/tool" import { Generated, GenerateRequest };

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

You build the module as `core/buri/ast` nodes, so a generator cannot emit a
parse error. The toolchain prints them and sends text plus anchors — never the
tree — and the compiler reads that text with its one ordinary parser.

A generator that has to *read* Buri — one whose input is source rather than a
schema — goes the other way with `ast.parse(ctx, file, source)`. Every node it
answers carries an `Origin` naming that file and the bytes it came from, so what
you build out of it anchors the same way what you built by hand does.

A generator used to be a `binary` whose `main` called `core/codegen`'s `run`.
Naming one is [`generator-is-a-binary`](../errors/generator-is-a-binary.md):
move the package under `tool/`, move `main.buri` to `tool.buri`, export
`generate`, and declare `generate {}`.

## The modules it produces

A module the generator names `N` is importable as
`<the declaring package's label>/N`. So `//lib/wire` running the tool above gets
`//lib/wire/units`, and its `lib.buri` decides which of those names leave the
library with `from "//lib/wire/units" export { width };`.

The module is internal to the rule, so another package reaches it through
`lib.buri` and nothing else. `unused-import` and `dead-code` step around it:
both ask a person to make an edit, and here there is no file to edit.

Your editor follows an origin wherever a generated name has one. Go-to-definition
opens the input at the bytes the name was written from — on the name, on a
re-export of it, and on the import path, which opens the input rather than the
module that has no file. Hover shows the generated signature and the comment
above that line of the input. Rename is refused, naming the tool and the input.
Find-references lists every place your code names it and not the generated
declaration, and a generated name is not a workspace symbol: neither is
somewhere a person can edit.

The name has to be the generator's own. Two entries naming one module, or a
module named after a `.buri` file of the package — `lib.buri` included — is
[`generator-module-taken`](../errors/generator-module-taken.md), and whichever
was there first is what the build compiles.

## What a generator says

A generator answers with diagnostics as well as modules. One whose `code` names
a page in the catalogue prints under that code, with the tool's own sentence.
One whose code the catalogue does not have prints under
[`tool-diagnostic`](../errors/tool-diagnostic.md), naming the code it asked for.

A diagnostic carrying an origin is reported at that span of that input. One
carrying none lands on the `generators` entry that ran the tool.

A tool that does not build, exits non-zero, is killed by a signal, or writes
something that is not an answer is [`tool-failed`](../errors/tool-failed.md) —
with the reason, and the tail of standard error, in the note. A tool built from
the target that declares it is
[`generator-cycle`](../errors/generator-cycle.md).

Taking a long time is not a failure. Nothing puts a clock on a tool, so the
build waits for as long as it runs, the way it waits for a compiler or a linker.

## The cache

Running a generator is an action like any other, keyed on the tool's program and
the contents of every input:

```
$ buri build //lib/wire --explain
keyed  generate //lib/wire js 4c1f0b8e2a71
keyed  compile //lib/wire js 07d2690d5c30
```

So editing an input re-runs the tool and rebuilds what read its modules, editing
the tool does the same, and `--check-reproducible` covers a generator for free.
[Hermeticity](./hermeticity.md) has the rest of the model.

A tool is compiled to JavaScript and run under the JavaScript runtime on the
machine doing the build. `std/proto` takes the same path: it is a Buri program
whose `generate` calls the `emit` of `std/codegen/proto`, compiled the first
time a build needs it and kept under `.buri/out/tools` by its key.
