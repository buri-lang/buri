# `REPO.buri`

One file, at the repository root. Its presence is what makes a directory a
repository root — every `//` label and every `//` module path is resolved
against the directory containing it, and the CLI walks up from the working
directory to find it.

It parses as `buri.build.v1.RepoConfig`
([`schema/repo.proto`](./cli/src/docs/schema/repo.proto)) — its own schema file, separate from
the one `BUILD.buri` uses.

**The whole file:**

```textproto schema=repo
# REPO.buri

toolchain {
  version: "0.3.0"
  sha256: "9f2b1c4e8a7d3f60b5c1e9a24d7f8036b1c5e2a94f7d0b83c6e1a5d2f9b04c7e3"
}

tag {
  name: "server"
  doc: "runs on infrastructure we operate"

  forbids { tags: ["client"] }

  requires { platforms: [LINUX, MACOS] }
}

tag {
  name: "client"
  doc: "ships to a user's machine or browser"
}
```

Two fields. That is not an abridged example — there is nothing else to write.
A repository-wide config file attracts settings, and every setting it accepts is
a way for two repositories to disagree about what the language is, or a way for a
rule to mean something different than it reads. The rule is that a knob goes on
the command (where it is visible in the invocation) or on the rule it affects
(where it is visible to someone reading that rule), and `REPO.buri` gets what
genuinely has no other home.

## `toolchain`

An exact version, never a range. Two checkouts of the same commit must not build
with two different compilers, and a range guarantees that eventually they will.
The `sha256` covers the compiler release archive; the CLI refuses to run if the
toolchain it resolved does not hash to it, which is the difference between
pinning a version and pinning a compiler.

Both are checked when a repository is opened, by every command that opens one,
and a disagreement is exit `2` before anything is compiled. Two details of how:

- **What is hashed is the running executable** — the artifact the release
  archive would have contained. There is no archive on the machine to hash,
  because there is no downloader yet to have fetched one, and hashing the
  executable is the stricter of the two: it also catches an executable replaced
  after it was unpacked.
- **A `sha256` of nothing but zeros means unpinned.** `"00"` and sixty-four
  zeros both say "this repository's toolchain has no published release to name",
  which is the state a repository is in while its compiler is built from source
  — including this toolchain's own test fixtures. An unpinned pin verifies
  nothing, still enters every cache key, and is reported as unpinned by
  `buri version`. That is the whole escape hatch: there is no flag and no
  environment variable, because a pin you can turn off from the command line is
  a pin that gets turned off in the one script that matters.

There is no `flags` field. A repository-wide compiler flag is a dialect, and a
dialect makes source files mean different things in different repositories — the
exact property the rest of this design spends its budget avoiding. If some flag
turns out to be genuinely necessary, it becomes a named field on this message,
argued about once, rather than an open list of strings nobody can enumerate.

## `tag`

The tag vocabulary and, on the same block, everything that follows from carrying
a tag. Fully documented in [`TAGS.md`](./cli/src/docs/build/tags.md); the summary is two blocks
named for their polarity, so what a tag rules out and what it demands are
distinguishable at a glance:

| | |
|---|---|
| `forbids { tags: [...] }` | Tags that may not appear anywhere in the same dependency closure. Symmetric. |
| `requires { platforms: [...] }` | The only platforms code carrying this tag may be built for. A whitelist; unset means all. |

Those are the only two fields either block accepts. `forbids` takes no platforms
and `requires` takes no tags, both for reasons `TAGS.md` gives. There is nothing
else — no axes, no composition modes, no defaults, and no separate block for
cross-cutting policy.

This is the only place a tag name is introduced, and the vocabulary is **closed**.
A build file three directories down writing `tags: ["internal"]` resolves to a
block here or fails: an undeclared tag is an error, not an annotation that
quietly means nothing. So `internal` declared twice is rejected rather than
meaning whichever was parsed first, and `internal` declared nowhere is rejected
rather than turning a typo into an unchecked build.

`Platform` is a closed enum in the schema — `LINUX`, `MACOS`, `JS` — and `Arch`
likewise. Adding one is a compiler change, not a configuration change, so there
is nothing to declare here. A repository that does not ship to JS does not need
to say so: with no library or tag naming a platform, nothing constrains anything,
and a JS build is only attempted if some binary lists a JS output.

Note that a restriction written as a whitelist means adding a platform to the
toolchain cannot silently widen code written before it existed — the reason
`platforms` lives under `requires` and is never spelled as an exclusion.

## What is not here

- **No `name`.** A repository does not need to announce what it is called. The
  label syntax is `//`-rooted and never mentions it, artifacts are named from
  their package directory, and a name here would be a second identifier
  competing with the directory the repository is checked out into. Rules in a
  `BUILD.buri` have no `name` either, for the same reason
  ([`BUILD-FILES.md`](./cli/src/docs/build/build-files.md#labels)).
- **No defaults block.** Visibility is private unless a rule says otherwise, and
  that is a fixed rule of the language rather than a repository setting. A
  repository that could flip the default to public would be one where reading
  `visibility: []` on a library tells you nothing until you have also read a
  file at the root — which defeats the point of putting visibility on the rule.
  There is likewise no repository-wide test timeout: a suite that needs longer
  writes `timeout_seconds` where the person reading that suite will see it.
- **No lint configuration.** `buri lint` has one catalogue and one severity for
  each check ([`CLI.md`](./cli/src/docs/build/cli.md#lint)), the same in every repository. A
  configurable linter means "does this code pass" is a question you cannot answer
  from the code, and a per-repository `allow` list is how a check that should have
  been argued about once gets silenced quietly instead. Several build-graph
  checks — a use with no dep, a dep nothing uses, a source file no rule declares
  — were already unconditional for the same reason; this makes the rest of them
  consistent with it.
- **No compiler flags.** Covered above: a flag list is a dialect.
- **No dependency versions or lockfile.** There are no external repositories
  yet; the only sources are this repository and the `core/*` that ships with the
  pinned toolchain. When external repositories arrive they get their own file
  rather than a section here, so that `REPO.buri` stays reviewable.
- **No build settings, profiles, or optimization levels.** `buri build --release`
  is a flag on the command, part of the cache key, and not a thing a repository
  configures per-target.
- **No environment.** Actions run with an empty environment
  ([`HERMETICITY-AND-CACHING.md`](./cli/src/docs/build/hermeticity.md)). There is
  nowhere to set a variable because nothing reads one.
- **No rule definitions.** Two rule kinds, both in the schema. A build system
  that lets a repository define rules is a build system where reading a
  `BUILD.buri` is not enough to know what will happen.
