# Tags, platforms, and policy

Two questions, checked two ways: *may this code end up in that program*, and
*which platforms may this code build for*. The first is a tag. The second is a
platform whitelist. Neither has a composition mode, a default, or a resolution
order.

**Tags mean the same thing on a library and on a binary.** There is no second
mechanism for entry points. A binary is a target, and it carries labels like any
other.

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
        platforms: [LINUX, MACOS]
    }
}
```

This is the split that matters. A build file states what its code *is*. The
repository states what follows from that. Adding a library that reuses an
existing tag never touches `REPO.buri`, and changing what `server` means never
touches a library.

Each block's name states its polarity, so anyone scanning `REPO.buri` sees at a
glance what a tag rules out and what it demands. Nobody has to remember which
field is which. Each takes exactly one kind of thing, and the
omissions are deliberate ([see below](#why-forbids-has-no-platforms)).

### The vocabulary is closed

Tags are one flat namespace. `tags: ["server"]` in a build file three
directories down resolves to that block and nowhere else. A name declared twice
is an error, never a silent win for whichever declaration came first.

**A tag that `REPO.buri` does not declare is an error.** `unknown-tag` reports
it and suggests the nearest declared name. There are no ad-hoc tags, and a
`tags` entry is never a harmless annotation.

The alternative, an undeclared tag meaning nothing, makes a typo the silent
difference between a checked build and an unchecked one. It fails in the bad
direction: `//lib/store` looks tagged, reviews as tagged, and links into the
browser build anyway. A closed vocabulary also means the tags in play are
exactly the tags in `REPO.buri`, so one file answers "what policies does this
repository have" and nobody greps the tree.

## `forbids { tags: [...] }`

Two tags that forbid each other may not appear anywhere in the same dependency
closure. That is the entire rule.

It is **symmetric**. Declaring that `server` forbids `client` says the same thing
as declaring that `client` forbids `server`. Write it once, on whichever tag
makes it easier to find. Policy about a restricted thing usually belongs on the
restricted thing.

The check runs at **every target**, not only at binaries:

> For a target `T`, let *closure(T)* be `T` together with everything reachable
> from it through `dependencies`. The union of all tags carried anywhere in
> *closure(T)* must contain no forbidden pair.

Two consequences follow.

**It is a union, not a path.** A binary that pulls in client-only code down one
dependency and server-only code down another is an error even though neither
reaches the other. It would still be one artifact containing both.

**Direction does not exist.** "Where may this code go" and "what is in this
binary" are the same reachability question asked from opposite ends. Deployment
tiers and license or data classification need one mechanism, not two that
compose differently. One walk checks both `server` forbidding `client` and
`experimental` forbidding `stable`.

## `requires { platforms: [...] }`

A platform is not a tag. You select a platform rather than merely constrain it,
because the compiler has to pick a backend. So it stays a typed field.

A binary names its platforms in `outputs`. A library names them only when it is
genuinely platform-specific, and writes them as a plain field, because a library
states facts rather than policy:

```textproto schema=build
# lib/posix_paths/BUILD.buri
library {
    platforms: [LINUX, MACOS]
    # this code does not mean anything on JS
}
```

**Unset means every platform, and unset is the overwhelmingly common case.** A
library has no opinion about platforms unless its code has one. `//lib/money`
and `//lib/ledger` in the example repository declare nothing and build
everywhere.

The same list appears under a tag's `requires`, with the same meaning, when the
restriction is policy across many libraries rather than a fact about one.
`server` requires `[LINUX, MACOS]` above, so every library tagged `server`
inherits that without repeating it.

It is a **whitelist**, never an exclusion. You write "anything but JS" by listing
what is allowed, which stays correct when the toolchain gains a platform. A
library written today does not silently acquire a WASM build tomorrow. The rule:

> *platforms(T)* is the intersection, over every target in *closure(T)*, of that
> target's `platforms` and the `requires.platforms` of every tag it carries,
> treating unset as "all". Each of a binary's `outputs` must name a platform in
> *platforms(binary)*.

Intersection, so restrictions accumulate downward. Depending on POSIX-only code
makes you POSIX-only. An empty intersection means nothing can ever build the
target, which is an error at the target rather than at whichever binary reaches
it first.

### Why `forbids` has no platforms

The blocks are not symmetric, and both missing combinations are missing on
purpose.

**`forbids { platforms: ... }` does not exist.** It would write the same
restriction as a negation, and a negation does not survive a new platform.
`server` forbidding JS silently permits WASM the day WASM arrives, while
`server` requiring linux and macos keeps meaning what its author meant. Every
platform restriction is a whitelist, so there is one place to write one.

**`requires { tags: ... }` does not exist.** It reads plausibly, as "everything
under this must also be server code", and it is unusable. Most targets carry no
tags at all, so the rule would force `server` transitively onto every library in
the repository, and the tag would stop distinguishing anything. Weaken it to
what people actually mean, "nothing under this may be *incompatible*", and you
have `forbids { tags: ... }`, which already exists.

## Outputs

```textproto schema=build
# cmd/server/BUILD.buri
binary {
    tags: ["server"]
    outputs: [
        { platform: LINUX, arch: X86_64 },
        { platform: MACOS, arch: ARM64 },
    ]
}
```

Each entry is a separate artifact and a separate check of the whole graph,
because each names a different platform. `buri build //cmd/server` builds both,
and `--output=linux/x86_64` picks one. The tag check does not vary between them,
so it runs once.

## What a failure reports

A `tag-violation` names both tags, the target carrying each, the path that
reaches it, and each tag's `doc`. It prints the path because in a repository of
any size the interesting question is never "which library is tagged `server`"
but "who dragged it in." It prints the `doc` strings for the same reason: a tag
is a policy, and a policy should say why. A `platform-violation` reports the
same way. [Enforce policy with tags](../../guides/tags-policy.md) walks one of
each.

Two rules, two diagnostics, and the second is why the first is not load-bearing.
A repository states its deployment policy on a tag once and gets both. A target
that drops the offending tag can still fail on the platform whitelist the other
tag carries.

An unsatisfiable target, one carrying `client` and depending on something tagged
`server`, reports the same way at the *library* itself, before any binary asks
for it. Catching it there is worth the extra pass. Otherwise the mistake
surfaces as a confusing failure in whichever binary happens to reach it first.

A `stable` binary reaching an `experimental` library reads identically, which is
the point: one mechanism and one diagnostic shape, whether the question is
deployment or maturity. `stable` is opt-in, and nothing is defaulted. A binary
that says nothing about maturity gets no maturity check, and a binary that
refuses to ship unfinished code says so. That loses real enforcement compared to
a mandatory axis, and it buys a design with no resolution algorithm to reason
about.

## Tags and tests

A test suite inherits its target's tags and platform restrictions, so a suite for
a `server` library gets checked as server code without saying anything.

By default a suite runs once, on the host platform, as a native binary. A suite
that must run in more than one lists them:

```textproto schema=build
# lib/codec/BUILD.buri
library {
    sources: ["codec.buri"]

    test {
        sources: ["test/codec.buri"]

        # One run per platform; the JS run goes through the JS backend.
        platforms: [LINUX, JS]
    }
}
```

That is how you write "this must behave identically on both backends". For a
language targeting a native binary and JavaScript, that is the test you most
want. `I64` on the JS target ([a `BigInt`, not a
`number`](../../guides/compile-to-js.md)) is the standing reason it exists. A
platform listed here must be one the target admits. Asking for a JS run of a
`[LINUX, MACOS]` library is an error, not a skip.

A native platform runs as a native binary wherever this toolchain can build one,
which means the host's own platform. There is no cross-compilation, so a `LINUX`
run happens on a Linux machine and a `MACOS` run on a Mac. The runner refuses
the other with `platform-not-implemented` rather than quietly running it through
JavaScript.

A suite that names no platforms also runs on the host natively, and there is no
second answer. Where this toolchain cannot build a binary for the host, or where
the suite's program reaches something the backend has no body for yet, the
runner **refuses**: `native-run-not-available` for the first, and a message
naming the intrinsic and the backend for the second. It reroutes nothing. A
suite that ran on a backend nobody chose would report a pass about the other
backend, and the line saying so would go to a stream that a green run's reader
does not read.

A declared platform and the default differ only in how the refusal reads. A
suite that wrote `platforms` down has somewhere to delete the request from, and
a suite that wrote none does not. Both refusals name the two ways to ask for
JavaScript: `test { platforms: [JS] }` in the build file, and `buri test
--output=js` for a whole invocation without editing one.

### One binary for several suites

Compiling a small suite is almost none of its cost. The charges are one `cc`
invocation and one first execution of a file the operating system has never run,
and you pay both per binary rather than per test. So `buri test` compiles the
suites that name no platform into **one binary per tag-compatible batch**, links
it once, and runs it once.

Tag-compatible is this chapter's own rule applied to the union. A batch is one
artifact, so two tags that forbid each other may not both be in it. A `client`
suite and a `server` suite are therefore two binaries, however convenient one
would have been. The tags that count are those of the suite's production closure
*and* of its `test { dependencies }`, everything the binary would actually link.
Three more conditions keep a suite out of a batch, and each marks a way two
suites would disagree about what building or running them means: a declared
`test { platforms }`, which is a request served on its own; a declared
`timeout_seconds`, since one suite's limit would become everybody's in a shared
process; and `--output=` on the invocation.

Nothing about the result changes. Each suite still has its own cache key, its own
cached verdict, and its own report. A suite whose verdict is already cached never
enters the batch. One suite's failure is an abort and takes its process with it,
but it costs that suite's test and no other suite's report, because the runner
resumes at the block after the one that aborted. If anything at all makes a batch
doubtful, from a type error to an intrinsic the backend has no body for, `buri
test` abandons the batch and compiles, links and runs every suite in it on its
own. There a diagnostic can name the one suite it belongs to.

## What tags are not

- **Not a boolean expression language.** A tag declaration has one list of
  forbidden tags and one whitelist of platforms. There is no `or`, no nesting,
  and no expression that mentions three tags at once. If you cannot write a rule
  as "these two may not coexist," it is probably a visibility rule.
- **Not conditional compilation.** No source file changes meaning across
  platforms, and there is no `#if`. A library that needs two implementations
  becomes two libraries with different `platforms` and one dependent that picks.
  The choice shows up in the build graph rather than hiding in a file.
- **Not a substitute for visibility.** Visibility answers "who may write this
  dependency edge", one edge at a time. Tags answer "what may end up in one
  artifact", over the whole closure. Use visibility for API ownership, tags for
  deployment boundaries.
- **Not an axis system.** There are no dimensions, so nothing requires a binary
  to state a tier, and nothing is resolved or defaulted. A tag is either present
  in a closure or it is not.
