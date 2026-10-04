## What it does

Serves the language, build system and CLI references from the binary, so what
you read matches this toolchain.

```text
buri docs                          every page, grouped
buri docs language/effects         one topic
buri docs cli build                one command, flags generated from the dispatch table
buri docs error result-discarded   one diagnostic, with a program that provokes it
buri docs core/list                a standard library module, rendered from its source
buri docs core/list.map            one item of one module
buri docs search compare ints      every page at once, by name or by intent
buri docs manifest                 every id and output shape, for an agent
buri docs assemble                 regenerate crates/docs/src/docs/SPEC.md
```

It works outside a repository.

## Searching

`buri docs search <words>` finds every page those words are about:

```text
$ buri docs search compare ints
buri docs core/order.int  fn int
  A comparator value, for sortBy. Ordering is a value you pass, which makes
buri docs core/str.compare  method compare
  Lexicographic, by Unicode scalar value — the same unit len counts
…
```

Each result line is the command that reads that page. Search covers names, the
prose of every page (including `///` and `//!` comments), and a small table of
concepts that puts `core/order` under "compare" and `core/str` under "pad" even
where the page never uses the word. `--format=json` gives the same hits with a
`command` on each.

## For agents

`--format=json` prints one object on one line. `--dense` drops prose but keeps
every heading and every example. `buri docs manifest` lists every id you can
fetch.

## Why this cannot go stale

The test suite compiles every fenced example against the real standard library,
including examples in `///` and `//!` comments, and checks the output of the
ones that print. `buri docs test` reads a source file through its comments, and
a failure names the `.buri` line the example is on.

Every example must also match `buri format`'s layout. Run `buri format` over the
documentation to fix one.

`crates/docs/src/docs/SPEC.md` is generated from these topics, and a test fails if the
checked-in copy drifts.
