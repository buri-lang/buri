## What it does

```
buri lint //...
```

The static checks that are not type errors: sources declared but absent, a
source on disk that no rule names, a dependency that is declared and unused, one
that is used and undeclared, a visibility or tag violation, a cycle in the
package graph, and the hygiene rules — an import nothing uses, an `export`
nothing reaches, a type nothing names or builds, a field nothing reads, a
variant nothing constructs, a test that asserts nothing.

Each finding carries a stable code, so you can grep a report and name a specific
check when you talk about it. Every finding is a warning: one catalogue, one
severity, the same in every repository. The code is also the page. `buri docs
lint <code>` reads one, and `buri docs lint` lists every one.

A repository decides the rest once, in `REPO.buri`'s [`lint`
block](../build/repo-config.md#lint). `check_during_build` runs these checks
during `buri build` and `buri test`. `fail_on_finding` makes what they report
fail the command. [`rules`](../build/repo-config.md#rules) turns a rule off by
the name the finding prints: `enabled(rule) = override.unwrap_or(default)`, and
`default: DISABLED` gives you an allow list.
[`allow`](../build/repo-config.md#allow) is the smaller move: one named
declaration a rule is not asked about, with the rule still on everywhere else.
There is no per-rule severity, no per-directory exemption and no per-file
suppression comment, so one file answers "is this rule on here".

Turning a rule off drops it from the report rather than downgrading it, and
never quietly. Whenever this repository runs less than the whole catalogue over
the whole of its code, the report says so:

```
REPO.buri turns off 2 of 25 lint rules: discarded-result, hex-digit-table
REPO.buri exempts 1 declaration: too-many-parameters on //lib/wire:encode
no findings
```

Import order is not a lint. `buri format` sorts imports, so an unsorted import
run is a file nobody has formatted rather than a finding to report.

## What it reads

Every source a rule declares: `sources`, the `test { sources }` beside them, and
the `testing { sources }` a suite imports. A suite is code, so the linter reads
a suite, and `--fix` rewrites a test source and a testing source exactly as it
rewrites a library source.

Two rules answer differently in a test source, and neither is a skip:

- `dead-code` never fires there. A test source may not `export` and nothing may
  import one, so it holds no declaration the rule could ask about. The runner
  reaches a `test` declaration, which makes it a root by definition.
- `ctx-rebinding` never fires there. A test source is one of the few places you
  may *build* a context, so `let ctx = …` is the real thing — the same answer
  the rule gives inside `main`.

A `testing/` module has both. Its surface is `testing/lib.buri`, which decides
what leaves the test-only half exactly as `lib.buri` decides what leaves the
library. So `dead-code` reports an `export` that file does not carry, and
`unused-type`, `unused-field` and `unused-variant` leave alone what it does: the
suite that uses a fixture lives in a package this analysis never loaded.

## Exit status

`0` if there was nothing to report, `1` if there was anything at all. Every
finding is a warning, a type error riding along in the same report is an error,
and both exit `1`, because a warning still answers the question you asked. So
`buri lint //...` works directly as a gate, with no flag to make it one.

`2` is not a report at all. It is the run that could not start: a target pattern
that names nothing, or a build file that does not read.

## When the code does not compile

The catalogue still runs, and the report holds both halves: the errors the front
end found, and every finding those errors cannot have caused. The errors cover
the whole closure, tests included, so `buri lint` says more here than `buri
build` does. A bound naming an effect that no longer exists shows up in a
library source, in a testing source and in a key of a context a test source
builds, while a build compiles only the first of the three. A syntax error is an
error in the report like any other: the linter analyses the declarations the
parser did recover, and still reads the file beside a broken one.

What a mistake takes away is the tree under it. A rule that reads bodies goes
quiet for exactly the body that failed — the declaration the error landed in,
and no other — because reading a truncated tree would call a name unused because
its only use went missing. A rule that reads the source rather than the tree
answers the same either way: parameter counts, nesting depth, function length,
warning comments, test titles, duplicate imports, unused imports.

The silence runs one way. The linter may miss a finding inside a broken body,
and it never invents one there. Fix the error and run the linter again to see
what the gap was hiding.

There is one file the linter does not read around, and it is a build file. A
`BUILD.buri` or `REPO.buri` that does not parse is the shape of the repository
rather than something in it: nothing downstream knows which files a package
holds or what it may see. So the run stops there and names the file.

## `--fix`

```
buri lint //... --fix
```

Applies the findings that have exactly one mechanical answer, then runs the
whole check again from the files on disk and reports what is left. The count
comes from the linter rather than from arithmetic, because an edit can uncover a
finding the first pass could not see.

Two kinds of answer, applied differently. A build file that disagrees with the
code goes to `buri gen`, which already writes exactly that file and keeps
`tags`, `visibility`, `outputs`, and comments. A source edit lands as bytes, one
edit per statement rather than one per name, because two adjacent unused names
share the comma between them.

It leaves everything else alone and reports it. A cycle has no mechanical
answer, because which of the two edges to cut is a design decision.

**`--fix` edits; it does not reformat.** It writes the bytes the findings name
and checks that the result still parses. Running the file through the formatter
would rewrite everything the fix did not touch, turning one deliberate edit into
a diff nobody asked for. Run `buri format` when you want the file formatted.

Where two edits in one file overlap, it applies none of that file's edits and
reports the findings instead.

## What a second run costs

A finding depends on two things and no others: the build graph, and the bytes of
the files the target's analysis read. So the linter writes the answer down.
Under `.buri/cache`, beside the build's own entries, each target keeps one
record: the closure it read, each file with a hash of its bytes, and what the
catalogue found. A run that finds every one of those files holding the bytes the
record names reports what the record says. A run that finds any of them moved
analyses the target again and writes the record back.

That makes `buri lint //...` after a one-file edit re-analyse the targets whose
closure holds that file, and no others. The report is the same either way, to
the byte: a record carries findings and nothing else, so the code that sorts,
promotes and prints a fresh finding is what prints a cached one.

A record holds what the *catalogue* found. The linter applies which rules this
repository runs after it reads the record back, so a record written before a
`rules` block turned a rule off cannot defeat that block.

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
key covers the target and the build graph rather than the closure, so it does
*not* move when you edit a source. What moved is inside the record, and one
target keeps one entry however long you edit it.

Three things make a record unusable, and each ends in re-analysis rather than in
a stale answer. A file the record names moved, appeared, or went away. An edit
to a `BUILD.buri` or `REPO.buri` changed the key for every target, because a
build file decides what a closure *is*. Or the toolchain moved, because the key
holds this `buri`'s version.

`buri clean` drops the records with the rest of the cache. Two `buri lint` runs
on one repository can safely overlap: each record goes to a temporary file and
is renamed into place, so a reader sees a whole one or none.
