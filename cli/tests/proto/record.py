"""Turns a recorded conversation into `vectors.txt`.

The recording is one line per chunk the operating system happened to deliver,
so the frames are reassembled from their length prefixes rather than from the
line breaks -- a pipe does not promise that a write arrives as one read.
"""

import sys

HEADER = """\
# Conformance vectors: a request the protobuf runner sent, and the response the
# Buri testee gave it.
#
# One exchange per line, as `request response`, both hex, both the *body* of a
# frame with its four-byte length prefix removed. `cli/tests/vectors/proto.rs`
# replays them through the same testee, which is the whole of the conformance
# pipeline -- vendored schema, generated module, generated codecs, framing --
# with the C++ runner absent. The `vectors::lean` arrangement: an external tool
# generates, a checked-in file replays, and `cargo test` needs neither.
#
# **What these pin, exactly.** They were recorded from a run the reference
# runner reported as PASSED with `failure_list.txt` applied, so each response
# was either accepted by the reference implementation or is one of the
# divergences that file explains. Sampled deterministically, so the set is a
# slice of the surface rather than a hand-picked flattering one. What they catch
# is a change of answer -- which is what a regression is -- rather than
# non-conformance, which only the runner can decide.
#
# Recorded against protobuf v35.1. Regenerate with `./run.sh --record`.
"""

TARGET = "protobuf_test_messages.proto3.TestAllTypesProto3"


def top_level(b):
    """(field number, wire type, value) for each field of a message."""
    out, i = [], 0
    while i < len(b):
        key, shift = 0, 0
        while i < len(b):
            c = b[i]
            i += 1
            key |= (c & 0x7F) << shift
            shift += 7
            if c < 0x80:
                break
        f, w = key >> 3, key & 7
        if w == 0:
            v, shift = 0, 0
            while i < len(b):
                c = b[i]
                i += 1
                v |= (c & 0x7F) << shift
                shift += 7
                if c < 0x80:
                    break
            out.append((f, w, v))
        elif w == 2:
            n, shift = 0, 0
            while i < len(b):
                c = b[i]
                i += 1
                n |= (c & 0x7F) << shift
                shift += 7
                if c < 0x80:
                    break
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


def main(recording, destination):
    sent, received = [], []
    for line in open(recording):
        data = bytes.fromhex(line[2:].strip())
        (sent if line[0] == ">" else received).append(data)

    pairs = []
    for request, response in zip(frames(sent), frames(received)):
        fields = {f: v for f, _, v in top_level(request)}
        if fields.get(4, b"").decode("utf8", "replace") != TARGET:
            continue
        # Only an exchange that produced a payload: a skip says nothing about a
        # codec, and pinning one would pin the absence of work.
        if not any(f in (3, 4) for f, _, _ in top_level(response)):
            continue
        pairs.append((request.hex(), response.hex()))

    pairs = sorted(set(pairs))
    step = max(1, len(pairs) // 160)
    sample = pairs[::step]
    with open(destination, "w") as out:
        out.write(HEADER)
        for a, b in sample:
            out.write(f"{a} {b}\n")
    print(f"{len(sample)} vectors from {len(pairs)} exchanges")


if __name__ == "__main__":
    main(sys.argv[1], sys.argv[2])
