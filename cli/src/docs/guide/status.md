## Status and open questions

The toolchain builds, tests, formats, lints, serves a language server, and
answers questions about a monorepo. It emits JavaScript and native code:
`buri test` compiles a suite to a native binary by default and falls back to
JavaScript where the native surface does not reach yet, while a binary that
declares no output still gets JavaScript. The sharpest unresolved
trade-off is the rule that a lambda may not capture an effect
([SPEC.md §10.5](./cli/src/docs/SPEC.md)) — it is what makes the purity theorem hold
structurally, and it still forces any function that *stores* an effectful
callback to put the context in that callback's type. The runner-up is the
absence of `break`. [SPEC.md §15](./cli/src/docs/SPEC.md) lists those and five other questions
that want real programs before they can be settled.
