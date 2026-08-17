# Tags, platforms, and policy

Two questions look different and are checked differently: *is this code allowed
to end up in that program*, and *which platforms can this code be built for*.
The first is a tag. The second is a platform whitelist. Neither has a
composition mode, a default, or a resolution order.

**Tags mean the same thing on a library and on a binary.** There is no second
mechanism for entry points: a binary is a target, and it carries labels like any
other.

## Tags are labels; policy lives on the tag

A target's `tags` are facts about it. They say nothing on their own:

```textproto schema=build
# lib/store/BUILD.buri
library {
  tags: ["server"]        # this library is server code
}
```

What that *costs* you is declared once, in `REPO.buri`:

```textproto ignore why="a fragment of a build file, not a whole one"
tag {
  name: "server"
  doc: "runs on infrastructure we operate"

  forbids { tags: ["client"] }

  requires { platforms: [LINUX, MACOS] }
}
```

This is the split that matters. A build file states what its code *is*; the
repository states what follows from that. Adding a library that reuses an
existing tag never touches `REPO.buri`, and changing what `server` means never
touches a library.

The two blocks are named for their polarity so that a reader scanning
`REPO.buri` can see at a glance what a tag rules out and what it demands,
without having to remember which field is which. Each takes exactly one kind of
thing, and the omissions are deliberate — [see below](#why-forbids-has-no-platforms).

### The vocabulary is closed

Tags are one flat namespace. `tags: ["server"]` in a build file three
directories down resolves to that block and nowhere else, so a name declared
twice is rejected rather than quietly meaning whichever came first.

**A tag that `REPO.buri` does not declare is an error.** There are no ad-hoc
tags, and a `tags` entry is never a harmless annotation:

```
error: unknown tag "sever"
  --> lib/store/BUILD.buri:22:10
   |
22 |   tags: ["sever"]
   |          ^^^^^^^
   |
   = no `tag` block in REPO.buri declares this name
   = did you mean "server"?
```

The alternative — an undeclared tag meaning nothing — makes a typo the silent
difference between a checked build and an unchecked one, and the failure mode is
the bad direction: `//lib/store` looks tagged, reviews as tagged, and is linked
into the browser build anyway. It also means the set of tags in play is exactly
the set in `REPO.buri`, so the question "what policies does this repository
have" is answered by reading one file rather than grepping the tree.

## `forbids { tags: [...] }`

Two tags that forbid each other may not appear anywhere in the same dependency
closure. That is the entire rule.

It is **symmetric**. Declaring `server` forbids `client` is the same statement as
declaring `client` forbids `server`; write it once, on whichever tag makes it
easier to find. Policy about a restricted thing usually belongs on the
restricted thing.

The check runs at **every target**, not only at binaries:

> For a target `T`, let *closure(T)* be `T` together with everything reachable
> from it through `dependencies`. The union of all tags carried anywhere in
> *closure(T)* must contain no forbidden pair.

Two consequences worth stating outright.

**It is a union, not a path.** A binary that pulls in client-only code down one
dependency and server-only code down another is an error even though neither
reaches the other. It would still be one artifact containing both.

**Direction does not exist.** "Where may this code go" and "what is in this
binary" — deployment tiers and license or data classification — are the same
reachability question asked from opposite ends, so they need one mechanism, not
two that compose differently. `server` forbidding `client` and `experimental`
forbidding `stable` are checked by the same walk.

## `requires { platforms: [...] }`

A platform is not a tag, because a platform is selected rather than merely
constrained: the compiler must pick a backend. It stays a typed field.

A binary names its platforms in `outputs`. A library names them only if it is
genuinely platform-specific, and writes them as a plain field, since a library
states facts rather than policy:

```textproto schema=build
# lib/posix_paths/BUILD.buri
library {
  platforms: [LINUX, MACOS]   # this code does not mean anything on JS
}
```

**Unset means every platform, and unset is the overwhelmingly common case.** A
library has no opinion about platforms unless it is doing something that has
one. `//lib/money` and `//lib/ledger` in the example repository declare nothing
and build everywhere.

The same list appears under a tag's `requires`, with the same meaning, for when
the restriction is policy spanning many libraries rather than a fact about one:
`server` requires `[LINUX, MACOS]` above, so every library tagged `server`
inherits that without repeating it.

It is a **whitelist**, never an exclusion. "Anything but JS" is written by
listing what is allowed, which stays correct when a platform is added to the
toolchain — a library written today does not silently acquire a WASM build
tomorrow. The rule:

> *platforms(T)* is the intersection, over every target in *closure(T)*, of that
> target's `platforms` and the `requires.platforms` of every tag it carries —
> treating unset as "all". Each of a binary's `outputs` must name a platform in
> *platforms(binary)*.

Intersection, so restrictions accumulate downward: depending on POSIX-only code
makes you POSIX-only. An empty intersection means the target can never be built,
which is an error at the target rather than at whichever binary reaches it
first.

### Why `forbids` has no platforms

The blocks are not symmetric, and the two missing combinations are missing on
purpose rather than by omission.

**`forbids { platforms: ... }` does not exist.** It would be the same restriction
written as negation, and negation does not survive a new platform: `server`
forbidding JS silently permits WASM the day WASM is added, while `server`
requiring linux and macos keeps meaning what its author meant. Every platform
restriction is a whitelist, so there is one place to write one.

**`requires { tags: ... }` does not exist.** It reads plausibly — "everything
under this must also be server code" — and it is unusable. Carrying no tags at
all is the common case, so the rule would force `server` onto every library in
the repository transitively, and the tag would stop distinguishing anything.
Weakened to what people actually mean by it — "nothing under this may be
*incompatible*" — it is precisely `forbids { tags: ... }`, which already exists.

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
because each names a different platform. `buri build //cmd/server` builds both;
`--output=linux/x86_64` picks one. The tag check does not vary between them, so
it runs once.

## What a failure looks like

Take the [`example/`](./cli/tests/example/) repository, and suppose `//lib/ledger` grows a
dependency on `//lib/store` — a reasonable-looking edge, added by someone who was
not thinking about the browser build:

```
//cmd/web          tags: ["client"], outputs: [{ platform: JS }]
  └─ //lib/ledger  tags: []            <- the new edge is here
       └─ //lib/store  tags: ["server"]
```

```
error: //cmd/web cannot contain both "client" and "server" code
  --> cmd/web/BUILD.buri:23:9
   |
23 |   tags: ["client"]
   |         ^^^^^^^^^^
   |
   = "client" is carried by //cmd/web itself
   = "server" is carried by //lib/store
       reached by: //cmd/web -> //lib/ledger -> //lib/store
       the edge that introduces it: lib/ledger/BUILD.buri:9 deps "//lib/store"
   = "client": ships to a user's machine or browser
   = "server": runs on infrastructure we operate
```

The path is printed because in a repository of any size the interesting question
is never "which library is tagged `server`" but "who dragged it in." The `doc`
strings are printed for the same reason: the tag is a policy, and the policy
should say why.

Dropping `tags: ["client"]` does not rescue this build, because `server` is also
restricted by platform:

```
error: //cmd/web cannot be built for js
  --> cmd/web/BUILD.buri:26:5
   |
26 |     { platform: JS, js { module: ESM } },
   |     ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^
   |
   = //lib/store is tagged "server", which requires linux, macos
   = reached by: //cmd/web -> //lib/ledger -> //lib/store
   = "server": runs on infrastructure we operate
```

Two rules, two diagnostics, and the second one is the reason the first is not
load-bearing: a repository states its deployment policy on the `server` tag once
and gets both.

An unsatisfiable target is an error at the target itself, before any binary asks
for it:

```
error: //lib/edge_cache can never be built
  --> lib/edge_cache/BUILD.buri:4:9
   |
 4 |   tags: ["client"]
   |         ^^^^^^^^^^
   |
   = it carries "client", and depends on //lib/store, which carries "server"
   = "client" and "server" forbid each other
```

Catching that at the library is worth the extra pass: otherwise the mistake
surfaces as a confusing failure in whichever binary happens to reach it first.

Maturity reads identically, which is the point:

```
error: //cmd/server cannot contain both "stable" and "experimental" code
  --> cmd/server/BUILD.buri:15:9
   |
15 |   tags: ["stable"]
   |         ^^^^^^^^^^
   |
   = "stable" is carried by //cmd/server itself
   = "experimental" is carried by //lib/vector_index
       reached by: //cmd/server -> //lib/search -> //lib/vector_index
   = "experimental": API may change without a deprecation period
```

Note that `stable` is opt-in. Nothing is defaulted, so a binary that
says nothing about maturity is not checked for it; a binary that refuses to ship
unfinished code says so. That is a real loss of enforcement compared to a
mandatory axis, traded for there being no resolution algorithm to reason about.

## Tags and tests

A test suite inherits its target's tags and platform restrictions, so a suite for
a `server` library is checked as server code without saying anything.

By default a suite runs once, on the host platform. A suite that must run in more
than one lists them:

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

That is the mechanism for "this must behave identically on both backends," which
for a language targeting a native binary and JavaScript is the test you most want
to be able to write. `I64` on the JS target ([`SPEC.md` §15](./SPEC.md)) is the
standing reason it exists. A platform listed here must be one the target admits —
asking for a JS run of a `[LINUX, MACOS]` library is an error, not a skip.

A native platform runs as a native binary where this toolchain can build one for
it, which means the host's own platform: there is no cross-compilation, so a
`LINUX` run is a Linux machine's and a `MACOS` run is a Mac's, and the other is
refused with `platform-not-implemented` rather than quietly run through
JavaScript. A suite that names no platforms still runs on JavaScript, which is
the only backend every program can rely on today.

## What tags are not

- **Not a boolean expression language.** A tag declaration has one list of
  forbidden tags and one whitelist of platforms. There is no `or`, no nesting,
  and no expression that mentions three tags at once. If a rule cannot be written
  as "these two may not coexist," it is probably a visibility rule.
- **Not conditional compilation.** No source file changes meaning across
  platforms; there is no `#if`. A library that needs two implementations gets two
  libraries with different `platforms` and one dependent that picks — which is
  visible in the build graph rather than hidden in a file.
- **Not a substitute for visibility.** Visibility answers "who may write this
  dependency edge" and is checked one edge at a time. Tags answer "what may end
  up in one artifact" and are checked over the closure. Use visibility for API
  ownership, tags for deployment boundaries.
- **Not an axis system.** There are no dimensions, so nothing requires a binary
  to state a tier, and nothing is resolved or defaulted. A tag is either present
  in a closure or it is not.
