## What it does

The static checks that are not type errors: sources declared but absent, a
source on disk that no rule names, a dependency that is declared and unused,
one that is used and undeclared, a visibility or tag violation, a cycle in the
package graph, and the hygiene rules — an import nothing uses, an `export`
nothing reaches, a type nothing names or builds, a field nothing reads, a
variant nothing constructs, a test that asserts nothing.

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
`dead-code`, `ctx-rebinding` — therefore stay silent for exactly the body that
failed: the function the error landed in, its signature counted as part of it,
and no other. The body beside it is read, and so is the body below the
declaration that failed. Nothing wider goes quiet, and the rules that read the
source rather than the tree — parameter counts, nesting depth, function length,
warning comments, test titles, duplicate imports, unused imports — answer the
same either way.

`unused-context` reads bodies and answers anyway, and what lets it is the one
thing it has that the four above do not: a context has exactly one spelling.
There is no alias for `ctx` and no way to reach it under a name of your own, so
where the typed tree is truncated the rule asks the *text* instead — does this
declaration write `ctx` anywhere but at the parameter? The lexer answers that
whether or not the checker could read the expression around it, so a mistake in
one corner of a function does not take the finding about its signature with it.
What the error does cost is the fix: the calls the rewrite would have to touch
are read off the same tree, so a package that did not check whole gets the
finding with the sentence and no bytes.

`unused-context-bound` is the same rule one level in — which of a context's
*effects* the body uses — and it goes quiet for a failed body where
`unused-context` does not. The asymmetry is the spelling: a context has one,
and a bound has none. A bound is used through the method names it declares and
through callees whose own bounds name it, and neither is a token the lexer can
recognise as this bound, so where the typed tree is truncated there is nothing
to answer with. It is also silent about a `ctx` nothing reads at all, because
every bound on such a parameter is dead by construction and the edit that
signature needs is `unused-context`'s: take the parameter out, not trim it.

`dead-code` asks a second question and so has a second reason to go quiet. It
reads the package's imports and re-exports to decide which exported names
anything reaches, so an error landing on one of those lines — or a run of
declarations the parser could not read at all, which might have held the import
that reaches the name — leaves it with a shorter list than the author wrote,
and it declines to call anything unreached on that evidence. An error anywhere
else is not a reason, however far from a body it sits: an alias that closes a
cycle, a field whose type did not check, a signature that did not, each is one
declaration a reader can see is wrong, and none of them says anything about
what reaches what.

Three rules read the bodies for a different reason, and answer the same
question a different way. `unused-type`, `unused-field` and `unused-variant`
ask what the *package* does with a shape it declares, so their evidence is
spread over every body the package owns rather than sitting in the one that
holds the finding — going quiet for the failed body would say nothing useful,
because the read that vanished could have been anywhere. So the doubt is per
name instead: an identifier written inside a body that did not check, or inside
a run of declarations the parser skipped, is a name that might be used there,
and nothing is reported about the type, the field or the variant it could be
naming. A name that appears nowhere in the unreadable text is a name that text
does not use, and its finding stands beside the error.

Those three have a second reason of their own, and it is narrower than
`dead-code`'s. What they never report is what the library *publishes*: a name
`lib.buri` puts on the surface is public API, and so are the exported fields
and the variants of a published type. An `export` line that did not resolve is
therefore a line that may have published anything, and nothing exported is
reported unused until it is spelled right. It reaches no further. A declaration
the parser skipped and an import that did not resolve are both still text, and
what a name is used by is read out of the text; what cannot be recovered from
the text is which names `lib.buri` meant to publish, and a name with no
`export` on it was never one of them.

The silence is one-directional: a finding may be missed inside a broken body,
and none is invented there. Fix the type error and run the linter again to see
what the gap was hiding.

## When a file does not parse

The same answer, through the same machinery. A syntax error is an error in the
report like any other, and the linter reads on around it: the declarations the
parser did recover are analysed, the file beside a broken one is still read,
and a package whose *dependency* does not parse is still linted. So the report
holds the syntax error and every finding the mistake did not take with it, and
the exit code is the one any finding earns rather than a special one for a file
that did not parse.

What goes quiet is the scope named above and nothing wider: the body the
mistake landed in. Where the mistake *sits* is not the test — a declaration the
parser recovered whole hides nothing at all, and a missing `;` on an import is
recovered whole, so the unused binding twenty lines below it is still reported.
A run of declarations the parser could not read is the case that does cost
something: it takes its own findings with it, and it stops `dead-code` counting
for that package, on the grounds the section above gives.

There is one file the linter does not read around, and it is a build file. A
`BUILD.buri` or `REPO.buri` that does not parse is the shape of the repository
rather than something in it: nothing downstream knows which files a package
holds or what it may see. So the run stops there and says which file it was,
rather than reporting a graph it had to guess at.

## Exit status

`0` if there was nothing to report, `1` if there was anything at all. Severity
does not enter into it — every finding is a warning, a type error riding along
in the same report is an error, and both are `1`, because a warning is still the
answer to the question you asked. Running the linter is itself the request to be
told, and a report that exits zero is one no script can branch on, so `buri
lint //...` is usable directly as a gate with no flag to make it one.

A syntax error in a source file is `1` on the same grounds: it is part of the
report, printed beside the findings. `2` is not a report at all — it is the run
that could not start: a target pattern that names nothing, or a build file that
does not read (above).

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
