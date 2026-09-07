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
to point at one. **nixpkgs does not package it** — `protobuf` there is the
library and `protoc`, and the runner is a test binary the release does not
install — so you build it from the protobuf source. `run.sh` prints the recipe
when it cannot find one: a CMake build against nixpkgs' abseil, about six
minutes on a laptop. One wrinkle: nixpkgs' `jsoncpp` ships no static library and
protobuf's CMake asks for `jsoncpp_static`, so the final link needs a
`libjsoncpp_static.dylib` symlinked to `libjsoncpp.dylib` on the library path.

`./run.sh --update` writes any unexpected failure to `unexpected.txt` for
classification. `./run.sh --record` re-records `vectors.txt`.

**This stays out of `cargo test` on purpose**, for the same reason
`editors/tree-sitter-buri/check.sh` does: a suite that cannot run without a C++
build of another project is a suite that does not run.
`cli/tests/vectors/proto.rs` is the half that does run under cargo. It replays
`vectors.txt` through the same testee, and needs only a Buri toolchain and a
JavaScript runtime.

## What was vendored, and from where

From **protobuf v35.1** (released 2026-06-11):

| Vendored file | Origin in the protobuf tree |
|---|---|
| `repo/lib/conformance/conformance.proto` | `conformance/conformance.proto`, verbatim but for its edition declaration |
| `repo/lib/conformance/test_messages_proto3.proto` | `src/google/protobuf/test_messages_proto3.proto`, **pruned and migrated to edition 2026** |
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

### What was pruned

`test_messages_proto3.proto` is built out of every construct the format has,
including three Buri's schema reader
[refuses](../../src/docs/reference/build/proto.md):

- **`import "google/protobuf/..."`** — nine imports of the well-known types. No
  bundled copy of those schemas exists here, and each one also has a JSON
  representation a generic mapping cannot produce.
- **Every field of a well-known type** — `Any`, `Duration`, `Timestamp`,
  `FieldMask`, `Struct`, `Value`, `ListValue`, `Empty`, `NullValue`, and the
  nine scalar wrappers. 46 fields.
- **Every `map<K, V>` field** — 19 of them. A map is sugar for a repeated entry
  message with a wire layout of its own, and `core/map` does not order itself
  the way a decoded one would have to.

61 lines came out. The file carries a banner saying so, and saying that the
migration took edition 2026's *own* defaults rather than preserving proto3
semantics — which is what makes the suite exercise the mapping this toolchain
implements. One test notices, under its own heading in the failure list.

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
`TestAllTypesProto3` from the pruned schema. The program contains no
hand-written protobuf anywhere, so a bug in the codecs shows up as a runner that
cannot talk to us at all.

Writing it is what asked for `Stdin.readBytes` and `Stdout.writeBytes` in
`core/effect`: `Stdin.readLine` reads the stream to its end, so a program using
it cannot answer before the other side has finished speaking.

## Where it stands

```text
CONFORMANCE SUITE PASSED: 970 successes, 1314 skipped, 456 expected failures, 0 unexpected failures.
```

The 1314 skips are the message types this testee does not implement — proto2 and
the editions variants — plus the text-format and JSPB categories. The 456
expected failures are `failure_list.txt`, which files each one under one of
seven reasons and leaves no entry unexplained.

Forty are worth naming, because they are the only ones not about the pruned
schema:

- **34 are 64-bit precision.** An `Int` is an `I64` and an `I64` is a double, so
  a value past 2^53 survives only to a double's precision and one at ±2^63 does
  not survive at all. Closing this needs a real 64-bit integer in the language.
- **2 are unknown-field retention.** Decoding skips a field the schema does not
  know rather than keeping the bytes, so they do not survive a re-encode.
- **1 is explicit presence**, and it is not a gap. The schema under test is
  edition 2026 and the reference is proto3, so the two disagree about whether a
  field set to its zero value gets written. Both are right about their own
  schema, and the same difference accounts for ~18 of the `Recommended`
  warnings.
- **2 are `core/json`'s number scanner**, which is deliberately generous and
  accepts a leading zero JSON's own grammar does not.
- **1 is duplicate keys in a JSON object**, which `core/json` does not reject.

The runner also reports ~50 `Recommended` warnings, which do not fail the suite.
They fall in the same buckets, plus one that does not: under
`JSON_IGNORE_UNKNOWN_PARSING_TEST` an unrecognised enum *name* should be ignored
rather than refused, and nothing can tell the generated decoder which mode it is
in.
