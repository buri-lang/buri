## What it does

```
buri lint //...
```

The static checks that are not type errors: sources declared but absent, a
source on disk that no rule names, a dependency that is declared and unused,
one that is used and undeclared, a visibility or tag violation, a cycle in the
package graph, and the hygiene rules — an import nothing uses, an `export`
nothing reaches, a type nothing names or builds, a field nothing reads, a
variant nothing constructs, a test that asserts nothing.

Each finding carries a stable code, so a report can be grepped and a specific
check can be talked about by name. Every one of them is a warning: there is one
catalogue and one severity, the same in every repository. The code is the page —
`buri docs lint <code>` reads it, `buri docs lint` lists every one, and the
lints section of this documentation is the same list.

What a repository decides is in `REPO.buri`'s [`lint`
block](../build/repo-config.md#lint), and it decides it once, for the whole
repository: `check_during_build` runs these checks during `buri build` and
`buri test`, `fail_on_finding` makes what they report fail the command, and
[`rules`](../build/repo-config.md#rules) turns a rule off by the name the
finding prints — `enabled(rule) = override.unwrap_or(default)`, with
`default: DISABLED` giving an allow list. There is still no per-rule severity,
no per-directory exemption and no per-file suppression comment: a rule is off
for the repository or it is on, so "is this rule on here" is answered by one
file rather than by whichever line somebody added to the file they were already
editing.

A rule turned off is dropped from the report, not downgraded, and never
silently: whenever this repository is not running the whole catalogue, the
report says so —

```
REPO.buri turns off 2 of 25 lint rules: discarded-result, hex-digit-table
no findings
```

— because a check that did not run, with nothing on the screen saying it did
not, is worse than the finding it was hiding.

Import order is not a lint. `buri format` sorts imports, so an unsorted import
run is not a finding to report — it is a file that has not been formatted.

## Exit status

`0` if there was nothing to report, `1` if there was anything at all. Severity
does not enter into it — every finding is a warning, a type error riding along
in the same report is an error, and both are `1`, because a warning is still the
answer to the question you asked. Running the linter is itself the request to be
told, and a report that exits zero is one no script can branch on, so `buri
lint //...` is usable directly as a gate with no flag to make it one.

`2` is not a report at all — it is the run that could not start: a target
pattern that names nothing, or a build file that does not read.

## When the code does not compile

The catalogue still runs, and the report holds both halves: the errors the front
end found, and every finding those errors cannot have caused. A file with a
mistake in one function is still a file with an import nothing uses, and holding
the second answer back until the first is fixed makes the tool tell you one
thing at a time about a file you are already looking at. A syntax error is an
error in the report like any other; the declarations the parser did recover are
analysed, and the file beside a broken one is still read.

What a mistake takes away is the tree under it. A rule that reads bodies goes
quiet for exactly the body that failed — the declaration the error landed in,
and no other — because reading a truncated tree would report the gap rather than
the code, calling a name unused because its only use went missing. A rule that
reads the source rather than the tree — parameter counts, nesting depth,
function length, warning comments, test titles, duplicate imports, unused
imports — answers the same either way.

The silence is one-directional: a finding may be missed inside a broken body,
and none is invented there. Fix the error and run the linter again to see what
the gap was hiding.

There is one file the linter does not read around, and it is a build file. A
`BUILD.buri` or `REPO.buri` that does not parse is the shape of the repository
rather than something in it: nothing downstream knows which files a package
holds or what it may see. So the run stops there and says which file it was,
rather than reporting a graph it had to guess at.

## `--fix`

```
buri lint //... --fix
```

Applies the findings that have exactly one mechanical answer, then runs the
whole check again from the files on disk and reports what is left. The count is
the linter's, not arithmetic — an edit can uncover a finding the first pass
could not see.

Two kinds of answer, and they are applied differently. A build file that
disagrees with the code is handed to `buri gen`, which already writes exactly
that file and preserves `tags`, `visibility`, `outputs`, and comments — so
`lint --fix` and `gen` cannot end up disagreeing about what a `BUILD.buri`
should say. A source edit is applied as bytes, one edit per statement rather
than one per name, because two adjacent unused names share the comma between
them.

Everything else is left alone and reported. A cycle has no mechanical answer —
which of the two edges to cut is a design decision — and a tool that picks one
is not fixing the finding, it is deleting the policy that raised it.

**`--fix` edits; it does not reformat.** It writes the bytes the findings name
and checks the result still parses. Running the file through the formatter would
answer that question too, and would also rewrite everything the fix did not
touch, which turns one deliberate edit into a diff nobody asked for. Run
`buri format` when you want the file formatted.

Where two edits in one file overlap, none of that file's are applied and the
findings are reported instead. Guessing which of two answers was meant is the
one thing a rewriting tool must not do.

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

A record holds what the *catalogue* found, and which rules this repository runs
is applied after it is read back. So a `rules` block that turns a rule off
cannot be defeated by a record written before it did, and turning one back on
cannot be answered from a record that never held it — which is also why editing
`REPO.buri` re-analyses everything, as the key below says.

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

Three things make a record unusable, and each of them ends in re-analysis rather
than in a stale answer: a file the record names moved, appeared, or went away; a
`BUILD.buri` or `REPO.buri` edit changed the key for every target, because a
build file decides what a closure *is*; or the toolchain moved, because the key
holds this `buri`'s version and a record an older one wrote is unreachable
rather than trusted.

`buri clean` drops the records with the rest of the cache. Two `buri lint` runs
on one repository are safe to overlap: a record is written to a temporary and
renamed into place, so a reader sees a whole one or none.
