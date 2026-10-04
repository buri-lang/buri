# `REPO.buri`

`REPO.buri` marks the repository root. Every `//` label and `//` module path
resolves against its directory, and the CLI walks up from your working
directory to find it. It parses as `buri.build.v1.RepoConfig`
([`schema/repo.proto`](../schema/repo.proto)), a separate schema from
`BUILD.buri`'s.

**The whole file, unabridged:**

```textproto schema=repo
# REPO.buri
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

tag {
    name: "client"
    doc: "ships to a user's machine or browser"
}

lint {
    check_during_build: true
    fail_on_finding: true
}

language {
    name: "jsonc"
    extensions: [".code-workspace"]
}
```

A knob goes on the command, where the invocation shows it, or on the rule it
affects, where its reader sees it. `REPO.buri` gets only what has no other home.

## `tag`

Declares the tag vocabulary and what carrying each tag implies.
[`tags.md`](./tags.md) has the details:

| | |
|---|---|
| `forbids { tags: [...] }` | Tags that may not appear anywhere in the same dependency closure. Symmetric. |
| `forbids { backends: [...], platforms: [...] }` | Backends and platforms code carrying this tag may not be built or tested for. Every other platform, including one added later, stays open. |
| `requires { backends: [...], platforms: [...] }` | The only backends and platforms code carrying this tag may be built for. A whitelist; unset means all. |

A tag admits what its `requires` admits (everything when unset), minus what its
`forbids` names. Naming a backend or platform on both sides is an error.
`requires` takes no tags; [`tags.md`](./tags.md) says why.

The vocabulary is **closed**: this file is the only place a tag name is
introduced. A build file writing `tags: ["internal"]` either resolves to a block
here or fails, so a typo can't turn into an unchecked build. Declaring a tag
twice is an error too.

The bundled platforms `native`, `node` and `web`, `Backend`, and `native`'s
variants are fixed by the toolchain, so there's nothing to declare for them.
With no library or tag naming a platform, nothing is constrained, and the build
attempts a `node` build only when some binary lists a `node` output.

## `lint`

Where the lint catalogue runs, what a finding costs, and which rules run. Every
field defaults to the behavior of having no `lint` block:

```textproto schema=repo
lint {
    check_during_build: true
    fail_on_finding: true

    rules {
        # What every rule not named below is. Omitted: ENABLED, so an empty
        # or absent `rules` block changes nothing.
        default: ENABLED

        hand_rolled_hex_digits: false
        ignored_result: false
    }
}
```

| | |
|---|---|
| `check_during_build` | `buri build` and `buri test` run the catalogue too, and report what it finds. Default false. |
| `fail_on_finding` | A finding fails whichever command reported it. Default false: the command prints the finding and returns its usual exit code. |
| `rules` | Which of the catalogue's rules run. Absent or empty: all of them. |

Turn on `check_during_build` because those are the commands you actually run,
and a shape finding is cheapest to fix while you're making the shape.
`fail_on_finding` is separate so you can hear from the linter on every build
before letting it stop one.

Neither field changes `buri lint`, which exits nonzero on any finding.

### `rules`

One field per lint code, with underscores for hyphens, plus a `default`:

```
enabled(rule) = override.unwrap_or(default)
```

`ignored_result: false` turns off one rule. `default: DISABLED` plus a few
rules set to `true` is an allow list:

```textproto schema=repo
lint {
    check_during_build: true

    rules {
        # Nothing runs but what is named here.
        default: DISABLED

        missing_dependency: true
        unused_import: true
    }
}
```

The catalogue **generates the field set**, so the block accepts exactly the
lint codes this `buri` has. `unused_improt: false` gets the
[`build-unknown-field`](../errors/build-unknown-field.md) diagnostic, offering
`unused_import` as the fix. A misspelled rule never stays quietly on.

Turning a rule off here turns it off everywhere: `buri lint`,
`check_during_build`, and the editor. Every command that reports findings
says which rules this file turned off:

```
REPO.buri turns off 2 of 25 lint rules: hand-rolled-hex-digits, ignored-result
```

Under `default: DISABLED` it prints the smaller side, the rules that still run.

There's no per-directory exemption and no suppression comment. Turning a rule
off takes a reviewed diff to this file, not a line slipped into the file you
were already editing.

## `language`

A file's extension decides its language, which decides how the build checks
the file and how `buri format` lays it out. That applies to any file a rule's
`inputs` lists. The built-in languages are `json` (`.json`), `jsonc`
(`.jsonc`), `json5` (`.json5`), `proto` (`.proto`) and `textproto` (`.txtpb`,
`.textproto`). A `language` block gives one of them more extensions:

```textproto schema=repo
language {
    name: "jsonc"
    extensions: [".code-workspace"]
}
```

Or it declares your own language, and names the [tools](./tools.md) that
check, format and generate from it:

```textproto schema=repo
language {
    name: "lines"
    extensions: [".lines"]
    check: "//tool/lines"
    format: "//tool/lines"
}
```

| | |
|---|---|
| `name` | The language the block is about. Required. |
| `extensions` | More extensions for it, each with a leading dot. |
| `check`, `format`, `generate` | A `tool` rule under `//tool/`, or a built-in tool by its bare name, whose entry point of the same name does the work. Refused on a built-in language: [`built-in-language-tool`](../errors/built-in-language-tool.md). |

- `check` runs on each referenced file before any generator reads it, in
  `buri build`, `buri test`, `buri lint` and your editor. `format` is what
  `buri format` and your editor use. A language without one isn't checked, or
  isn't formatted.
- A tool without the entry point is
  [`tool-missing-entry-point`](../errors/tool-missing-entry-point.md), and a
  name that's no tool is [`unknown-tool`](../errors/unknown-tool.md).
- `generate` is validated the same way, but nothing runs it yet: a
  `generators` entry names its own tool.
- One extension names one language, so claiming a taken one is
  [`duplicate-extension`](../errors/duplicate-extension.md). A
  language is declared once
  ([`duplicate-language`](../errors/duplicate-language.md)).

A built-in language keeps its own check so a `.json` file means the same thing
in every repository. [`guides/json.md`](../../guides/json.md) covers that check.

## What is not here

- **No toolchain pin.** Nothing fetches a toolchain, so a pin has nothing to
  do. `buri version --verbose` prints the running executable's identity for bug
  reports. A leftover `toolchain` block gets the build-unknown-field diagnostic.
- **No `name`.** Labels are `//`-rooted, artifacts take their names from their
  package directory, and a name here would compete with the checkout directory.
  Rules in a `BUILD.buri` have no `name` either
  ([`build-files.md`](./build-files.md#labels)).
- **No defaults block.** Visibility is private unless a rule says otherwise,
  always. There's no repository-wide test timeout: a suite that needs longer
  writes `timeout_seconds` where its reader sees it.
- **No per-file or per-directory lint suppression**, and no `severity` field.
  [`rules`](#rules) turns a rule off for the whole repository. Every finding is
  a warning ([`buri lint`](../cli/lint.md)), and only `fail_on_finding` changes
  that, for every rule at once.
- **No compiler flags.** A repository-wide flag is a dialect: one source file
  would mean different things in different repositories.
- **No dependency versions or lockfile.** Your only sources are this repository
  and the `core/*` that ships with the toolchain. External repositories will
  get their own file.
- **No build settings, profiles, or optimization levels.** `buri build
  --release` is a flag on the command and part of the cache key.
- **No environment.** Actions run with an empty environment
  ([`hermeticity.md`](./hermeticity.md)), so nothing reads a variable.
- **No rule definitions.** The schema has three rule kinds. If a repository
  could define rules, reading a `BUILD.buri` would no longer tell you what
  happens.
