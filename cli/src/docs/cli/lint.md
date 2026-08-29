## What it does

The static checks that are not type errors: sources declared but absent, a
source on disk that no rule names, a dependency that is declared and unused,
one that is used and undeclared, a visibility or tag violation, a cycle in the
package graph, and the hygiene rules — an import nothing uses, an `export`
nothing reaches, a test that asserts nothing.

Each finding carries a stable code, so a report can be grepped and a specific
check can be talked about by name. Every one of them is a warning: there is one
catalogue and one severity, the same in every repository, and
[`cli.md`](../build/cli.md#lint) is the whole list.

## When the target does not type check

The catalogue still runs, and the report holds both halves: the type errors the
front end found, and every finding those errors cannot have caused. A file with
a mistake in one function is still a file with an import nothing uses, and
holding the second answer back until the first is fixed makes the tool tell you
one thing at a time about a file you are already looking at.

What a type error does take away is the typed tree under it. The checker stops
where the body stopped making sense, so everything written inside the failed
expression is gone — and a rule that read it would report the gap rather than
the code, calling a name unused because its only use went missing. The four
rules that read bodies — `unused-variable`, `test-without-assertion`,
`dead-code`, `ctx-rebinding` — therefore stay silent for exactly the scope that
failed: the body the checker gave up on, and, when the failure is at a module's
top level, every body in that module, because an import that resolved to
nothing changes what all of them can see. Nothing wider goes quiet, and the
rules that read the source rather than the tree — parameter counts, nesting
depth, function length, warning comments, test titles, duplicate imports,
unused imports — answer the same either way.

The silence is one-directional: a finding may be missed inside a broken body,
and none is invented there. Fix the type error and run the linter again to see
what the gap was hiding.

## Exit status

`0` if there was nothing to report, `1` if there was anything at all. Severity
does not enter into it — every finding is a warning, a type error riding along
in the same report is an error, and both are `1`, because a warning is still the
answer to the question you asked. Running the linter is itself the request to be
told, and a report that exits zero is one no script can branch on, so `buri
lint //...` is usable directly as a gate with no flag to make it one.

Whether a finding also stops `buri build` or `buri test` is a different
question, and the repository answers it rather than this command: the
[`lint` block](../build/repo-config.md#lint) in `REPO.buri` decides whether
those two run the catalogue at all (`check_during_build`) and whether what it
finds fails them (`fail_on_finding`). Both default to no. Neither can turn a
check off — the block only ever tightens.

## What a second run costs

A finding depends on two things and no others: the build graph, and the bytes
of the files the target's analysis read. So the answer is written down. Under
`.buri/cache`, beside the build's own entries, each target keeps one record —
the closure it read, each file with a hash of its bytes, and what the catalogue
found. A run that finds every one of those files holding the bytes the record
names reports what the record says; a run that finds any of them moved analyses
the target again and writes the record back.

That makes `buri lint //...` after a one-file edit re-analyse the targets whose
closure holds that file, and no others. The report is the same either way, to
the byte: a record carries findings and nothing else, so a cached finding is
sorted, promoted and printed by the code that would have printed a fresh one.

```
buri lint //... --explain
```

says which was which, one line per target:

```
cached lint //lib/money - 8b2e77c1904a
run    lint //cmd/web - 5fda356eb977
```

The fourth column is the platform for a build action, and `-` here: a lint asks
one question of a target's whole closure whatever that target is built for. The
key is over the target and the build graph rather than over the closure, so it
does *not* move when a source is edited — what moved is inside the record, and
one target keeps one entry however long you edit it.

Three things can make a record unusable, and each of them ends in re-analysis
rather than in a stale answer:

- **A file moved.** Any file the record names, including one that appeared or
  went away — the list of `.buri` files in the repository is part of the key.
- **The graph moved.** A `BUILD.buri` or `REPO.buri` edit changes the key for
  every target, because a build file decides what a closure *is*.
- **The toolchain moved.** The key holds this `buri`'s version, so a record an
  older one wrote is unreachable rather than trusted.

`buri clean` drops the records with the rest of the cache. Two `buri lint` runs
on one repository are safe to overlap: a record is written to a temporary and
renamed into place, so a reader sees a whole one or none.

## `--fix`

```
buri lint //... --fix
```

Applies the findings that have exactly one mechanical answer, then runs the
whole check again from the files on disk and reports what is left. The count is
the linter's, not arithmetic — an edit can uncover a finding the first pass
could not see.

Two kinds of answer, and they are applied differently:

- **A build file that disagrees with the code** — `missing-dep`,
  `unused-library`, `duplicate-source` — is handed to `buri gen`, which
  already writes exactly that file and preserves `tags`, `visibility`,
  `outputs`, and comments. A `BUILD.buri` is never edited byte by byte, so
  `lint --fix` and `gen` cannot end up disagreeing about what it should say.
- **A source edit** — `unused-import` — is applied as bytes, one edit per
  import statement rather than one per name, because two adjacent unused names
  share the comma between them.

Everything else is left alone and reported. A `dep-cycle` has no mechanical
answer — which of the two edges to cut is a design decision — and a tool that
picks one is not fixing the finding, it is deleting the policy that raised it.

**`--fix` edits; it does not reformat.** It writes the bytes the findings name
and checks the result still parses. Running the file through the formatter
would answer that question too, and would also rewrite everything the fix did
not touch, which turns one deliberate edit into a diff nobody asked for. Run
`buri format` when you want the file formatted.

Where two edits in one file overlap, none of that file's are applied and the
findings are reported instead. Guessing which of two answers was meant is the
one thing a rewriting tool must not do.

These are separate from type checking because they are questions about the
*build graph* rather than about a program, and because a repository wants to be
able to run them without paying for a full compile.

The rules that do need one — `missing-dep`, `dead-code` and the hygiene set —
read an analysis rather than running their own, which is what lets the language
server report the same findings on a keystroke for the price of the type errors
it was going to report anyway.
