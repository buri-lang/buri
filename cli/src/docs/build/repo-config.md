# `REPO.buri`

One file, at the repository root. Its presence is what makes a directory a
repository root — every `//` label and every `//` module path is resolved
against the directory containing it, and the CLI walks up from the working
directory to find it.

It parses as `buri.build.v1.RepoConfig`
([`schema/repo.proto`](../schema/repo.proto)) — its own schema file, separate from
the one `BUILD.buri` uses.

**The whole file:**

```textproto schema=repo
# REPO.buri

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

One field. That is not an abridged example — there is nothing else to write.
A repository-wide config file attracts settings, and every setting it accepts is
a way for two repositories to disagree about what the language is, or a way for a
rule to mean something different than it reads. The rule is that a knob goes on
the command (where it is visible in the invocation) or on the rule it affects
(where it is visible to someone reading that rule), and `REPO.buri` gets what
genuinely has no other home.

There is no `flags` field. A repository-wide compiler flag is a dialect, and a
dialect makes source files mean different things in different repositories — the
exact property the rest of this design spends its budget avoiding. If some flag
turns out to be genuinely necessary, it becomes a named field on this message,
argued about once, rather than an open list of strings nobody can enumerate.

## `tag`

The tag vocabulary and, on the same block, everything that follows from carrying
a tag. Fully documented in [`tags.md`](./tags.md); the summary is two blocks
named for their polarity, so what a tag rules out and what it demands are
distinguishable at a glance:

| | |
|---|---|
| `forbids { tags: [...] }` | Tags that may not appear anywhere in the same dependency closure. Symmetric. |
| `requires { platforms: [...] }` | The only platforms code carrying this tag may be built for. A whitelist; unset means all. |

Those are the only two fields either block accepts. `forbids` takes no platforms
and `requires` takes no tags, both for reasons [`tags.md`](./tags.md) gives. There is nothing
else — no axes, no composition modes, no defaults, and no separate block for
cross-cutting policy.

This is the only place a tag name is introduced, and the vocabulary is **closed**.
A build file three directories down writing `tags: ["internal"]` resolves to a
block here or fails: an undeclared tag is an error, not an annotation that
quietly means nothing. So `internal` declared twice is rejected rather than
meaning whichever was parsed first, and `internal` declared nowhere is rejected
rather than turning a typo into an unchecked build.

`Platform` is a closed enum in the schema — `LINUX`, `MACOS`, `JS`, `WEB` — and
`Arch` likewise. Adding one is a compiler change, not a configuration change, so there
is nothing to declare here. A repository that does not ship to JS does not need
to say so: with no library or tag naming a platform, nothing constrains anything,
and a JS build is only attempted if some binary lists a JS output.

Note that a restriction written as a whitelist means adding a platform to the
toolchain cannot silently widen code written before it existed — the reason
`platforms` lives under `requires` and is never spelled as an exclusion.

## What is not here

- **No toolchain pin.** There was one: `toolchain { version, sha256 }`, an exact
  compiler version and the hash of the compiler that had to build this
  repository, refused with exit `2` by every command that opened a repository.
  It was removed. A pin is worth its weight where a toolchain is *fetched* — it
  is the thing a downloader verifies before unpacking an archive — and nothing
  fetches one: a compiler is installed by whoever installs it, and a field
  naming a hash that the same person also writes checks that they agree with
  themselves. What is left of it is `buri version --verbose`, which prints the
  running executable's hash so a bug report can name one build of a version.
  A `REPO.buri` still carrying a `toolchain` block gets the unknown-field
  diagnostic every other undeclared field gets.
- **No `name`.** A repository does not need to announce what it is called. The
  label syntax is `//`-rooted and never mentions it, artifacts are named from
  their package directory, and a name here would be a second identifier
  competing with the directory the repository is checked out into. Rules in a
  `BUILD.buri` have no `name` either, for the same reason
  ([`build-files.md`](./build-files.md#labels)).
- **No defaults block.** Visibility is private unless a rule says otherwise, and
  that is a fixed rule of the language rather than a repository setting. A
  repository that could flip the default to public would be one where reading
  `visibility: []` on a library tells you nothing until you have also read a
  file at the root — which defeats the point of putting visibility on the rule.
  There is likewise no repository-wide test timeout: a suite that needs longer
  writes `timeout_seconds` where the person reading that suite will see it.
- **No lint configuration.** `buri lint` has one catalogue and one severity for
  each check ([`cli.md`](./cli.md#lint)), the same in every repository. A
  configurable linter means "does this code pass" is a question you cannot answer
  from the code, and a per-repository `allow` list is how a check that should have
  been argued about once gets silenced quietly instead. Several build-graph
  checks — a use with no dep, a dep nothing uses, a source file no rule declares
  — were already unconditional for the same reason; this makes the rest of them
  consistent with it.
- **No compiler flags.** Covered above: a flag list is a dialect.
- **No dependency versions or lockfile.** There are no external repositories
  yet; the only sources are this repository and the `core/*` that ships with
  the toolchain. When external repositories arrive they get their own file
  rather than a section here, so that `REPO.buri` stays reviewable.
- **No build settings, profiles, or optimization levels.** `buri build --release`
  is a flag on the command, part of the cache key, and not a thing a repository
  configures per-target.
- **No environment.** Actions run with an empty environment
  ([`hermeticity.md`](./hermeticity.md)). There is
  nowhere to set a variable because nothing reads one.
- **No rule definitions.** Two rule kinds, both in the schema. A build system
  that lets a repository define rules is a build system where reading a
  `BUILD.buri` is not enough to know what will happen.
