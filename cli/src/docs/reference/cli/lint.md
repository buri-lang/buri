## What it does

```
buri lint //...
```

Runs the static checks that aren't type errors: declared sources that are
missing, a source no rule names, a dependency that's unused or undeclared, a
visibility or tag violation, a package cycle, and the hygiene rules (an unused
import, an `export` nothing reaches, a type nothing names or builds, a field
nothing reads, a variant nothing constructs, a test that asserts nothing).

Every finding is a warning with a stable code, and the code names its page:
`buri docs lint <code>` reads one, `buri docs lint` lists them all.

`REPO.buri`'s [`lint` block](../build/repo-config.md#lint) sets the rest.
`check_during_build` runs these checks during `buri build` and `buri test`.
`fail_on_finding` makes a finding fail the command.
[`rules`](../build/repo-config.md#rules) turns a rule off by the name the finding
prints: `enabled(rule) = override.unwrap_or(default)`, so `default: DISABLED`
gives you an allow list. There's no per-rule severity, per-directory exemption or
suppression comment, so that one file answers "is this rule on here".

A rule that's off drops out of the report, and the report says so:

```
REPO.buri turns off 2 of 25 lint rules: discarded-result, hex-digit-table
no findings
```

Import order isn't a lint. `buri format` sorts imports.

## What it reads

Every source a rule declares: `sources`, the `test { sources }` beside them, and
the `testing { sources }` a suite imports. `--fix` rewrites test and testing
sources just like library sources.

Two rules never fire in a test source:

- `dead-code`, because a test source can't `export` and nothing imports one. The
  runner reaches every `test` declaration.
- `ctx-rebinding`, because a test source may build a context, just like `main`.

A `testing/` module's surface is `testing/lib.buri`. `dead-code` reports an
`export` that file doesn't carry, and `unused-type`, `unused-field` and
`unused-variant` leave alone what it does carry, since the suites that use a
fixture live in packages this run never loaded.

A JSON file a generator lists is checked against its schema, and a file in your
own language by its tool's `check`, just as `buri build` does. A failure there is
an error, not a finding, so it fails the run whatever `REPO.buri` says.

## Exit status

`0` if there's nothing to report, `1` if there's anything: a warning, or a type
error in the same report. So `buri lint //...` works as a gate with no extra
flag.

`2` means the run couldn't start: a target pattern that names nothing, or a
build file that doesn't read.

## When the code does not compile

The report holds the front end's errors plus every finding those errors can't
have caused. The errors cover the whole closure, tests included, so `buri lint`
reports more than `buri build`. A syntax error is just another error: the linter
analyses whatever declarations the parser recovered, and still reads the files
beside it.

A rule that reads bodies goes quiet for the one declaration an error landed in,
so a use that went missing never turns into "unused". Rules that read source
text answer the same either way: parameter counts, nesting depth, function
length, warning comments, test titles, duplicate and unused imports. The linter
may miss a finding in a broken body, but never invents one. Fix the error and
run it again.

A `BUILD.buri` or `REPO.buri` that doesn't parse stops the run and names the
file, since nothing downstream knows what a package holds.

## `--fix`

```
buri lint //... --fix
```

Applies every finding that has exactly one mechanical answer, then re-runs the
whole check from disk and reports what's left. An edit can uncover a new
finding, so the count comes from the linter, not subtraction.

- A build file that disagrees with the code goes to `buri gen`, which keeps
  `tags`, `visibility`, `outputs` and comments.
- A source edit lands as bytes, one per statement, because adjacent unused names
  share a comma.
- Anything else, like a cycle, is reported and left alone. Which edge to cut is
  your call.

`--fix` edits; it doesn't reformat. It writes only the bytes the findings name
and checks the result still parses. Run `buri format` for the rest. If two edits
in one file overlap, it applies none of that file's edits and reports the
findings instead.

## What a second run costs

A finding depends only on the build graph and the bytes of the files a target's
analysis read. So each target keeps one record under `.buri/cache`: the files it
read, a hash of each, and what the catalogue found. If every file still matches,
the run reports the record. Otherwise it analyses the target again and rewrites
the record. After a one-file edit, only targets whose closure holds that file
re-run, and the report is byte-identical either way.

The record holds what the whole catalogue found. `rules` applies after reading
it, so an old record can't bring back a rule you turned off.

```
buri lint //... --explain
```

```
cached lint //lib/money - 8b2e77c1904a
run    lint //cmd/web - 5fda356eb977
```

The fourth column is `-`, not a platform: a lint covers a target's whole closure
whatever it's built for. The key covers the target and the build graph, not the
files, so one target keeps one entry however much you edit.

A record goes unused, and the target re-analysed, when a file it names changed,
appeared or vanished, when any `BUILD.buri` or `REPO.buri` changed, or when the
`buri` version changed.

`buri clean` drops the records with the rest of the cache. Overlapping
`buri lint` runs are safe: each record is written to a temporary file and renamed
into place.
