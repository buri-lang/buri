# Your first program

In this tutorial we will create a Buri repository, run its tests, change the
one library in it, and watch that change come out on the terminal. It takes
about ten minutes.

You need `buri` on your `PATH` first — [installing](./installing.md) has the
three ways to get it.

## Create the repository

Run:

```text
$ buri init hello-buri
wrote REPO.buri
wrote .gitignore
wrote libs/greeting/BUILD.buri
wrote libs/greeting/lib.buri
wrote libs/greeting/greeting.buri
wrote libs/greeting/test/greeting.buri
wrote apps/hello/BUILD.buri
wrote apps/hello/main.buri
wrote .agent/skills/buri-language/SKILL.md
wrote .agent/skills/buri-types/SKILL.md
wrote .agent/skills/buri-build/SKILL.md
wrote .agent/skills/buri-testing/SKILL.md
wrote .agent/skills/buri-cli/SKILL.md
```

Move into it:

```text
$ cd hello-buri
```

Eight of those files are the repository; the five under `.agent/skills` are
the agent skills for this toolchain, so that a coding agent working here has
them.

## Look at what it wrote

`REPO.buri` marks the repository root: `//` in every label and every module
path resolves against the directory holding it. `libs/greeting/BUILD.buri`
declares one library, listing its sources and its tests one path at a time.
`libs/greeting/greeting.buri` holds that library's one function, `greeting`,
which answers `"hello world"`.

`apps/hello/main.buri` is the program. The `context` it builds is this
program's entire effect budget — it can allocate and it can print, and nothing
else:

```buri repo=cli/src/docs/init package=//apps/hello role=entry
from "core/effect" import { Alloc, Stdout };
from "core/host" import * as host;
from "core/io" import * as io;
from "//libs/greeting" import { greeting };

export fn main(): Result<(), Str> {
    let ctx = context {
        Alloc: host.alloc,
        Stdout: host.stdout,
    };

    match (io.println(ctx, greeting())) {
        .Ok(_) => .Ok(()),
        .Err(_) => .Err("could not write to standard output"),
    }
}
```

## Test it and run it

```text
$ buri test
1 passed, 0 failed, 0 skipped (0.4s)
$ buri run //apps/hello
hello world
```

(Timings vary, here and below.)

## Make the greeting ours

Open `libs/greeting/greeting.buri` and change the one string it answers:

```buri
/// The greeting this repository was born with.
export fn greeting(): Str {
    "hello, Buri"
}
```

Run the tests again:

```text
$ buri test
FAIL //libs/greeting  test/greeting.buri  "the greeting"
  assert.eq failed
    actual:   "hello, Buri"
    expected: "hello world"
  --> libs/greeting/test/greeting.buri:7:1

0 passed, 1 failed, 0 skipped (0.3s)
```

The suite is telling us the truth: we changed what the library answers and did
not change what we claim it answers.

## Make the test agree

Open `libs/greeting/test/greeting.buri` and change the expected string in its
one assertion to `"hello, Buri"`, so the line reads:

```text
    assert.eq(greeting(), "hello, Buri");
```

Run them once more, and run the program:

```text
$ buri test
1 passed, 0 failed, 0 skipped (0.5s)
$ buri run //apps/hello
hello, Buri
```

That is the whole loop: change the code, run the suite, run the program.

## Next

[Tutorial: a small program, end to end](./tutorial.md) builds a real command
line program from an empty directory — two libraries, a binary, and the tests
that hold them up.
