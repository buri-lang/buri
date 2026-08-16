## What it does

The static checks that are not type errors: sources declared but absent, a
source on disk that no rule names, a dependency that is declared and unused,
one that is used and undeclared, a visibility or tag violation, a cycle in the
package graph, and the hygiene rules — an import nothing uses, an `export`
nothing reaches, a test that asserts nothing.

Each finding carries a stable code, so a report can be grepped and a specific
check can be talked about by name.

## `--fix`

```
buri lint //... --fix
```

Applies the findings that have exactly one mechanical answer, then runs the
whole check again from the files on disk and reports what is left. The count is
the linter's, not arithmetic — an edit can uncover a finding the first pass
could not see.

Two kinds of answer, and they are applied differently:

- **A build file that disagrees with the code** — `missing-dep`, `unused-dep`,
  `undeclared-source`, `duplicate-source` — is handed to `buri gen`, which
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
