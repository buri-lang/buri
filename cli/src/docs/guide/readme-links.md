## Where the documentation is

All of it is compiled into the binary — `buri docs` is the index,
`buri docs search "tail call"` finds the page that answers a question, and both
work in any directory. The same pages are the files under
[`cli/src/docs/`](./cli/src/docs/): the language reference
[`SPEC.md`](./cli/src/docs/SPEC.md), the [guide](./cli/src/docs/guide/), the
[build system](./cli/src/docs/build/), and one page per diagnostic. A worked
monorepo lives in [`cli/tests/example/`](./cli/tests/example/);
[`design/`](./design/) holds contributor notes and [`formal/`](./formal/) the
Lean 4 formalisation of the type system.

## License

MIT — the text is in [`LICENSE`](./LICENSE) and covers everything here.
