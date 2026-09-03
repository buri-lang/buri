# The protobuf conformance suite

Protobuf's own conformance tests, run against the codecs Buri generates from a
`.proto` schema. It is the only test in this repository whose *ground truth*
comes from somewhere else: everything else here checks that Buri does what Buri
says, and this checks that Buri does what protobuf says.

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

## What was vendored, and from where

From **protobuf v35.1** (released 2026-06-11), the current stable release:

| Vendored file | Origin in the protobuf tree |
|---|---|
| `repo/lib/conformance/conformance.proto` | `conformance/conformance.proto`, verbatim |
| `repo/lib/conformance/test_messages_proto3.proto` | `src/google/protobuf/test_messages_proto3.proto`, **pruned and migrated to edition 2026** |
| `vendor/LICENSE` | `LICENSE` — protobuf is BSD-3-Clause, and these files carry that licence |

`conformance.proto` needed no change but its declaration: it is a plain schema
with nothing in it the reader refuses, so migrating it was replacing
`syntax = "proto3"` with `edition = "2026"`.

## The edition, and what protoc thinks of it

The reader requires `edition = "2026"`. Two facts about that, from v35.1's own
source:

- **`EDITION_2026 = 1002` is in `descriptor.proto`'s `Edition` enum.** It is a
  real, reserved edition value.
- **protoc v35.1 refuses to compile a file that declares it**: *"Edition 2026 is
  later than the maximum supported edition 2024"*. The maximum it implements is
  2024.

So the vendored schemas here declare an edition no protobuf toolchain will
compile yet. That costs nothing: the runner never reads them — it has its own
descriptors compiled in — and protobuf's own resolution rule ("a feature takes
the default of the closest edition at or before it") gives 2026 a fully
determined feature set from `descriptor.proto` alone. Every wire- and
JSON-affecting feature resolves the same at 2023, 2024 and 2026:

| feature | value at 2026 |
|---|---|
| `field_presence` | `EXPLICIT` |
| `enum_type` | `OPEN` |
| `repeated_field_encoding` | `PACKED` |
| `utf8_validation` | `VERIFY` |
| `message_encoding` | `LENGTH_PREFIXED` |
| `json_format` | `ALLOW` |

The only defaults that changed after 2023 are `enforce_naming_style` and
`default_symbol_visibility`, both `RETENTION_SOURCE` — lints, not wire format.

## What was pruned, and why that is stated rather than hidden

`test_messages_proto3.proto` is not. It is deliberately built out of every
construct protobuf has, including three Buri's schema reader
[refuses](../../src/docs/reference/build/proto.md):

- **`import "google/protobuf/..."`** — nine imports of the well-known types.
  There is no bundled copy of those schemas here, and each of them additionally
  has a JSON representation of its own that a generic mapping cannot produce.
- **Every field of a well-known type** — `Any`, `Duration`, `Timestamp`,
  `FieldMask`, `Struct`, `Value`, `ListValue`, `Empty`, `NullValue`, and the
  nine scalar wrappers. 46 fields.
- **Every `map<K, V>` field** — 19 of them. A map is sugar for a repeated entry
  message with a wire layout of its own, and `core/map` is not ordered the way a
  decoded one would have to be.

61 lines came out in total, and the file carries a banner saying so — and saying
that it was migrated to edition 2026 with the edition's *own* defaults rather
than by a semantics-preserving migration. A faithful migration would have put
`features.field_presence = IMPLICIT` on every singular field to keep proto3's
behaviour; taking the default instead is what makes the suite exercise the
mapping this toolchain implements. The one test that notices is in the failure
list under its own heading.

**The pruning alternative was worse.** Teaching the schema reader to skip a construct it does
not support would mean a schema that silently means something other than what it
says — the field would vanish and nothing would mention it. Deleting the fields
in a copy, saying which, and listing every test that the deletion forfeits is
the version of this that a reader can audit.

Everything else is intact: all fifteen scalar types in singular, `optional`,
repeated, packed and unpacked forms; a recursive message and a mutually
recursive one; a nested message and three nested enums, one with `allow_alias`
and one with a negative value; a nine-case `oneof`; and the eighteen fields that
exist to test the field-name-to-JSON-name convention.

## The testee

`repo/cmd/testee` is a Buri binary. The runner forks it and speaks a
four-byte-little-endian-length framing over a pipe: a `ConformanceRequest` in, a
`ConformanceResponse` out, end of input ends it.

**Everything but the framing is the code under test.** The request and the
response are themselves protobuf messages, decoded and encoded by the codecs
generated from the vendored `conformance.proto`; the payload is a
`TestAllTypesProto3` from the pruned schema. There is no hand-written protobuf
anywhere in the program — a bug in the codecs shows up as a runner that cannot
talk to us at all, which is a louder failure than a wrong answer.

**Why a Buri binary rather than a JavaScript shim.** A shim that framed the
protocol and called into the compiled module was the other candidate, and it
does not work: a built artifact is a program, not a module — it exports nothing
and its function names are mangled — so there is nothing for a shim to call. It
would have had to implement `ConformanceRequest` in JavaScript, which is a
second protobuf implementation sitting between the runner and the one under
test.

What a Buri binary needed was the ability to read and write *octets* on standard
input and output. `Stdin.readLine` reads the stream to its end, so a program
using it cannot answer before the other side has finished speaking — which a
request/response protocol requires. So `Stdin.readBytes` and `Stdout.writeBytes`
were added to `core/effect`, with implementations in `core/host` and
`core/host/testing` and two intrinsics behind them. That is a capability the
language wanted anyway; this is just what asked for it first.

## Running it

```sh
cargo build -p buri
cli/tests/proto/run.sh
```

`conformance_test_runner` has to be on `PATH`, or `CONFORMANCE_TEST_RUNNER` has
to point at one. **nixpkgs does not package it** — `protobuf` there is the
library and `protoc`, and the runner is a test binary the release does not
install — so it has to be built from the protobuf source. `run.sh` prints the
recipe when it cannot find one; it is a CMake build against nixpkgs' abseil,
about six minutes on a laptop. One wrinkle worth writing down: the nixpkgs
`jsoncpp` ships no static library and protobuf's CMake asks for
`jsoncpp_static`, so the final link needs a `libjsoncpp_static.dylib` symlinked
to `libjsoncpp.dylib` on the library path.

`./run.sh --update` writes any unexpected failure to `unexpected.txt` for
classification. `./run.sh --record` re-records `vectors.txt`.

**This is not part of `cargo test`, on purpose** — the same reasoning as
`editors/tree-sitter-buri/check.sh`. A suite that cannot run without a C++ build
of another project is a suite that does not run. `cli/tests/vectors/proto.rs` is
the half that does run under cargo: it replays `vectors.txt` through the same
testee, needing only a Buri toolchain and a JavaScript runtime.

## Where it stands

```text
CONFORMANCE SUITE PASSED: 970 successes, 1314 skipped, 456 expected failures, 0 unexpected failures.
```

The 1314 skips are the message types this testee does not implement — proto2 and
the editions variants — plus the text-format and JSPB categories. The 456
expected failures are `failure_list.txt`, where each is filed under one of seven
reasons, and no entry is unexplained.

Forty of them are worth naming here because they are the only ones that are not
about the pruned schema:

- **34 are 64-bit precision.** An `Int` is an `I64` and an `I64` is a double, so
  a value past 2^53 survives only to a double's precision and one at ±2^63 does
  not survive at all. Closing this means a real 64-bit integer in the language.
- **2 are unknown-field retention.** Decoding skips a field the schema does not
  know; it does not keep the bytes, so they do not survive a re-encode.
- **1 is explicit presence**, and it is not a gap: the schema under test is
  edition 2026 and the reference is proto3, so they disagree about whether a
  field set to its zero value is written. Both are right about their own schema.
  The same difference is ~18 of the `Recommended` warnings.
- **2 are `core/json`'s number scanner**, which is deliberately generous and
  accepts a leading zero JSON's own grammar does not.
- **1 is duplicate keys in a JSON object**, which `core/json` does not reject.

The runner also reports ~50 `Recommended` warnings, which do not fail the suite.
They fall in the same buckets, plus one that does not: under
`JSON_IGNORE_UNKNOWN_PARSING_TEST` an unrecognised enum *name* should be ignored
rather than refused, and the generated decoder has no way to be told which mode
it is in.

## What the suite found

Six real defects, each fixed rather than listed. They are the reason this
directory exists:

1. **A 32-bit field did not truncate.** The wire format has one integer
   encoding, so a `uint32` field can arrive carrying 2^33; protobuf reads the
   low 32 bits. Buri kept the whole number, and disagreed with every other
   implementation. Reading the low bits *as such* rather than truncating
   afterwards matters too, because a 64-bit varint has already rounded by the
   time an `Int` could be masked.
2. **A tag was read as 32 bits.** A five-byte varint can carry more than 32,
   and the low half of one naming field 2147483649 names field 1 — so a message
   was decoded as a different message instead of being rejected. Field numbers
   above 2^29−1 and over-long tags are now refused.
3. **NaN and the infinities were written as bare words**, which is not JSON at
   all. proto3 spells all three as strings.
4. **A singular message field arriving twice replaced rather than merged.** The
   specification asks for a recursive merge, and it is what makes a message
   splittable across two encodings of itself.
5. **`\uXXXX` was not implemented in `core/json`**, surrogate pairs included,
   and control characters were written raw into strings — both of which made
   documents this toolchain could not read and other tools would not accept.
   `\b` and `\f` were missing from the escape table as well.
6. **JSON numbers were not checked.** proto3 JSON rejects a value the field
   cannot hold rather than truncating it — the opposite of what the binary
   format does — and admits a quoted number only in JSON's own grammar. Buri
   accepted out-of-range values, ` 1`, and `1 `.

Two more came out of writing the testee rather than running it: `[packed=false]`
was ignored, so an explicitly unpacked field was written packed; and proto3
JSON's rule that a field is *accepted* under the schema's own name as well as
its camelCase one was not implemented.
