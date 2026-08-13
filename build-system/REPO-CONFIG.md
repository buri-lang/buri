# `REPO.buri`

One file, at the repository root. Its presence is what makes a directory a
repository root — every `//` label is resolved against the directory containing
it, and the CLI walks up from the working directory to find it.

It parses as `buri.build.v1.RepoConfig`
([`schema/build.proto`](./schema/build.proto)).

```textproto
# REPO.buri

name: "acme"

toolchain {
  version: "0.2.0"
  sha256: "9f2b1c4e8a7d3f60b5c1e9a24d7f8036b1c5e2a94f7d0b83c6e1a5d2f9b04c7e3"
  flags: ["--warnings-as-errors"]
}

dimension {
  name: "tier"
  value { name: "client" doc: "ships to a user's machine or browser" }
  value { name: "server" doc: "runs on infrastructure we operate" }
}

constraint {
  when: "platform=js"
  require: ["tier=client"]
  message: "the JS output ships to browsers; server code is not shipped to users"
}

defaults {
  visibility: ["//visibility:private"]
  test_timeout_seconds: 60
}

lint {
  error: ["unused-dep", "undeclared-src", "unreachable-export"]
}
```

## `toolchain`

```textproto
toolchain {
  version: "0.2.0"
  sha256: "9f2b1c…"
  flags: ["--warnings-as-errors"]
}
```

An exact version, never a range. Two checkouts of the same commit must not build
with two different compilers, and a range guarantees that eventually they will.
The `sha256` covers the compiler release archive; the CLI refuses to run if the
toolchain it resolved does not hash to it, which is the difference between
pinning a version and pinning a compiler.

`flags` apply to every compile action in the repository and are part of every
cache key, so changing one invalidates everything. There is no per-target flag
field, and adding one would mean two targets in the same repository disagreeing
about what the language is.

## `dimension` and `constraint`

The tag vocabulary. Fully documented in [`TAGS.md`](./TAGS.md); the summary is
that a `dimension` is an axis a build varies along, a `constraint` rules out
combinations of values, and libraries then say which values they accept.

`platform` and `arch` are predeclared:

```textproto
dimension {
  name: "platform"
  value { name: "linux" }
  value { name: "macos" }
  value { name: "js" }
}

dimension {
  name: "arch"
  value { name: "x86_64" }
  value { name: "arm64" }
  default: "arm64"      # only the host default; outputs still pin it explicitly
}
```

Redeclaring either is allowed only to **remove** values — a repository that does
not ship to JS can say so, and then a library tagged `js` fails to parse rather
than failing to link. Adding a platform value is a compiler change, not a
configuration change.

Value names are unique across dimensions. The tool rejects a `REPO.buri` that
declares `internal` twice, because `tags: ["internal"]` in some build file three
directories down would then quietly mean whichever one was declared first.

## `defaults`

```textproto
defaults {
  visibility: ["//visibility:private"]
  test_timeout_seconds: 60
}
```

`visibility` is the last resort in the chain: rule, then package
`default_visibility`, then here, then `//visibility:private`. Making the
repository default private and opening surfaces one at a time is the posture
these documents assume; the opposite default is defensible in a small
repository and gets progressively harder to walk back.

## `lint`

```textproto
lint {
  error: ["unused-dep", "undeclared-src", "unreachable-export"]
  allow: ["name-matches-directory"]
}
```

Promote diagnostics to errors, or silence them repository-wide. The catalogue is
in [`CLI.md`](./CLI.md#lint). Several build-graph checks — an import with no
dep, a dep no import uses, a source file no rule declares — are errors
unconditionally and cannot be moved by this block, because each one makes the
build graph a description of something other than the code.

Prefer narrowing a rule to silencing it. `allow` is repository-wide and there is
no per-file suppression comment, which is a deliberate friction: a lint that has
to be silenced everywhere is a lint that should be argued about once, in this
file, in a commit someone reviews.

## What is not here

- **No dependency versions or lockfile.** There are no external repositories
  yet; the only sources are this repository and the `core/*` that ships with the
  pinned toolchain. When external repositories arrive they get their own file
  rather than a section here, so that `REPO.buri` stays reviewable.
- **No build settings, profiles, or optimization levels.** `buri build --release`
  is a flag on the command, part of the cache key, and not a thing a repository
  configures per-target.
- **No environment.** Actions run with an empty environment
  ([`HERMETICITY-AND-CACHING.md`](./HERMETICITY-AND-CACHING.md)). There is
  nowhere to set a variable because nothing reads one.
- **No rule definitions.** Two rule kinds, both in the schema. A build system
  that lets a repository define rules is a build system where reading a
  `BUILD.buri` is not enough to know what will happen.
