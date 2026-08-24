## Where the documentation is

Everything is compiled into the binary, so the fastest way to read any of it is
to ask the toolchain — it works in any directory, with or without a checkout:

```sh
buri docs                    # the index: every page, grouped
buri docs lang/effects       # one page
buri docs search "tail call" # find the page that answers a question
buri docs cli docs           # every other form, including the ones for an agent
```

The same pages are the files under [`cli/src/docs/`](./cli/src/docs/), which is
the one place documentation is edited:

| Where | What |
|---|---|
| [`cli/src/docs/SPEC.md`](./cli/src/docs/SPEC.md) | The language reference, assembled from `cli/src/docs/lang/` |
| [`cli/src/docs/grammar.ebnf`](./cli/src/docs/grammar.ebnf) | The normative grammar, in extended BNF |
| [`cli/src/docs/guide/`](./cli/src/docs/guide/) | The guide: goals, the three ideas, numbers, methods and traits, effects, the standard library |
| [`cli/src/docs/build/`](./cli/src/docs/build/) | The monorepo build system, `BUILD.buri`, tags, hermeticity, and the CLI reference |
| [`cli/src/docs/errors/`](./cli/src/docs/errors/) | One page per diagnostic, each with a program that provokes it |
| [`cli/tests/example/`](./cli/tests/example/) | A worked monorepo, and the largest body of Buri here to read |

Two things are documentation but not user documentation, and they live apart:
[`design/`](./design/) holds the working notes, roadmaps and design documents
that contributors write for each other, and [`formal/`](./formal/) holds the
Lean 4 formalisation of the type system.
