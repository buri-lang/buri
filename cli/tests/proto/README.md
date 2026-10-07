# The protobuf conformance suite

Protobuf's own conformance tests, run against the codecs Buri generates from a
`.proto` schema. This is the only test in this repository whose *ground truth*
comes from somewhere else.

```text
cli/tests/proto/
  README.md            this file
  run.sh               builds the testee and drives the runner
  failure_list.txt     every test expected to fail, grouped by why
  vectors.txt          recorded exchanges, replayed by cargo (no runner needed)
  record.mjs           the tap that records them
  record.py            and the script that turns a recording into vectors
  vendor/LICENSE       protobuf's licence, because two files here are theirs
  repo/                a Buri repository holding the testee
    lib/conformance/   the two vendored schemas, and their surface
    cmd/testee/        the program the runner forks
```

## Running it

```sh
cargo build -p buri
cli/tests/proto/run.sh
```

`conformance_test_runner` has to be on `PATH`, or `CONFORMANCE_TEST_RUNNER` has
to point at one. nixpkgs doesn't package it, so the flake builds it from the
vendored release, in about a minute and a half on a laptop:

```sh
nix build .#conformance-runner
CONFORMANCE_TEST_RUNNER=$PWD/result/bin/conformance_test_runner cli/tests/proto/run.sh
```

CI's `protobuf conformance` job does the same on every push, through
`vectors::proto::the_conformance_runner_passes`.

`./run.sh --update` writes any unexpected failure to `unexpected.txt` for
classification. `./run.sh --record` re-records `vectors.txt`.

**This stays out of `cargo test`**: a suite that cannot run without a C++ build
of another project is a suite that does not run. `cli/tests/vectors/proto.rs` is
the half that does, with only a Buri toolchain and a JavaScript runtime:

- `the_recorded_exchanges_still_hold` replays every request the runner sent
  about `TestAllTypesProto3` and checks each answer byte for byte. A change to
  any answer, a regression or a fix, fails it until `--record` is run again.
- `the_failure_list_is_the_recorded_failures` checks that `failure_list.txt`
  names exactly the tests recorded as failing, so the list can't drift.
- `the_recording_covers_every_fixed_class_of_bug` checks that the recording
  still reaches every class of bug the runner has found.

`--record` refuses to write anything unless the runner passed, and it writes
the runner's verdict beside each exchange.

## What was vendored, and from where

From **protobuf v35.1** (released 2026-06-11):

| Vendored file | Origin in the protobuf tree |
|---|---|
| `repo/lib/conformance/conformance.proto` | `conformance/conformance.proto`, verbatim but for its edition declaration |
| `repo/lib/conformance/test_messages_proto3.proto` | `src/google/protobuf/test_messages_proto3.proto`, **migrated to edition 2026** |
| `vendor/LICENSE` | `LICENSE` — protobuf is BSD-3-Clause, and these files carry that licence |

The reader requires `edition = "2026"`. `EDITION_2026 = 1002` is a real value in
`descriptor.proto`'s `Edition` enum, and protoc v35.1 refuses to compile a file
that declares it (*"Edition 2026 is later than the maximum supported edition
2024"*). That costs nothing: the runner never reads these schemas — it has its
own descriptors compiled in — and every wire- and JSON-affecting feature
resolves the same at 2023, 2024 and 2026 (`field_presence` `EXPLICIT`,
`enum_type` `OPEN`, `repeated_field_encoding` `PACKED`, `utf8_validation`
`VERIFY`, `message_encoding` `LENGTH_PREFIXED`, `json_format` `ALLOW`). The only
defaults that changed after 2023 are `enforce_naming_style` and
`default_symbol_visibility`, both `RETENTION_SOURCE`. Lints, not wire format.

### What was changed

Nothing was pruned: every field and import of protobuf's file is here, the
well-known types included, whose schemas come with the toolchain. The file
carries a banner saying it was migrated. The migration keeps proto3's semantics
the way protoc's own does: the file sets `features.field_presence = IMPLICIT`,
because a proto3 singular scalar has no presence, and the reference message the
runner compares against is proto3.

Everything else is intact: all fifteen scalar types in singular, `optional`,
repeated, packed and unpacked forms; a recursive message and a mutually
recursive one; a nested message and three nested enums, one with `allow_alias`
and one with a negative value; a nine-case `oneof`; and the eighteen fields that
test the field-name-to-JSON-name convention.

## The testee

`repo/cmd/testee` is a Buri binary. The runner forks it and speaks a
four-byte-little-endian-length framing over a pipe: a `ConformanceRequest` in, a
`ConformanceResponse` out, end of input ends it.

**Everything but the framing is the code under test.** The request and the
response are themselves protobuf messages, decoded and encoded by the codecs
generated from the vendored `conformance.proto`. The payload is a
`TestAllTypesProto3` from the vendored schema. The program contains no
hand-written protobuf anywhere, so a bug in the codecs shows up as a runner that
cannot talk to us at all.

Writing it is what asked for `Stdin.readBytes` and `Stdout.writeBytes` in
`platform/effect`: `Stdin.readLine` reads the stream to its end, so a program using
it cannot answer before the other side has finished speaking.

## Where it stands

```text
CONFORMANCE SUITE PASSED: 1445 successes, 1314 skipped, 36 expected failures, 0 unexpected failures.
```

The 1314 skips are the message types this testee does not implement — proto2 and
the editions variants — plus the text-format and JSPB categories. The 36
expected failures are `failure_list.txt`, which files each one under one of
three reasons and leaves no entry unexplained:

- **24 are `Any` in JSON.** proto3 JSON writes an `Any` as the message inside
  it, which needs a registry of every type a URL can name; a generated module
  has only its own schema's types, so `Any` goes out as its two fields.
- **10 are a JSON writer that cannot fail.** A `Duration` or `Timestamp` out of
  JSON's range should fail to serialize, and `encodeMJson` answers a `Json`
  rather than a `Result`.
- **2 are unknown-field retention.** Decoding skips a field the schema doesn't
  know rather than keeping its bytes, so it doesn't survive a re-encode.
  Keeping them needs a field on every generated struct, and that breaks every
  struct literal written without `..defaultM()`.

The runner also reports 13 `Recommended` warnings, which do not fail the suite.
Five are a JSON writer that cannot fail: a `FieldMask` path with no camelCase
form, and a `Value` holding NaN or an infinity. Three are a JSON object naming
one field twice, which `core/json` keeps rather than refuses. Five are
`JSON_IGNORE_UNKNOWN_PARSING_TEST`, where an unrecognised enum *name* should be
ignored rather than refused, and nothing tells the generated decoder which mode
it is in.
