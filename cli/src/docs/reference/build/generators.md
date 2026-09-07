# `generators`

A generator is a program the build runs. It reads one JSON request on standard
input, writes one JSON response back, and every module it names belongs to the
rule that declared it, exactly as a `.buri` source does.

```textproto schema=build
# lib/wire/BUILD.buri
library {
    generators: [
        { tool: "//cmd/gen", inputs: ["units.txt"] },
    ]
}
```

Nothing reaches the source tree. There is no generated file to check in and no
step to forget to run.

| Field | Meaning |
|---|---|
| `tool` | The program to run. A `//label` names a binary target in this repository; anything else names a generator the toolchain ships, and `std/codegen/proto` is the only one. |
| `inputs` | The files handed to the tool, package-relative, no globs. |

`generators` is hand-authored, like `visibility` and `outputs`. `buri gen`
cannot know which generator owns a file, so it never writes the field and never
touches an entry's `inputs`.

An input counts as declared, the way a `sources` entry does. The question is
asked back, too, but only of files wearing an extension a generator in that
package already reads: once an entry names a `.units`, a second `.units` nothing
names is [`unused-library`](../lints/unused-library.md). A generator reads
whatever it likes, so a `README.md` is still nobody's.

## Writing one

`core/codegen`'s `run` is the whole of a generator's `main`:

```buri role=entry
from "core/buri/ast" import * as ast;
from "core/codegen" import * as codegen;
from "core/codegen" import { Request, Response };
from "core/effect" import { Alloc, Stdin, Stdout };
from "core/host" import * as host;

export fn main(): Result<(), Str> {
    let ctx = context {
        Alloc: host.alloc,
        Stdin: host.stdin,
        Stdout: host.stdout,
    };
    codegen.run(ctx, fn(c, request) => generate(c, request))
}

/// One module named `units`, holding an `export let` per input.
fn generate<C: Alloc>(ctx: C, request: Request): Response {
    let items = request.inputs.map(ctx, fn(input) => width(input.0));
    Response {
        modules: [("units", ast.Module { items: items, docs: [] })],
        diagnostics: [],
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
parse error. `run` prints them and sends text plus anchors — never the tree —
and the compiler reads that text with its one ordinary parser.

`run` hands your `generate` the context `main` built, bounded by `Alloc`,
`Stdin` and `Stdout`. Write `main` the way the example does and reaching for the
clock or the filesystem is a type error, not a rule to remember. Bind more than
those three and you are answering for the result yourself: what a generator
writes has to be a function of what it was handed, and `--check-reproducible` is
what asks.

`buri docs core/codegen` has the request and response documents, byte for byte.

## The modules it produces

A module the generator names `N` is importable as
`<the declaring package's label>/N`. So `//lib/wire` running the tool above gets
`//lib/wire/units`, and its `lib.buri` decides which of those names leave the
library with `from "//lib/wire/units" export { width };`.

The module is internal to the rule, so another package reaches it through
`lib.buri` and nothing else. `unused-import` and `dead-code` step around it:
both ask a person to make an edit, and here there is no file to edit.

The name has to be the generator's own. Two entries naming one module, or a
module named after a `.buri` file of the package — `lib.buri` included — is
[`generator-module-taken`](../errors/generator-module-taken.md), and whichever
was there first is what the build compiles.

## What a generator says

A generator answers with diagnostics as well as modules. One whose `code` names
a page in the catalogue prints under that code, with the generator's own
sentence. One whose code the catalogue does not have prints under
[`generator-diagnostic`](../errors/generator-diagnostic.md), naming the code it
asked for.

A diagnostic carrying an origin is reported at that span of that input. One
carrying none lands on the `generators` entry that ran the tool.

A tool that exits non-zero, is killed by a signal, writes nothing, or writes
something that is not a response is
[`generator-failed`](../errors/generator-failed.md) — with the status or the
signal, and the tail of standard error, in the note. A tool built from the
target that declares it is
[`generator-cycle`](../errors/generator-cycle.md).

Taking a long time is not a failure. Nothing puts a clock on a generator, so the
build waits for as long as the tool runs, the way it waits for a compiler or a
linker. The one deadline in the build system is `timeout_seconds` on a `test`
rule, and you write that one yourself.

## The cache

Running a generator is an action like any other, keyed on the tool's linked
artifact and the contents of every input:

```
$ buri build //lib/wire --explain
keyed  generate //lib/wire js 4c1f0b8e2a71
keyed  compile //lib/wire js 07d2690d5c30
```

So editing an input re-runs the tool and rebuilds what read its modules, editing
the tool does the same, and `--check-reproducible` covers a generator for free.
[Hermeticity](./hermeticity.md) has the rest of the model.

A repository tool is built for `JS` and run under the JavaScript runtime,
whatever the tool's own `outputs` say. A generator runs on the machine doing the
build, and an `.mjs` is the one artifact every host can produce and run without
a linker.

A generator the toolchain ships takes the same path. `std/codegen/proto` is a
Buri program — `core/codegen`'s `run` over the `emit` the standard library
exports — compiled to an `.mjs` the first time a build needs it, kept under
`.buri/out/toolchain`, and handed a request on standard input like any other
tool. There is no second path for it, which is the point: the `.proto`
generator is the worked example of this page rather than an exception to it.

That compile happens **once per repository**. The file's name is its action key,
so a new toolchain writes a new one instead of reading a stale one, and every
build after the first reads what is there — one schema or fifty, one target or
the whole tree, every platform. `buri clean` drops it with the rest of `.buri`,
and the next build pays for it again: on the order of a tenth of a second, in
front of the first schema read and nothing else.
