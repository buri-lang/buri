# `REPO.buri`

One file, at the repository root. It is what makes a directory the repository
root. Every `//` label and every `//` module path resolves against the directory
holding it, and the CLI walks up from your working directory to find it.

It parses as `buri.build.v1.RepoConfig`
([`schema/repo.proto`](../schema/repo.proto)), its own schema file, separate from
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

Two fields. The example is not abridged. There is nothing else to write. A
repository-wide configuration file attracts settings, and every setting it
accepts lets two repositories disagree about what the language is, or lets a
rule mean something other than what it reads. So a knob goes on the command,
where the invocation shows it, or on the rule it affects, where anyone reading
that rule sees it. `REPO.buri` gets what has no other home.

`lint` passes that test rather than dodging it. Nothing in it changes what a
finding is, what raises one, or what it is called. The catalogue is the same in
every repository. Two of its fields say where the catalogue runs and what a
finding costs. The third says which of its rules this repository listens to,
once, in the file everybody reads, under the name the finding prints. Two
repositories reading each other's code still agree about what each finding means
and what raises it. They may disagree about how early they hear it, how loudly,
and, through a diff somebody had to review, about which of the rules they run.

There is no `flags` field. A repository-wide compiler flag is a dialect, and a
dialect makes one source file mean different things in different repositories.
Avoiding exactly that is what the rest of this design spends its budget on. If
some flag turns out to be genuinely necessary, it becomes a named field on this
message, argued about once, rather than an open list of strings nobody can
enumerate.

## `tag`

The tag vocabulary and, on the same block, everything that follows from carrying
a tag. [`tags.md`](./tags.md) documents it fully. In summary: two blocks named
for their polarity, so you can tell at a glance what a tag rules out and what it
demands:

| | |
|---|---|
| `forbids { tags: [...] }` | Tags that may not appear anywhere in the same dependency closure. Symmetric. |
| `requires { platforms: [...] }` | The only platforms code carrying this tag may be built for. A whitelist; unset means all. |

Those are the only two fields either block accepts. `forbids` takes no platforms
and `requires` takes no tags, both for reasons [`tags.md`](./tags.md) gives.
There is nothing else: no axes, no composition modes, no defaults, and no
separate block for cross-cutting policy.

This file is the only place that introduces a tag name, and the vocabulary is
**closed**. A build file three directories down writing `tags: ["internal"]`
either resolves to a block here or fails. An undeclared tag is an error, not an
annotation that quietly means nothing. So `internal` declared twice is an error
rather than whichever block the parser saw first, and `internal` declared
nowhere is an error rather than a typo that turns into an unchecked build.

`Platform` is a closed enum in the schema, `LINUX`, `MACOS`, `JS`, `WEB`, and so
is `Arch`. Adding one is a compiler change rather than a configuration change,
so there is nothing to declare here. A repository that does not ship to JS never
has to say so. With no library or tag naming a platform, nothing constrains
anything, and the build attempts a JS build only when some binary lists a JS
output.

Writing a restriction as a whitelist stops a new toolchain platform from
silently widening code written before it existed. That is why `platforms` lives
under `requires` and never appears as an exclusion.

## `lint`

Where the lint catalogue runs, what a finding costs, and which of its rules run
here. A `REPO.buri` that writes none of these behaves exactly like one with no
`lint` block, so write the block only to say something:

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
| `fail_on_finding` | A finding is an error, and fails whichever command reported it. Default false: the command prints the finding and returns its usual exit code. |
| `rules` | Which of the catalogue's rules run. Absent, or empty: all of them. |

The first field is the one that matters, and the argument for it is about when a
finding arrives rather than what it says. You run `buri build` and `buri test`
every few minutes while writing a change, and an agent runs them harder than
that. A check that lives only in a separate command runs once, at the end, on a
change too large to still hold in your head. That is the worst possible moment
to hear about `deep-nesting` or `oversized-function`. Those findings are about
shape, and a shape is cheap to fix while you are making it and expensive
afterwards. Turning `check_during_build` on adds no check. It moves the checks
you already have into the loop that would have found them days earlier.

The second field is for a repository that wants the ratchet at its maximum. A
finding stops the build, the same as a type error, instead of leaving a note to
act on later. It is a separate field because it is a separate decision. A
repository can want to hear from the linter during every build long before it
wants every finding to stop one.

Neither field changes `buri lint`. It exits nonzero on any finding, whatever
this file says. Running the linter is already a request to be told, and no
script can act on a report that exits zero. Those two fields decide only whether
the other two commands take part.

### `rules`

Which of the catalogue's rules run here. One field per lint code, spelled with
underscores instead of hyphens because a textproto field name cannot hold one,
plus one `default` that every field is read against:

```
enabled(rule) = override.unwrap_or(default)
```

That is the whole semantics, and everything about the block follows from it. An
absent `rules` block is an absent override over the `ENABLED` default, so it
changes nothing, and an empty one changes nothing either. `discarded_result:
false` turns off one rule and leaves every other one alone. `default: DISABLED`
plus a handful of rules written `true` is an allow list, spelled with the same
two fields rather than in a mode of its own:

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

The catalogue **generates the field set** rather than repeating it alongside. A
`rules` block accepts exactly the lint codes this `buri` has, so a rule cannot
ship without a field, and a field cannot outlive the rule it names.
`unused_improt: false` gets the [`unknown-field`](../errors/unknown-field.md)
diagnostic every other undeclared field gets, offering `unused_import` as the
fix. A misspelled rule is a file that does not read. It is never a rule left
quietly on, which is the failure mode of every configurable linter.

Turning a rule off here turns it off everywhere at once. `buri lint` stops
reporting it, `check_during_build` stops reporting it, and the editor stops
underlining it. The report drops the rule rather than downgrading it, since
there is still one severity, and it never drops one quietly. Every command that
reports findings prints which rules this file turned off:

```
REPO.buri turns off 2 of 25 lint rules: discarded-result, hex-digit-table
```

Under `default: DISABLED` it prints the smaller side instead, the rules that
still run. A clean report from a repository that has turned rules off is a
sentence about a smaller catalogue, and that line is what says so. The
alternative is a check that did not run with nothing on screen to say so, which
is worse than the finding it was hiding.

The block deliberately cannot make that question local. There is no
per-directory exemption and no per-file suppression comment. One file answers
"is this rule on here" for the whole repository, and turning a rule off takes a
diff somebody reviews rather than a line somebody adds to the file they were
already editing. The two booleans above are ratchets: each may only make the
toolchain stricter. `rules` is the one field that is not, which is why it is
shaped to be loud.

## What is not here

- **No toolchain pin.** There was one: `toolchain { version, sha256 }`, an exact
  compiler version and the hash of the compiler that had to build this
  repository. Every command that opened a repository refused with exit `2` when
  it did not match. That pin is gone. A pin earns its keep where something
  *fetches* a toolchain, since a downloader verifies it before unpacking an
  archive, and nothing fetches one here. Whoever installs the compiler installs
  it, and a field naming a hash that the same person also writes only checks
  that they agree with themselves. What survives is `buri version --verbose`,
  which prints the running executable's hash so a bug report can name one build
  of a version. A `REPO.buri` still carrying a `toolchain` block gets the
  unknown-field diagnostic every other undeclared field gets.
- **No `name`.** A repository does not announce what it is called. Label syntax
  is `//`-rooted and never mentions a name, artifacts take their names from
  their package directory, and a name here would compete with the directory you
  checked the repository out into. Rules in a `BUILD.buri` have no `name`
  either, for the same reason
  ([`build-files.md`](./build-files.md#labels)).
- **No defaults block.** Visibility is private unless a rule says otherwise, and
  that is a fixed rule of the language rather than a repository setting. If a
  repository could flip the default to public, reading `visibility: []` on a
  library would tell you nothing until you had also read a file at the root,
  which defeats the point of putting visibility on the rule. There is no
  repository-wide test timeout either. A suite that needs longer writes
  `timeout_seconds` where the person reading that suite will see it.
- **No per-file or per-directory lint suppression.** [`rules`](#rules) turns a
  rule off for the *repository*, by name, in this file. There is no suppression
  comment, no `allow` attribute on a rule in a `BUILD.buri`, and no
  per-directory exemption. Those three absences are one decision. A
  repository-wide switch keeps "is this rule on here" answerable by one file
  everybody has read, and keeps turning a rule off a diff somebody reviews. A
  local switch turns the same act into a line added by whoever tripped over the
  finding, in the file they were already editing, which is how a check that
  deserved one argument gets silenced instead. There is no `severity` field
  either. One catalogue, one severity: every finding is a warning
  ([`buri lint`](../cli/lint.md)), and only `fail_on_finding` moves it, for
  every rule at once. What survives all of this is the property worth having.
  The code plus one short file still answers "does this code pass lint", because
  what a finding means never varies. A file that passes under one repository's
  rules gets read against the next repository's rules, and the report says which
  rules those were.
- **No compiler flags.** Covered above: a flag list is a dialect.
- **No dependency versions or lockfile.** There are no external repositories
  yet. Your only sources are this repository and the `core/*` that ships with
  the toolchain. When external repositories arrive they get their own file
  rather than a section here, so that `REPO.buri` stays reviewable.
- **No build settings, profiles, or optimization levels.** `buri build --release`
  is a flag on the command and part of the cache key. A repository does not
  configure it per target.
- **No environment.** Actions run with an empty environment
  ([`hermeticity.md`](./hermeticity.md)). There is
  nowhere to set a variable because nothing reads one.
- **No rule definitions.** Two rule kinds, both in the schema. Where a
  repository can define rules, reading a `BUILD.buri` no longer tells you what
  will happen.
