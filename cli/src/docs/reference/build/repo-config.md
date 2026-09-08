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

Two fields. The example is not abridged. A knob goes on the command, where the
invocation shows it, or on the rule it affects, where anyone reading that rule
sees it. `REPO.buri` gets what has no other home. There is no `flags` field: a
repository-wide compiler flag is a dialect, and a dialect makes one source file
mean different things in different repositories.

## `tag`

The tag vocabulary and, on the same block, everything that follows from carrying
a tag. [`tags.md`](./tags.md) documents it fully. In summary, two blocks named
for their polarity:

| | |
|---|---|
| `forbids { tags: [...] }` | Tags that may not appear anywhere in the same dependency closure. Symmetric. |
| `requires { platforms: [...] }` | The only platforms code carrying this tag may be built for. A whitelist; unset means all. |

Those are the only two fields either block accepts. `forbids` takes no platforms
and `requires` takes no tags, both for reasons [`tags.md`](./tags.md) gives.

This file is the only place that introduces a tag name, and the vocabulary is
**closed**. A build file three directories down writing `tags: ["internal"]`
either resolves to a block here or fails. A tag declared twice is an error, and
one declared nowhere is an error rather than a typo that turns into an unchecked
build.

`Platform` is a closed enum in the schema, `LINUX`, `MACOS`, `JS`, `WEB`, and so
is `Arch`. Adding one is a compiler change rather than a configuration change,
so there is nothing to declare here. With no library or tag naming a platform,
nothing constrains anything, and the build attempts a JS build only when some
binary lists a JS output.

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
| `allow` | Declarations a rule is not asked about. Absent, or empty: none. |

Turn `check_during_build` on because those are the commands you actually run,
and a finding about shape is cheap to fix while you are making the shape and
expensive afterwards. `fail_on_finding` is separate because it is a separate
decision: a repository can want to hear from the linter during every build long
before it wants every finding to stop one.

Neither field changes `buri lint`. It exits nonzero on any finding, whatever
this file says.

### `rules`

Which of the catalogue's rules run here. One field per lint code, spelled with
underscores instead of hyphens because a textproto field name cannot hold one,
plus one `default` that every field is read against:

```
enabled(rule) = override.unwrap_or(default)
```

An absent or empty `rules` block changes nothing. `discarded_result: false`
turns off one rule and leaves every other one alone. `default: DISABLED` plus a
handful of rules written `true` is an allow list:

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

The catalogue **generates the field set**. A `rules` block accepts exactly the
lint codes this `buri` has, so a rule cannot ship without a field, and a field
cannot outlive the rule it names. `unused_improt: false` gets the
[`unknown-field`](../errors/unknown-field.md) diagnostic every other undeclared
field gets, offering `unused_import` as the fix. A misspelled rule is a file that
does not read, never a rule left quietly on.

Turning a rule off here turns it off everywhere at once: `buri lint`,
`check_during_build`, and the editor. The report drops the rule rather than
downgrading it, since there is still one severity, and it never drops one
quietly. Every command that reports findings prints which rules this file turned
off:

```
REPO.buri turns off 2 of 25 lint rules: discarded-result, hex-digit-table
```

Under `default: DISABLED` it prints the smaller side instead, the rules that
still run. A check that did not run with nothing on screen to say so is worse
than the finding it was hiding.

There is no per-directory exemption and no per-file suppression comment. One
file answers "is this rule on here" for the whole repository, and turning a rule
off takes a diff somebody reviews rather than a line somebody adds to the file
they were already editing.

### `allow`

Sometimes the shape a rule objects to is not yours. A wire format, a protocol, an
API somebody else specified: the signature is fixed outside the repository, and
the fix the finding names would break every caller. Turning the rule off for the
whole repository to say that is too big a hammer, so name the declaration
instead:

```textproto schema=repo
lint {
    allow {
        # The frame header is the protocol's, field for field. Grouping it
        # would not change what a caller assembles.
        too_many_parameters: ["//lib/wire:encode"]
    }
}
```

The rule stays on everywhere else, including on the function beside that one. An
entry is a package label, a colon, and the name the finding prints. Two
declarations of one name in one package share an exemption.

One rule has a field: [`too-many-parameters`](../lints/too-many-parameters.md).
Two things earn one, and both are demanding. The finding is reported on a
declaration's own name, so a label says exactly what the exemption is about — a
rule reported inside a body has nothing for a label to name, and a field for one
would be an exemption that could never match. And what the rule objects to can
be decided outside the repository, which a signature can be and a body's length
cannot. Writing any other rule's name here gets the
[`unknown-field`](../errors/unknown-field.md) diagnostic.

Every report names its exemptions, one by one, for the reason it names the rules
that did not run:

```
REPO.buri exempts 1 declaration: too-many-parameters on //lib/wire:encode
```

A label that is not a label is a file that does not read
([`allow-not-a-declaration`](../errors/allow-not-a-declaration.md)). A label that
reads and names nothing — a declaration since renamed — is not an error, because
the finding it stopped exempting comes back, which is the safe direction to fail
in.

## What is not here

- **No toolchain pin.** There was one: `toolchain { version, sha256 }`. A pin
  earns its keep where something *fetches* a toolchain, and nothing fetches one
  here. What survives is `buri version --verbose`, which prints the running
  executable's hash so a bug report can name one build of a version. A
  `REPO.buri` still carrying a `toolchain` block gets the unknown-field
  diagnostic every other undeclared field gets.
- **No `name`.** Label syntax is `//`-rooted and never mentions a name,
  artifacts take their names from their package directory, and a name here would
  compete with the directory you checked the repository out into. Rules in a
  `BUILD.buri` have no `name` either
  ([`build-files.md`](./build-files.md#labels)).
- **No defaults block.** Visibility is private unless a rule says otherwise, and
  that is a fixed rule of the language rather than a repository setting. There
  is no repository-wide test timeout either. A suite that needs longer writes
  `timeout_seconds` where the person reading that suite will see it.
- **No per-file or per-directory lint suppression.** [`rules`](#rules) turns a
  rule off for the *repository*, by name, in this file, and [`allow`](#allow)
  takes one off a single named declaration. Neither is a comment beside the code
  or a line covering a directory. There is no `severity` field either. One
  catalogue, one severity: every finding is a warning
  ([`buri lint`](../cli/lint.md)), and only `fail_on_finding` moves it, for
  every rule at once. So the code plus one short file still answers "does this
  code pass lint", and a report says which rules it ran and what it did not ask
  them about.
- **No compiler flags.** A flag list is a dialect.
- **No dependency versions or lockfile.** There are no external repositories
  yet. Your only sources are this repository and the `core/*` that ships with
  the toolchain. When external repositories arrive they get their own file.
- **No build settings, profiles, or optimization levels.** `buri build --release`
  is a flag on the command and part of the cache key.
- **No environment.** Actions run with an empty environment
  ([`hermeticity.md`](./hermeticity.md)), so there is nowhere to set a variable
  because nothing reads one.
- **No rule definitions.** Two rule kinds, both in the schema. Where a
  repository can define rules, reading a `BUILD.buri` no longer tells you what
  will happen.
