## What it does

Formats in place: `.buri` sources, `BUILD.buri` and `REPO.buri`, the JSON and
other files a rule's `inputs` lists, and the Buri in documentation (every
```` ```buri ```` fence in a markdown file, and every example in a `///` or
`//!` comment). In a document only fence bodies change; the prose is yours.
There are no options, so there's nothing to configure or argue about.

## What to format

```
buri format                        # the whole repository
buri format libs/greeting          # a path
buri format //libs/greeting/...    # a label
```

A **path** is a file or directory, and formats everything under it, markdown
included. A **label** names packages, and formats the sources their rules
declare plus their `BUILD.buri`, as it does for `buri gen` and `buri lint`.

Both are repository-absolute and can be mixed in one invocation. A path that
isn't there, or a label that names no package, exits `2`.

Formatting is a fixed point: a second run changes nothing. That's what lets
`buri gen` and `buri format` write the same file without fighting.

The formatter sorts the leading run of imports: `core/*` before `//*`, then by
path, then by clause, with one blank line between the two groups. That's why
there's no `unsorted-imports` lint. An import written after a declaration stays
put, because moving it could change what the module means.

## Type declarations

Width decides almost every break, except in struct and enum declarations. There,
every field or variant gets its own line with a trailing comma, however short:

```buri
export enum Hello {
    World,
    Now(Bool),
}
```

A type reads the same at one member as at ten, and adding a member is a one-line
diff. An empty body stays shut (`struct S {}`, `enum E {}`), and a tuple struct
like `struct Meters(F64);` stays as written.

Struct literals and match arms break on width like everything else.

## A comment beside the code

A comment at the end of a line stays there, one space after the code. Every
other comment goes on its own line above what it was above. Comments never
affect layout, since the formatter measures a line without them, and it never
rewraps them.

## A file with a syntax error

The declaration the parser couldn't read comes back byte for byte, and the rest
of the file is laid out as usual. The result is still a fixed point, keeps every
comment and token, and fits the margin everywhere outside the broken region.

`buri format` names each file it could only partly read. `--check` exits `1`
for a file that would change, a file with a syntax error, or a file the
formatter refused outright.

## JSON files

A `.json`, `.jsonc` or `.json5` file is formatted only when some rule's `inputs`
lists it, so a stray `package.json` is left alone.

```json
{ "$schema": "regions.schema.json", "regions": ["eu-west", "us-east"] }
```

It gets the same width and indent as `.buri` files, and a list that doesn't fit
puts one element per line. Every comment survives, placed as in `.buri`.
Scalars keep their spelling. `.json` and `.jsonc` never get a trailing comma;
`.json5` gets one wherever a list breaks. The formatter never adds `"$schema"`.

A file that doesn't parse is left as is, and `--check` exits `1` for it.

## Text format files

A `.txtpb` or `.textproto` file a rule's `inputs` lists gets one field per line.
A scalar takes `:`, a message takes `{ }`, `< >` becomes `{ }`, and a trailing
`;` or `,` goes. A list, or a message inside one, stays on one line when it
fits. Values keep their spelling and every comment survives.

```textproto ignore why="a data file, not a build file"
name: "api"
ports: [80, 443]
limits {
    cpu: 0.5
}
```

## Files in a language of your own

A file in a language `REPO.buri` declares is laid out by its tool's `format`,
only when some rule's `inputs` lists it, at the same width and indent. A file
the tool won't format is left as is, and `--check` exits `1`. A language with no
`format` is left alone. See [`build/tools.md`](../build/tools.md).

## Build files

Fields come back in the order the schema declares them: `library` before
`binary`, `sources` before `dependencies` before `test`. An unknown field keeps
its place at the end. Repeated fields, like two `tag` blocks or the entries of
an `outputs` list, keep the order you wrote.

One field per line, four-space indent, `name: value` for a scalar, `name { … }`
for a block. A short list stays on one line; a long one gets one element per line
and a trailing comma. Every comment stays with the field beneath it.

`buri gen` writes build files through this same printer, so its output passes
`format --check`.

`--check` writes nothing and exits `1` if anything would change or any source
has a syntax error. A build file that doesn't read stops the run with exit `2`.
