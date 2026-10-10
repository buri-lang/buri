# Test coverage

```sh
buri test //... --coverage
```

prints a branch count per file after the verdicts and writes
`.buri/coverage/lcov.info`:

```text
3 passed, 0 failed, 0 skipped (0.4s)

branch coverage
  lib/shapes/shapes.buri   3/5   60.0%
  total                    3/5   60.0%
lcov: .buri/coverage/lcov.info
```

`--coverage=branch` is the same. The lcov file has `BRDA`/`BRF`/`BRH` for
branches and `DA`/`LF`/`LH` for lines. A file counts when a suite loads it as
ordinary source: the standard library, the bundled platforms, generated
modules and test sources never do. Paths are the source map's
repository-relative names.

## Which branches count

`middle::coverage::decisions` is the one definition. A decision and its
branches, in branch order:

- `if`: then, else;
- `match`: each arm;
- a guard: true, false;
- `&&`, `||`: the right side ran, it didn't;
- `?`: went on, returned early.

The `match` `semantics` writes for `a < b` isn't one: its arms carry the
operator's span, which `float_operator` already relies on. Derived code and
`hand_written` bodies are built in the middle end, so they're never in
`Checked::bodies` and never in the universe. `else` is mandatory, so an `if`
always has both branches, and there is no `if let` or `let ... else`.

A decision is named by its kind and span, so monomorphized copies and suites
that load the same file share it. Its lcov line is where the choice is made: the
`if`, the `match`, the guard, the right side of `&&`/`||`, the `?`. The block
number is its place among the decisions on that line, ordered by that position.
A decision whose branches all count zero was never reached and reads `-`.

## MC/DC

`--coverage=mcdc` sets `Flags::mcdc` and keys its builds apart from
`--coverage`'s. On top of the branches above:

- **Conditions.** `middle::coverage::Tree` is an `&&`/`||`/`!` chain, which
  always has two conditions or more. Each condition adds to a number that only
  its path reaches: a true path of `a && b` is `a·T(b) + b`, a false one
  follows the true ones, and so on up the tree. The decision becomes
  `{ let p = <path>; coverage.hit(base + p); p < T }`, so a path is a key.
  `Tree::decode` turns a path back into each condition's value, or `None` where
  short-circuiting skipped it. The report pairs the paths that ran:
  unique-cause MC/DC with masking. The right sides of `&&` and `||` come from the
  same paths, with no probe of their own. A tree of more than 2³² paths keeps
  its branch probes and isn't counted, and so does one in tail position whose
  last condition can end in a call (`ends_in_tail_call`): numbering its path
  reads the call's answer, which takes the call out of tail position.
- **Instantiations.** `monomorphize::Program::instances` names the declaration
  and type arguments of each generic instantiation. A key carries the
  instantiation's name, so each one counts apart. A generic function no suite
  instantiated is `(never instantiated)`.
- **Aborts.** An integer `/` or `%`, or a call of a `core/bits` shift, counts
  how often it was reached and how often execution went on, like `?`.
- **Derived code** (`coverage::derived`). Each call of a derived `Equal`,
  `Ordered`, `Show`, `Hash` or `ToJson` on a type the user derives it for first
  calls a shadow function: it walks the value as the derived code does and
  counts its decisions, and the program's call runs unchanged after it. The
  decisions sit on the `derive`'s line, named for the operation and type, as in
  `Equal Point, x: same`.

lcov gets `MCDC` records, one group a line, because lcov tells groups on a
line apart only by size, and `MCF`/`MCH`. lcov 2.4's man page says `MRF`/`MRH`;
its parser reads `MCF`/`MCH`.

## Which lines count

A line counts when a **site** starts on it. `middle::coverage::sites` is the
one definition:

- a function's body;
- each statement of a block, and its tail;
- each branch of an `if`;
- each arm of a `match`;
- a lambda's body.

A multi-line expression counts on the line it starts on.

## Where the probes go

Between monomorphization and `middle::run`, once, so all three backends get
the same probes and inlining copies them along with the code they guard.
`middle::coverage::instrument` puts a probe in front of every site in a user
file, then `middle::coverage::branches` one on every branch:

```text
coverage.hit(key)    // an inline intrinsic, like `test.leave`
```

A line's `key` is the file name and line, and a branch's is the file name,
the decision's kind and span, and the branch, each hashed to 53 bits so a
JavaScript number holds it exactly. Two probes on one line share a key and add
up.

- `if` and `match` get a probe at the top of each branch.
- `&&`, `||` and a guard become an `if` with a probe on each side.
- `?` has nowhere to put a probe on its early return. It becomes
  `{ let b = x; came back; let v = b?; went on; v }`, and the report takes
  "returned early" as "came back" less "went on".

Lines go first, so the line pass never sees the `if`s the branch pass writes.

- **Native.** A runtime table row, `buri_rt_coverage_hit(key)`. The runtime
  adds to a table behind a mutex and registers an `atexit` handler on the first
  hit. An abort leaves through `exit`, so a failing test's branches still count.
- **JavaScript.** `$coverage_hit`, with a `process.on("exit")` handler.
  `emit_test_bundle` appends it to a coverage build's bundle
  (`build::coverage::JS`). It isn't in `runtime.js`, because a test bundle
  carries the whole runtime unminified.

A plain build never runs the pass, never names the key, and emits the same
bytes it did before.

## How counts come back

`buri` sets `BURI_COVERAGE=<root>/.buri/coverage/raw` on every process it
spawns during a coverage run. Each test process writes one new file there on
exit, one `key count` line per key it hit. Results stay on stdout untouched,
so verdicts, output and exit codes match a plain run.

Each suite's front end also hands over its **universe**: every site line and
every decision in every user function it checked, from `Checked::bodies`, so a
function nothing monomorphized still has them. After the last suite reports,
`buri` reads every raw file, sums counts across processes and suites, puts them
against the universe, and writes the report. A key outside the universe is
dropped.
## Caching

A coverage run re-runs every suite. Serving a cached verdict would serve no
counts, and a recorded build may predate a whitespace edit that moved every
line.

- `suite_key` gets a `coverage` term, so plain and instrumented builds and
  verdicts never share a cache entry.
- Codegen units are keyed on their IR, which holds the probes.
- The verdict cache and the build records aren't read during a coverage run.
- Suites don't batch, so each suite's front end sees its own program.

`--watch` and `--coverage` together are refused at parsing.

## LLVM

`--release` reads the same runtime table, so the probe needs nothing from it.
`buri test --release --coverage` works wherever `--release` does.

## The standard library's gate

```sh
cargo run -p buri-stdlib-coverage              # measure and check
cargo run -p buri-stdlib-coverage -- --bless   # and rewrite the baseline
```

`BURI_COVERAGE_STD=1` counts the standard library as if it were the
repository's own source. It's a variable rather than a flag because only this
gate wants it; the platforms stay out either way.

The gate runs the conformance corpus under `--coverage=mcdc` four times:
JavaScript, JavaScript `--release`, the stencil backend and LLVM `--release`.
LLVM builds only under `--release`, so there's no LLVM debug run. The corpus
pins its suites to JavaScript, so the native runs move each one to the native
backends, except the six packages they can't build yet. The two JavaScript runs
must count every module alike, and so must the two native ones, or the gate
fails: a difference is a coverage bug. Their records merge into `coverage/stdlib.txt`, every module's conditions,
decisions and branches and the 30 with the most unshown conditions, and
`coverage/stdlib-mcdc.txt`, the baseline: conditions shown per module.

A smaller share of conditions shown, in total or in any module, fails. A module
that's new, or that has no conditions, has nothing to fall from, though a new
one's conditions count in the total. A larger share
anywhere is a rise: on main, CI's `stdlib-coverage` job commits the new baseline
itself, as the compiler's coverage job does, and elsewhere it prints the commit.
The baseline is x86_64 Linux's, so on any other host the gate prints its
numbers and compares nothing.

Each instantiation of a generic function counts on its own, as under
`--coverage=mcdc` anywhere, so a test that calls `list.map` at a new type adds
that instantiation's conditions.
