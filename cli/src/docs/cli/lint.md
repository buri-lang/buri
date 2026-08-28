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

## Exit status

`0` if there was nothing to report, `1` if there was anything at all. Severity
does not enter into it — every finding is a warning, and a warning is still the
answer to the question you asked. Running the linter is itself the request to be
told, and a report that exits zero is one no script can branch on, so `buri
lint //...` is usable directly as a gate with no flag to make it one.

Whether a finding also stops `buri build` or `buri test` is a different
question, and the repository answers it rather than this command: the
[`lint` block](../build/repo-config.md#lint) in `REPO.buri` decides whether
those two run the catalogue at all (`check_during_build`) and whether what it
finds fails them (`fail_on_finding`). Both default to no. Neither can turn a
check off — the block only ever tightens.

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
