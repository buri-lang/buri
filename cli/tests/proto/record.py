"""Turns a recorded conversation into `vectors.txt`.

The recording is one line per chunk the operating system happened to deliver,
so the frames are reassembled from their length prefixes rather than from the
line breaks -- a pipe does not promise that a write arrives as one read.

The runner's `--verbose` report names each test as it sends it, in the order
the frames went over the pipe, so the n-th name is the n-th exchange.

    record.py <frames> <report> <failure list> <vectors.txt>
"""

import re
import sys

HEADER = """\
# Conformance vectors: every request the protobuf runner sent about the message
# under test, and the response the Buri testee gave it.
#
# One exchange per line, as `verdict test request response`. The request and
# the response are hex, each the *body* of a frame with its four-byte length
# prefix removed. The verdict is `fail` for a test failure_list.txt expects to
# fail and `pass` for every other. `cli/tests/vectors/proto.rs` replays them
# through the same testee, which is the whole of the conformance pipeline --
# vendored schema, generated module, generated codecs, framing -- with the C++
# runner absent, and checks that the `fail` lines are failure_list.txt exactly.
#
# **What these pin, exactly.** They were recorded from a run the reference
# runner reported as PASSED with `failure_list.txt` applied, so every `pass`
# response was accepted by the reference implementation and every `fail` one is
# a divergence that file explains. What they catch is a change of answer --
# which is what a regression is -- rather than non-conformance, which only the
# runner can decide.
#
# Only the skips are left out: an exchange about a message type the testee
# does not implement says nothing about a codec.
#
# Recorded against protobuf v35.1. Regenerate with `./run.sh --record`.
"""

TARGET = "protobuf_test_messages.proto3.TestAllTypesProto3"


def varint(b, i):
    v, shift = 0, 0
    while i < len(b):
        c = b[i]
        i += 1
        v |= (c & 0x7F) << shift
        shift += 7
        if c < 0x80:
            break
    return v, i


def top_level(b):
    """(field number, wire type, value) for each field of a message."""
    out, i = [], 0
    while i < len(b):
        key, i = varint(b, i)
        f, w = key >> 3, key & 7
        if w == 0:
            v, i = varint(b, i)
            out.append((f, w, v))
        elif w == 2:
            n, i = varint(b, i)
            out.append((f, w, b[i : i + n]))
            i += n
        elif w == 1:
            out.append((f, w, b[i : i + 8]))
            i += 8
        elif w == 5:
            out.append((f, w, b[i : i + 4]))
            i += 4
        else:
            break
    return out


def frames(chunks):
    b = b"".join(chunks)
    out, i = [], 0
    while i + 4 <= len(b):
        n = int.from_bytes(b[i : i + 4], "little")
        i += 4
        out.append(b[i : i + n])
        i += n
    return out


def listed(path):
    out = set()
    for line in open(path):
        line = line.split("#", 1)[0].strip()
        if line:
            out.add(line)
    return out


def main(recording, report, failure_list, destination):
    sent, received = [], []
    for line in open(recording):
        data = bytes.fromhex(line[2:].strip())
        (sent if line[0] == ">" else received).append(data)
    names = re.findall(r"^conformance test: name=(\S+?), request=", open(report).read(), re.M)
    requests, responses = frames(sent), frames(received)
    if not (len(names) == len(requests) == len(responses)):
        sys.exit(f"{len(names)} names, {len(requests)} requests, {len(responses)} responses")

    expected = listed(failure_list)
    lines = []
    for name, request, response in zip(names, requests, responses):
        fields = {f: v for f, _, v in top_level(request)}
        if fields.get(4, b"").decode("utf8", "replace") != TARGET:
            continue
        # Field 5 is `skipped`.
        if any(f == 5 for f, _, _ in top_level(response)):
            continue
        verdict = "fail" if name in expected else "pass"
        lines.append(f"{verdict} {name} {request.hex()} {response.hex()}\n")

    missing = expected - {line.split(" ")[1] for line in lines}
    if missing:
        sys.exit("listed but never sent: " + ", ".join(sorted(missing)))
    with open(destination, "w") as out:
        out.write(HEADER)
        out.writelines(lines)
    print(f"{len(lines)} vectors, {sum(l.startswith('fail') for l in lines)} expected to fail")


if __name__ == "__main__":
    main(*sys.argv[1:5])
