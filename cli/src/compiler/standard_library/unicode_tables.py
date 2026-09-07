"""Writes the Unicode tables `core/char` and `core/str` read.

The tables are checked in, so a build needs no network and no Unicode
installation. This script is how they were made, and running it again is how
they move to a new Unicode version.

    curl -sSLO https://www.unicode.org/Public/16.0.0/ucd/UnicodeData.txt
    curl -sSLO https://www.unicode.org/Public/16.0.0/ucd/CaseFolding.txt
    curl -sSLO https://www.unicode.org/Public/16.0.0/ucd/CompositionExclusions.txt
    curl -sSLO https://www.unicode.org/Public/16.0.0/ucd/DerivedCoreProperties.txt
    curl -sSLO https://www.unicode.org/Public/16.0.0/ucd/auxiliary/GraphemeBreakProperty.txt
    curl -sSLO https://www.unicode.org/Public/16.0.0/ucd/emoji/emoji-data.txt
    python3 cli/src/compiler/standard_library/unicode_tables.py --ucd <that directory>

**Unicode 16.0**, which is the version `cli/runtime/char.rs`'s General Category
table names too. Both have to move together: they are one library's answers
about one set of characters.

Every table is an ASCII string of fixed-width fields, and a code point is four
base-36 digits -- 36^4 is 1679616, which covers U+10FFFF with room to spare, and
the digits `0`-`9a`-`z` sort the way the numbers do, so a binary search may
compare the text. The one exception is a *value* table (a decomposition, a case
folding), which holds the characters themselves: there is no way to turn a
number into a `Char` in Buri without a `Result`, and slicing a string that
already holds the answer avoids needing one.
"""

import argparse
import os
import sys

D36 = "0123456789abcdefghijklmnopqrstuvwxyz"

CHAR_FILE = "sources/character.buri"
STR_FILE = "sources/str.buri"
START = "// --- Generated Unicode tables"
END = "// --- End of the generated Unicode tables"

# The order `str.buri`'s `graphemeClass` reads. `Other` is 0, so a range table
# that lists nothing for a code point still answers.
GCB = [
    "Other",
    "CR",
    "LF",
    "Control",
    "Extend",
    "ZWJ",
    "Regional_Indicator",
    "Prepend",
    "SpacingMark",
    "L",
    "V",
    "T",
    "LV",
    "LVT",
]
EXTENDED_PICTOGRAPHIC = 16
INCB_LINKER = 32
INCB_CONSONANT = 64
INCB_EXTEND = 128

# The first of each Hangul jamo run, and how many there are (UAX #15 §3.12).
HANGUL_BASE, HANGUL_COUNT = 0xAC00, 11172
LEADING, VOWEL, TRAILING = 0x1100, 0x1161, 0x11A7


def b36(n, width):
    out = ""
    for _ in range(width):
        out = D36[n % 36] + out
        n //= 36
    if n:
        raise ValueError(f"{n} does not fit in {width} base-36 digits")
    return out


def quote(text):
    """The literal the Buri formatter would print for `text`."""
    out = ['"']
    for c in text:
        if c == '"':
            out.append('\\"')
        elif c == "\\":
            out.append("\\\\")
        elif c == "\n":
            out.append("\\n")
        elif c == "\t":
            out.append("\\t")
        elif ord(c) < 0x20 or ord(c) == 0x7F:
            raise ValueError(f"U+{ord(c):04X} would not survive a format")
        else:
            out.append(c)
    out.append('"')
    return "".join(out)


def read_unicode_data(ucd):
    """General Category, canonical and compatibility decompositions, and CCC."""
    category, decomposition, combining = {}, {}, {}
    lines = open(os.path.join(ucd, "UnicodeData.txt"), encoding="utf-8").read().splitlines()
    i = 0
    while i < len(lines):
        f = lines[i].split(";")
        cp = int(f[0], 16)
        if f[1].endswith(", First>"):
            end = int(lines[i + 1].split(";")[0], 16)
            for c in range(cp, end + 1):
                category[c] = f[2]
                if f[3] != "0":
                    combining[c] = int(f[3])
            i += 2
            continue
        category[cp] = f[2]
        if f[3] != "0":
            combining[cp] = int(f[3])
        if f[5]:
            decomposition[cp] = f[5]
        i += 1
    return category, decomposition, combining


def read_ranges(path, wanted):
    """`{code point: value}` for every `; value` line naming one of `wanted`."""
    out = {}
    for line in open(path, encoding="utf-8"):
        line = line.split("#")[0].strip()
        if not line:
            continue
        fields = [x.strip() for x in line.split(";")]
        value = "; ".join(fields[1:])
        if value not in wanted:
            continue
        span = fields[0]
        lo, _, hi = span.partition("..")
        for cp in range(int(lo, 16), int(hi or lo, 16) + 1):
            out[cp] = value
    return out


def to_ranges(value_of):
    """`[(start, end, value)]` over every code point whose value is not None."""
    out, start, current = [], None, None
    for cp in range(0x110000 + 1):
        value = value_of(cp) if cp <= 0x10FFFF else None
        if value != current:
            if current is not None:
                out.append((start, cp - 1, current))
            start, current = cp, value
    return out


def range_table(ranges, width):
    """Ranges as start, end and value, each field fixed width."""
    return "".join(
        b36(lo, 4) + b36(hi, 4) + (b36(v, width) if width else "") for lo, hi, v in ranges
    )


def mapping_table(mapping):
    """A sorted key table, an offset and length table, and the values."""
    keys, offsets, lengths, values = [], [], [], []
    at = 0
    for cp in sorted(mapping):
        target = mapping[cp]
        keys.append(b36(cp, 4))
        offsets.append(b36(at, 4))
        lengths.append(b36(len(target), 1))
        values.append("".join(chr(c) for c in target))
        at += len(target)
    return "".join(keys), "".join(offsets), "".join(lengths), "".join(values)


def build(ucd):
    category, raw_decomposition, combining = read_unicode_data(ucd)

    canonical = {
        cp: [int(x, 16) for x in v.split()]
        for cp, v in raw_decomposition.items()
        if not v.startswith("<")
    }
    compatible = {
        cp: [int(x, 16) for x in v.split()[1:]]
        for cp, v in raw_decomposition.items()
        if v.startswith("<")
    }

    def expand(cp, tables):
        for table in tables:
            if cp in table:
                return [d for c in table[cp] for d in expand(c, tables)]
        return [cp]

    full_canonical = {cp: expand(cp, [canonical]) for cp in canonical}
    full_compatible = {
        cp: expand(cp, [canonical, compatible]) for cp in list(canonical) + list(compatible)
    }

    exclusions = set()
    for line in open(os.path.join(ucd, "CompositionExclusions.txt"), encoding="utf-8"):
        line = line.split("#")[0].strip()
        if line:
            exclusions.add(int(line, 16))

    # A primary composite: a canonical decomposition of two characters that is
    # neither listed as an exclusion nor headed by a non-starter (UAX #15).
    composition = {}
    for cp, target in canonical.items():
        if len(target) != 2 or cp in exclusions:
            continue
        if combining.get(target[0], 0) != 0:
            continue
        composition[(target[0], target[1])] = cp

    folding = {}
    for line in open(os.path.join(ucd, "CaseFolding.txt"), encoding="utf-8"):
        line = line.split("#")[0].strip()
        if not line:
            continue
        fields = [x.strip() for x in line.split(";")]
        if fields[1] in ("C", "F"):
            folding[int(fields[0], 16)] = [int(x, 16) for x in fields[2].split()]

    breaks = read_ranges(
        os.path.join(ucd, "GraphemeBreakProperty.txt"), set(GCB)
    )
    pictographic = read_ranges(
        os.path.join(ucd, "emoji-data.txt"), {"Extended_Pictographic"}
    )
    conjunct = read_ranges(
        os.path.join(ucd, "DerivedCoreProperties.txt"),
        {"InCB; Linker", "InCB; Consonant", "InCB; Extend"},
    )

    def grapheme_class(cp):
        value = GCB.index(breaks.get(cp, "Other"))
        if cp in pictographic:
            value += EXTENDED_PICTOGRAPHIC
        if conjunct.get(cp) == "InCB; Linker":
            value += INCB_LINKER
        if conjunct.get(cp) == "InCB; Consonant":
            value += INCB_CONSONANT
        if conjunct.get(cp) == "InCB; Extend":
            value += INCB_EXTEND
        return value or None

    def punctuation(cp):
        return 0 if category.get(cp, "Cn").startswith("P") else None

    def printable(cp):
        # Python's `str.isprintable`: neither Other nor Separator, plus a space.
        general = category.get(cp, "Cn")
        return 0 if cp == 0x20 or general[0] not in "CZ" else None

    def combining_class(cp):
        return combining.get(cp) or None

    fold_keys, fold_offsets, fold_lengths, fold_values = mapping_table(folding)
    nfd_keys, nfd_offsets, nfd_lengths, nfd_values = mapping_table(full_canonical)
    nfkd_keys, nfkd_offsets, nfkd_lengths, nfkd_values = mapping_table(full_compatible)

    compose_keys = "".join(
        b36(a, 4) + b36(b, 4) for a, b in sorted(composition)
    )
    compose_values = "".join(chr(composition[pair]) for pair in sorted(composition))

    return {
        CHAR_FILE: [
            (
                "PUNCTUATION",
                "General Category `P`, as sorted, disjoint code-point ranges.",
                range_table(to_ranges(punctuation), 0),
            ),
            (
                "PRINTABLE",
                "Everything outside General Categories `C` and `Z`, plus the space.",
                range_table(to_ranges(printable), 0),
            ),
        ],
        STR_FILE: [
            (
                "GRAPHEME_CLASS",
                "Grapheme_Cluster_Break, with Extended_Pictographic at 16 and "
                "the two Indic_Conjunct_Break values at 32 and 64.",
                range_table(to_ranges(grapheme_class), 2),
            ),
            (
                "COMBINING_CLASS",
                "Canonical_Combining_Class, for every character whose class is "
                "not zero.",
                range_table(to_ranges(combining_class), 2),
            ),
            ("NFD_KEYS", "Every character with a canonical decomposition.", nfd_keys),
            ("NFD_OFFSETS", "Where each canonical decomposition starts.", nfd_offsets),
            ("NFD_LENGTHS", "How long each canonical decomposition is.", nfd_lengths),
            ("NFD_VALUES", "The canonical decompositions, run together.", nfd_values),
            (
                "NFKD_KEYS",
                "Every character with a compatibility or canonical decomposition.",
                nfkd_keys,
            ),
            ("NFKD_OFFSETS", "Where each compatibility decomposition starts.", nfkd_offsets),
            ("NFKD_LENGTHS", "How long each compatibility decomposition is.", nfkd_lengths),
            ("NFKD_VALUES", "The compatibility decompositions, run together.", nfkd_values),
            ("COMPOSE_KEYS", "Each primary composite's two-character source.", compose_keys),
            ("COMPOSE_VALUES", "The primary composites themselves.", compose_values),
            ("FOLD_KEYS", "Every character full case folding changes.", fold_keys),
            ("FOLD_OFFSETS", "Where each folding starts.", fold_offsets),
            ("FOLD_LENGTHS", "How long each folding is.", fold_lengths),
            ("FOLD_VALUES", "The foldings, run together.", fold_values),
            (
                "HANGUL_SYLLABLES",
                "U+AC00 to U+D7A3 in order, so composition can index them.",
                "".join(chr(HANGUL_BASE + i) for i in range(HANGUL_COUNT)),
            ),
            ("HANGUL_LEADING", "The nineteen leading jamo.", "".join(chr(LEADING + i) for i in range(19))),
            ("HANGUL_VOWEL", "The twenty-one vowel jamo.", "".join(chr(VOWEL + i) for i in range(21))),
            (
                "HANGUL_TRAILING",
                "The twenty-seven trailing jamo, after one place-holder for a "
                "syllable that has none.",
                "".join(chr(TRAILING + i) for i in range(28)),
            ),
        ],
    }


def render(tables):
    out = [
        START + " (Unicode 16.0) ---------------------------",
        "//",
        "// Written by `cli/src/compiler/standard_library/unicode_tables.py`, which",
        "// says what the fields are. Regenerate rather than edit.",
    ]
    for name, why, text in tables:
        out.append("")
        out.append(f"/// {why}")
        out.append(f"let {name}: Str = {quote(text)};")
    out.append("")
    out.append(END + " -------------------")
    return "\n".join(out) + "\n"


def splice(path, body):
    text = open(path, encoding="utf-8").read()
    lines = text.splitlines(keepends=True)
    first = next((i for i, l in enumerate(lines) if l.startswith(START)), None)
    last = next((i for i, l in enumerate(lines) if l.startswith(END)), None)
    if first is None or last is None:
        raise SystemExit(f"{path} has no generated region")
    open(path, "w", encoding="utf-8").write(
        "".join(lines[:first]) + body + "".join(lines[last + 1 :])
    )


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--ucd", required=True, help="a directory of Unicode data files")
    args = parser.parse_args()
    here = os.path.dirname(os.path.abspath(__file__))
    tables = build(args.ucd)
    for relative, entries in tables.items():
        splice(os.path.join(here, relative), render(entries))
        total = sum(len(text) for _, _, text in entries)
        print(f"{relative}: {total} characters", file=sys.stderr)
        for name, _, text in entries:
            print(f"    {name}: {len(text)}", file=sys.stderr)


if __name__ == "__main__":
    main()
