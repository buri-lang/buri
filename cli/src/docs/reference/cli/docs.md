## What it does

Serves the language reference, the build system documentation, and this CLI
reference — from the binary, so what you read is what this toolchain does.
There is no website to fall out of date and no second copy to forget.

```text
buri docs                          every page, grouped
buri docs language/effects         one topic
buri docs cli build                one command, flags generated from the dispatch table
buri docs error result-discarded   one diagnostic, with a program that provokes it
buri docs core/list                a standard library module, rendered from its source
buri docs core/list.map            one item of one module
buri docs search <words>           every page at once
buri docs manifest                 every id and output shape, for an agent
buri docs assemble                 regenerate cli/src/docs/SPEC.md
```

It works outside a repository: the prose ships inside the executable.

## For agents

`--format=json` prints one object on one line. `--dense` drops prose but keeps
every heading and **every example** — code is what a caller needs most, so it
is never abridged. `buri docs manifest` lists every id that can be fetched, and
a test asserts that each one really can be.

## Why this cannot go stale

Every fenced example in every page is compiled by the test suite against the
real standard library; the ones that print something are executed and their
output compared. **That includes examples written in `///` and `//!` comments
in `.buri` sources**, not only the ones in prose pages — a documentation
comment is documentation, and an example in one has the same claim on being
true. `buri docs test` reads a source file through its comments, and a failure
names the `.buri` line the example is written on.

Every fenced example is also held to the **layout** `buri format` writes, in
the same run and by the same printer: what a reader copies out of a page is the
house style, so it is the house style that is printed there. `buri format` over
the documentation is what fixes one.

`cli/src/docs/SPEC.md` is generated from these same topics, and a test fails if
the checked-in file drifts from what the topics produce.
