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

    forbids {
        tags: ["client"]
    }

    requires {
        platforms: [LINUX, MACOS]
    }
}

tag {
    name: "client"
    doc: "ships to a user's machine or browser"
}

lint {
    check_during_build: true
    fail_on_finding: true
}
```

Two fields. That is not an abridged example — there is nothing else to write.
A repository-wide configuration file attracts settings, and every setting it
accepts is a way for two repositories to disagree about what the language is, or
a way for a rule to mean something different than it reads. The rule is that a
knob goes on the command (where it is visible in the invocation) or on the rule
it affects (where it is visible to someone reading that rule), and `REPO.buri`
gets what genuinely has no other home.

`lint` is here on that test rather than in spite of it. Nothing in it changes
what a finding is, what raises one, or what it is called — the catalogue is the
same in every repository. Two of its fields say where the catalogue is run and
what a finding costs; the third says which of its rules this repository listens
to, once, in the file everybody reads, by the name the finding prints. Two
repositories reading each other's code still agree about what each finding
means and what raises it; they are allowed to disagree about how early they are
told, how loudly, and — with a diff somebody had to review — about which of the
rules they run.

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

A restriction written as a whitelist means adding a platform to the toolchain
cannot silently widen code written before it existed — the reason
`platforms` lives under `requires` and is never spelled as an exclusion.

## `lint`

Where the lint catalogue runs, what a finding costs, and which of its rules run
here. A `REPO.buri` that writes none of it behaves exactly like one with no
`lint` block at all — so the block is worth writing only to say something:

```textproto schema=repo
lint {
    check_during_build: true
    fail_on_finding: true

    rules {
        # What every rule not named below is. Omitted: ENABLED, so an empty
        # or absent `rules` block changes nothing.
        default: ENABLED

        discarded_result: false
        hex_digit_table: false
    }
}
```

| | |
|---|---|
| `check_during_build` | `buri build` and `buri test` run the catalogue too, and report what it finds. Default false: they do not. |
| `fail_on_finding` | A finding is an error, and fails whichever command reported it. Default false: it is printed and changes no exit code. |
| `rules` | Which of the catalogue's rules run. Absent, or empty: all of them. |

The first field is the one that matters, and the argument for it is about when a
finding arrives rather than what it says. `buri build` and `buri test` are the
loop somebody actually runs — every few minutes while the change is being
written, and an agent runs them harder than that. A check that lives only in a
separate command is a check that gets run once, at the end, on a change too large
to still be holding in your head. That is the worst possible moment to be told
about `deep-nesting` or `oversized-function`: the finding is about a shape, and a
shape is cheap to fix while it is being made and expensive afterwards. Turning
`check_during_build` on does not add a check. It moves the ones that already
exist into the loop that would have found them anyway, days earlier.

The second is for a repository that wants the ratchet at its maximum — where a
finding is not a note to act on later but a build that does not go through, the
same as a type error. It is a separate field precisely because it is a separate
decision: a repository can want to be told during every build long before it
wants every finding to stop one.

Neither field changes `buri lint`, which exits nonzero on any finding whatever
this file says. Running the linter is already the request to be told, and a
report that exits zero is one no script can act on. What those two decide is
whether the other two commands participate at all.

### `rules`

Which of the catalogue's rules run here. One field per lint code — the code with
its hyphens turned into underscores, because a textproto field name cannot hold
one — and one `default` they are all read against:

```
enabled(rule) = override.unwrap_or(default)
```

That is the whole semantics, and everything about the block falls out of it. An
absent `rules` block is an absent override over the `ENABLED` default, so it
changes nothing; so does an empty one. `discarded_result: false` turns off one
rule and leaves every other one alone. And `default: DISABLED` with a handful of
rules written `true` is an allow list, spelled in the same two fields rather
than in a mode of its own:

```textproto schema=repo
lint {
    check_during_build: true

    rules {
        # Nothing runs but what is named here.
        default: DISABLED

        missing_dep: true
        unused_import: true
    }
}
```

The field set is **generated from the catalogue**, not written down beside it:
the names a `rules` block accepts are the lint codes this `buri` has, so a rule
cannot ship without a field, and a field cannot outlive the rule it names.
`unused_improt: false` is the [`unknown-field`](../errors/unknown-field.md)
diagnostic every other undeclared field gets, with `unused_import` offered as
the fix. A misspelled rule is a file that does not read — never a rule quietly
left on, which is the failure mode every configurable linter has.

A rule turned off here is turned off everywhere at once: `buri lint` does not
report it, `check_during_build` does not report it, and the editor does not
underline it. It is dropped from the report rather than downgraded — there is
still one severity — and it is never dropped quietly. Every command that reports
findings prints which rules this file turned off:

```
REPO.buri turns off 2 of 25 lint rules: discarded-result, hex-digit-table
```

and, under `default: DISABLED`, prints the smaller side instead — the rules that
still run. A clean report from a repository that has turned rules off is a
sentence about a smaller catalogue, and the line above it is what says so. The
alternative is a check that did not run and nothing on the screen that says it
did not, which is worse than the finding it was hiding.

What the block deliberately cannot do is make that question local. There is no
per-directory exemption and no per-file suppression comment, so "is this rule on
here" is answered by one file for the whole repository, and turning a rule off is
a diff somebody reviews rather than a line somebody adds to the file they were
already editing. The two booleans above are still ratchets — each may only make
the toolchain stricter — and `rules` is the one field that is not, which is why
it is shaped to be loud.

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
- **No per-file or per-directory lint suppression.** [`rules`](#rules) turns a
  rule off for the *repository*, by name, in this file. There is no suppression
  comment, no `allow` attribute on a rule in a `BUILD.buri`, and no
  per-directory exemption — and the three absences are one decision. A
  repository-wide switch keeps "is this rule on here" answerable by one file
  that everybody has read, and keeps turning a rule off a diff somebody reviews;
  a local one turns the same act into a line added by whoever tripped over the
  finding, in the file they were already editing, which is how a check that
  should have been argued about once gets silenced instead. There is likewise no
  `severity` field: one catalogue, one severity — every finding is a warning
  ([`buri lint`](../cli/lint.md)) — and `fail_on_finding` is the only thing that
  moves it, for every rule at once. What survives all of this is the property
  worth having: "does this code pass lint" is still answered by the code plus one
  short file, because what a finding means never varies — a file that passes
  under one repository's rules is a file the next repository's rules are read
  against, and the report says which rules those were.
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
