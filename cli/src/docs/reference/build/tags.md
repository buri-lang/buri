# Tags, platforms, and policy

Two questions, checked two ways: *may this code end up in that program*, and
*which platforms may this code build for*. The first is a tag. The second is a
pair of lists, `backends` and `platforms`, either a whitelist or an exclusion. Neither has a composition
mode, a default, or a resolution order.

**Tags mean the same thing on a library and on a binary.** There is no second
mechanism for entry points.

## Tags are labels; policy lives on the tag

A target's `tags` are facts about it. They say nothing on their own:

```textproto schema=build
# lib/store/BUILD.buri
library {
    tags: ["server"]
    # this library is server code
}
```

What that *costs* you is declared once, in `REPO.buri`:

```textproto ignore why="a fragment of a build file, not a whole one"
tag {
    name: "server"
    doc: "runs on infrastructure we operate"

    forbids {
        tags: ["client"]
    }

    requires {
        backends: [NATIVE]
    }
}
```

A build file states what its code *is*. The repository states what follows from
that. Adding a library that reuses an existing tag never touches `REPO.buri`,
and changing what `server` means never touches a library.

Each block's name states its polarity, so anyone scanning `REPO.buri` sees at a
glance what a tag rules out and what it demands. `forbids` takes tags,
backends and platforms. `requires` takes only backends and platforms, and that
omission is deliberate
([see below](#why-requires-has-no-tags)).

### The vocabulary is closed

Tags are one flat namespace. `tags: ["server"]` in a build file three
directories down resolves to that block and nowhere else. A name declared twice
is an error.

**A tag that `REPO.buri` does not declare is an error.** `unknown-tag` reports
it and suggests the nearest declared name. There are no ad-hoc tags, so a typo
can never be the silent difference between a checked build and an unchecked one,
and one file answers "what policies does this repository have".

## `forbids { tags: [...] }`

Two tags that forbid each other may not appear anywhere in the same dependency
closure. That is the entire rule.

It is **symmetric**. Declaring that `server` forbids `client` says the same thing
as declaring that `client` forbids `server`. Write it once, on whichever tag
makes it easier to find.

The check runs at **every target**, not only at binaries:

> For a target `T`, let *closure(T)* be `T` together with everything reachable
> from it through `dependencies`. The union of all tags carried anywhere in
> *closure(T)* must contain no forbidden pair.

Two consequences follow.

**It is a union, not a path.** A binary that pulls in client-only code down one
dependency and server-only code down another is an error even though neither
reaches the other. It would still be one artifact containing both.

**Direction does not exist.** "Where may this code go" and "what is in this
binary" are the same reachability question asked from opposite ends. One walk
checks both `server` forbidding `client` and `experimental` forbidding `stable`.

## `requires { backends: [...], platforms: [...] }`

A platform is not a tag. You select a platform rather than merely constrain it,
because the compiler has to pick a backend. So it stays a field of its own.

Two lists say where code may go. `backends` names how it is compiled, `NATIVE`
or `JS`, for code that relies on one backend's behaviour, and admits every
platform that backend builds, a repository's own included. `platforms` names
bundled platforms, `"native"`, `"node"` or `"web"`, or a repository platform by
label, `"//platform/cloudflare_worker"`, for code that means something on one
platform only. A platform must satisfy every list written, and a label that
names no `platform` rule is `unknown-platform`.

A binary names its platforms in `outputs`. A library names them only when it is
genuinely platform-specific, and writes them as a plain field:

```textproto schema=build
# lib/posix_paths/BUILD.buri
library {
    backends: [NATIVE]
    # this code does not mean anything in JavaScript
}
```

```textproto schema=build
# lib/edge_cache/BUILD.buri
library {
    platforms: ["//platform/cloudflare_worker"]
    # only the worker's outputs may hold this
}
```

**Unset means every platform, and unset is the overwhelmingly common case.**
`//lib/money` and `//lib/ledger` in the example repository declare nothing and
build everywhere.

The same list appears under a tag's `requires`, with the same meaning, when the
restriction is policy across many libraries rather than a fact about one.
`server` requires `backends: [NATIVE]` above, so every library tagged `server`
inherits that without repeating it.

It is a **whitelist**. A platform the toolchain gains later stays out until
someone adds it.

## `forbids { backends: [...], platforms: [...] }`

The opposite polarity: code carrying the tag may not be built, or tested, for
these platforms, and every other platform stays open.

```textproto ignore why="a fragment of a build file, not a whole one"
tag {
    name: "legacy"
    doc: "wraps the old storage driver, which has no JavaScript port"

    forbids {
        backends: [JS]
    }
}
```

Pick the list by what should happen when the toolchain gains a platform. A
forbid list admits it the day it arrives. `legacy` above means "anywhere but
JS", and a new platform is "anywhere". A `requires` list keeps it out until you
add it, so use `requires` when the new platform should be a decision rather
than a default.

One tag may carry both. It admits what its `requires` admits, or every
platform when that is unset, minus what its `forbids` names. Naming the same
backend or platform in both is `tag-platform-conflict`, and naming one
twice in a list is `tag-duplicate-platform`.

## The platform rule

> *platforms(T)* is the intersection, over every target in *closure(T)*, of that
> target's `backends` and `platforms` and what every tag it carries admits. Each of a binary's
> `outputs` must name a platform in *platforms(binary)*.

Intersection, so restrictions accumulate downward. Depending on POSIX-only code
makes you native-only, and depending on `legacy` code takes JavaScript away. An empty
intersection means nothing can ever build the target, which is an error at the
target rather than at whichever binary reaches it first.

### Why `requires` has no tags

**`requires { tags: ... }` does not exist.** Most targets carry no tags at all,
so the rule would force `server` transitively onto every library in the
repository. Weaken it to what people actually mean, "nothing under this may be
*incompatible*", and you have `forbids { tags: ... }`.

## Outputs

```textproto schema=build
# cmd/server/BUILD.buri
binary {
    tags: ["server"]
    outputs: [
        { platform: "native", variant: "linux-x86_64" },
        { platform: "native", variant: "macos-arm64" },
    ]
}
```

Each entry is a separate artifact and a separate check of the whole graph,
because each names a different platform. `buri build //cmd/server` builds both,
and `--output=native/linux-x86_64` picks one. The tag check does not vary between them,
so it runs once.

## What a failure reports

A `tag-conflict` names both tags, the target carrying each, the path that
reaches it, and each tag's `doc`. It prints the path because the interesting
question is never "which library is tagged `server`" but "who dragged it in." A
`platform-violation` reports the same way. [Enforce policy with
tags](../../guides/tags-policy.md) walks one of each.

An unsatisfiable target, one carrying `client` and depending on something tagged
`server`, reports the same way at the *library* itself, before any binary asks
for it. Otherwise the mistake surfaces as a confusing failure in whichever
binary happens to reach it first.

A `stable` binary reaching an `experimental` library reads identically: one
mechanism and one diagnostic shape, whether the question is deployment or
maturity. `stable` is opt-in, and nothing is defaulted. A binary that says
nothing about maturity gets no maturity check.

## Tags and tests

A test suite inherits its target's tags and platform restrictions, so a suite for
a `server` library gets checked as server code without saying anything.

By default a suite runs once, natively, on the host. A suite that must run on
more than one backend lists them:

```textproto schema=build
# lib/codec/BUILD.buri
library {
    sources: ["codec.buri"]

    test {
        sources: ["test/codec.buri"]

        # One run per backend.
        backends: [NATIVE, JS]
    }
}
```

That is how you write "this must behave identically on both backends". `I64` on
JavaScript ([a `BigInt`, not a `number`](../../guides/compile-to-js.md)) is the
standing reason it exists. The platform each run builds must be one the target
admits. Asking for a JS run of a `backends: [NATIVE]` library, or of a `legacy`
one, is an error, not a skip.

A `NATIVE` run builds the host's own variant, because a suite has to run where
it was built. A `JS` run builds the first JavaScript platform the target's
outputs name, or else the first it admits: `node`, then `web`. A target that
admits only a repository platform is checked against that platform on its
backend, so a library limited to `//platform/cloudflare_worker` tests with
`backends: [JS]`.

A suite that names no backends also runs on the host natively. Where this
toolchain cannot build a binary for the host, or where the suite's program
reaches something the backend has no body for yet, the runner **refuses**:
`test-run-unavailable` for the first, and a message naming the intrinsic and
the backend for the second. It reroutes nothing, because a suite that ran on a
backend nobody chose would report a pass about the other backend.

The two refusals say different things, because the two are different problems.
`test-run-unavailable` is about your toolchain, so it names the two ways to
ask for JavaScript: `test { backends: [JS] }` in the build file, and `buri test
--output=js` for a whole invocation. A missing body is about the *toolchain's*
gap rather than yours, so it says to report it — a program the front end
accepted is one the backend should compile. `--output=js` gets you moving in the
meantime, and it is a workaround rather than the answer.

### One binary for several suites

Compiling a small suite is almost none of its cost. The charges are one `cc`
invocation and one first execution of a file the operating system has never run,
and you pay both per binary rather than per test. So `buri test` compiles the
suites that name no platform into **one binary per tag-compatible batch**, links
it once, and runs it once.

Tag-compatible is this chapter's own rule applied to the union. A batch is one
artifact, so two tags that forbid each other may not both be in it. A `client`
suite and a `server` suite are therefore two binaries. The tags that count are
those of the suite's production closure *and* of its `test { dependencies }`,
everything the binary would actually link. Three more conditions keep a suite
out of a batch: a declared `test { backends }`, which is a request served on
its own; a declared `timeout_seconds`, since one suite's limit would become
everybody's in a shared process; and `--output=` on the invocation.

A batch is type checked once, but it can still become several binaries:

- Suites that take snapshots into different packages' `test/__snapshots__` get
  different binaries, because one process gets one snapshot directory.
- A binary holds about 128 MB of code at most, so the binaries build side by
  side. Set `BURI_TEST_BATCH_BYTES` to change the limit.

Each suite in a binary runs in processes of its own, a few tests to a process,
so a batch's suites run side by side too.

Nothing about the result changes. Each suite still has its own cache key, its own
cached verdict, and its own report. A suite whose verdict is already cached never
enters the batch. One suite's failure is an abort and takes its process with it,
but it costs that suite's test and no other suite's report, because the runner
resumes at the block after the one that aborted. A suite that fails to type
check leaves the batch, and the rest are batched without it. If anything else
makes a batch doubtful, such as an intrinsic the backend has no body for, `buri
test` abandons the batch and compiles, links and runs every suite in it on its
own, where a diagnostic can name the one suite it belongs to. A suite whose
process ends without a verdict for every test runs on its own the same way.

## What tags are not

- **Not a boolean expression language.** A tag declaration has a list of
  forbidden tags, lists of forbidden backends and platforms, and whitelists of
  both.
  There is no `or`, no nesting, and no expression that mentions three tags at
  once. If you cannot write a rule
  as "these two may not coexist," it is probably a visibility rule.
- **Not conditional compilation.** No source file changes meaning across
  platforms, and there is no `#if`. A library that needs two implementations
  becomes two libraries with different `backends` or `platforms` and one
  dependent that picks.
- **Not a substitute for visibility.** Visibility answers "who may write this
  dependency edge", one edge at a time. Tags answer "what may end up in one
  artifact", over the whole closure.
- **Not an axis system.** There are no dimensions, so nothing requires a binary
  to state a tier, and nothing is resolved or defaulted. A tag is either present
  in a closure or it is not.
