#!/bin/sh
# The colour test: one file, two layers, a named answer for every token.
#
#   editors/tree-sitter-buri/check_highlighting.sh
#
# `check.sh` proves the grammar parses Buri and that every query compiles. A
# query that compiles still colours nothing if it names a field the grammar
# does not put where the pattern looks, so this asks the other question: what
# colour does each token actually come out?
#
# The subject is `fixture/lib/reference/sections.buri` — the struct section of
# `cli/src/docs/lang/types.md` 5.6 and the import forms of
# `cli/src/docs/lang/modules.md` 4.1, written so they compile. It is a real
# repository, so `buri lsp` can analyse it and answer about it too.
#
# Three assertions, all by name rather than by count:
#
#   1. Every token in GRAMMAR below gets the capture named there, from the real
#      query engine (`tree-sitter query`) over the real `highlights.scm`.
#   2. Every identifier in the file is either captured by some pattern or is
#      named in UNCOLOURED — the short list of things a grammar cannot know,
#      which is a local's use, a parameter's use, and a module alias's use.
#   3. `buri lsp` answers `textDocument/semanticTokens/full` about the same
#      file, and every token in SERVER gets the type named there, and the
#      modifier where one is named — including every name on the UNCOLOURED
#      list, which is the point: the layer that resolves is the layer that
#      colours what the grammar left alone.
#
# Positions are the ones both layers speak in: a zero-based row. A row and the
# token's text name a token, rather than a column, so that a line reflowing by
# a space does not read as a regression — and where one row writes the same
# word twice in two roles, a fourth column pins which one is meant.
#
# In SERVER a fourth column is a *modifier* instead: `readonly` for one the
# token must carry, `!readonly` for one it must not.

set -eu

here=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
root=$(CDPATH= cd -- "$here/../.." && pwd)
query=$root/editors/zed/languages/buri/highlights.scm
source=$here/fixture/lib/reference/sections.buri

for tool in tree-sitter cargo; do
  if ! command -v "$tool" >/dev/null 2>&1; then
    echo "$tool is not on PATH" >&2
    exit 2
  fi
done

work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT

# --- What the grammar is asked for ------------------------------------------
#
# One line a token: the row it is on, its text, and the capture Zed keeps for
# it. Where two patterns capture the same node the later one wins, which is why
# `UserId(` is a constructor rather than the type the shape rule says it is.
GRAMMAR="
5 Alloc type
6 list namespace
7 round function
7 rounded function
8 fromInt function
8 fromFloat function
11 MAX_RETRIES constant
14 UserId type
17 id property
18 name property
20 passwordHash property
24 Meters type
26 A type.parameter 12
26 A type 18
29 Source type
31 Shelf constructor
33 Catalogue constructor
33 page property
40 describe function
40 hash variable.parameter
40 Alloc type
41 UserId constructor
44 User type
44 id property
44 name property
44 passwordHash property
45 User type
46 u variable
46 name property
47 Meters constructor
47 rounded function.call
48 0 property
49 Pair constructor
49 name property
50 range function.method
50 MAX_RETRIES constant
51 empty function.method
51 Int type
52 Source type
52 Catalogue constructor
53 len function.method
55 Shelf constructor
57 Catalogue constructor
57 page property
58 other variable
58 fromFloat function.call
62 Source type
64 label function
64 self variable.builtin
67 _ variable
74 anonymous function
74 hash variable.parameter
74 User type
75 seed variable
75 User type
75 UserId constructor
75 id property
75 name property
75 passwordHash property
76 seed variable
76 name property
83 credential function
83 u variable.parameter
84 passwordHash property
85 id property
"

# --- What the grammar is not asked for ---------------------------------------
#
# An identifier with no capture at all, and the reason there is none: telling a
# local from a parameter from a module alias is scope tracking, and Zed does
# not read a `locals.scm`. Each of these is answered by the server instead —
# SERVER below names every one of them.
UNCOLOURED="
43 hash
44 hash
48 d
49 raw
49 shorthand
50 list
51 list
53 u2
53 counts
53 nothing
56 origin
57 page
57 shelf
58 other
58 pair
75 hash
84 u
85 u
86 stored
87 mark
88 stored
"

# --- What the server is asked for --------------------------------------------
#
# The same file, the same rows, and the answers only a resolver has: `Alloc` is
# a trait and not merely a capitalized word, `list` is a module, `Shelf` is a
# variant, and every name on UNCOLOURED is something.
SERVER="
5 Alloc interface
6 list namespace
7 rounded function
8 fromInt function
11 MAX_RETRIES variable readonly
14 UserId type
17 id property
20 passwordHash property
24 Meters type
26 A typeParameter
26 B typeParameter
29 Source type
31 Shelf enumMember
33 page property
40 describe function
40 C typeParameter
40 hash variable
40 Alloc interface
41 UserId type
41 id variable !readonly
43 hash variable
44 User type
44 id property
44 passwordHash property
45 id variable
45 shorthand variable
46 u variable
46 name property
47 d variable
47 Meters type
47 rounded function
49 raw variable
49 shorthand variable
50 list namespace
50 range function
50 MAX_RETRIES variable readonly
51 list namespace
51 empty function
52 Source type
52 Catalogue enumMember
53 u2 variable
53 counts variable
53 nothing variable
53 len method
55 Shelf enumMember
56 origin variable
57 shelf variable
57 label method
57 fromInt function
58 other variable
58 pair variable
58 fromFloat function
62 Source type
64 label method
74 anonymous function
74 hash variable
74 User type
75 seed variable
75 User type
75 id property
75 name property
75 passwordHash property
75 hash variable
76 seed variable
76 name property
83 credential function
83 u variable
83 User type
84 stored variable
84 u variable
84 passwordHash property
85 mark variable
85 u variable
85 id property
86 stored variable
87 mark variable
88 stored variable
"

# ---------------------------------------------------------------------------
# Layer one: the query engine
# ---------------------------------------------------------------------------

cd "$here"
tree-sitter generate >/dev/null

# `row column capture text`, in the order the engine reports them, which is the
# order a later pattern overrides an earlier one in.
captures() {
  sed -n 's/^ *pattern: *[0-9]*, capture: [0-9]* - \([A-Za-z.]*\), start: (\([0-9]*\), \([0-9]*\)).*text: `\(.*\)`$/\2 \3 \1 \4/p'
}

tree-sitter query -c "$query" "$source" >"$work/raw" 2>"$work/raw.err" || {
  echo "FAIL  the highlight query did not run against the fixture" >&2
  cat "$work/raw.err" >&2
  exit 1
}
captures <"$work/raw" >"$work/captures"

printf '(identifier) @identifier\n' >"$work/identifiers.scm"
tree-sitter query -c "$work/identifiers.scm" "$source" 2>/dev/null | captures >"$work/identifiers"

printf '%s\n' "$GRAMMAR" >"$work/expected.grammar"
printf '%s\n' "$UNCOLOURED" >"$work/uncoloured"

failures=$(
  awk '
    # The winner at a position is the last capture reported for it.
    FILENAME == ARGV[1] { won[$1 " " $2] = $3; text[$1 " " $2] = $4; next }

    # Every identifier is either captured or excused by name.
    FILENAME == ARGV[2] { identifier[$1 " " $2] = $4; next }

    # A three-field row names every occurrence of a word on that row; a
    # four-field row names the one at that column.
    FILENAME == ARGV[3] && NF == 4 { pinned[$1 " " $4] = $3; pinnedText[$1 " " $4] = $2; next }
    FILENAME == ARGV[3] && NF == 3 { want[$1 " " $2] = $3; next }
    FILENAME == ARGV[4] && NF == 2 { excused[$1 " " $2] = 1; next }

    END {
      for (at in won) {
        split(at, part, " ")
        key = part[1] " " text[at]
        if (key in want) {
          seen[key] = 1
          if (won[at] != want[key]) {
            print "FAIL  row " part[1] " `" text[at] "` is @" won[at] ", expected @" want[key]
            bad++
          }
        }
      }
      for (key in want) {
        if (!(key in seen)) {
          print "FAIL  row " key " is in the table but no capture there matches it"
          bad++
        }
      }
      for (at in pinned) {
        if (!(at in won)) {
          print "FAIL  " at " (" pinnedText[at] ") has no capture at all"
          bad++
        } else if (text[at] != pinnedText[at]) {
          print "FAIL  " at " is `" text[at] "`, expected `" pinnedText[at] "`"
          bad++
        } else if (won[at] != pinned[at]) {
          print "FAIL  " at " `" text[at] "` is @" won[at] ", expected @" pinned[at]
          bad++
        }
      }
      for (at in identifier) {
        split(at, part, " ")
        if (at in won) continue
        key = part[1] " " identifier[at]
        if (key in excused) { used[key] = 1; continue }
        print "FAIL  row " part[1] " `" identifier[at] "` gets no capture at all"
        bad++
      }
      for (key in excused) {
        if (!(key in used)) {
          print "FAIL  row " key " is excused from colouring but is coloured, or is gone"
          bad++
        }
      }
      exit bad > 0
    }
  ' "$work/captures" "$work/identifiers" "$work/expected.grammar" "$work/uncoloured"
) || true

if [ -n "$failures" ]; then
  printf '%s\n' "$failures" >&2
  echo "the grammar's colours are not what highlights.scm claims" >&2
  exit 1
fi

grammar_count=$(printf '%s' "$GRAMMAR" | grep -c .)

# ---------------------------------------------------------------------------
# Layer two: the server
# ---------------------------------------------------------------------------

# A copy, so that a session cannot leave anything behind in the repository.
mkdir -p "$work/fixture"
cp -R "$here/fixture/." "$work/fixture/"
copy=$work/fixture/lib/reference/sections.buri

framed() { LC_ALL=C awk '{ printf "Content-Length: %d\r\n\r\n%s", length($0), $0 }'; }

cargo build -q -p buri --manifest-path "$root/Cargo.toml"
printf '%s\n' \
  "{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"initialize\",\"params\":{\"rootUri\":\"file://$work/fixture\"}}" \
  '{"jsonrpc":"2.0","method":"initialized","params":{}}' \
  "{\"jsonrpc\":\"2.0\",\"id\":2,\"method\":\"textDocument/semanticTokens/full\",\"params\":{\"textDocument\":{\"uri\":\"file://$copy\"}}}" \
  "{\"jsonrpc\":\"2.0\",\"id\":3,\"method\":\"textDocument/diagnostic\",\"params\":{\"textDocument\":{\"uri\":\"file://$copy\"}}}" \
  '{"jsonrpc":"2.0","id":4,"method":"shutdown"}' \
  '{"jsonrpc":"2.0","method":"exit"}' \
  | framed \
  | cargo run -q -p buri --manifest-path "$root/Cargo.toml" -- lsp >"$work/session" 2>"$work/session.err" || {
  echo "FAIL  buri lsp did not answer about the fixture" >&2
  cat "$work/session.err" >&2
  exit 1
}

# A fixture the compiler has something to say about is a fixture whose colours
# are the broken-file ones, which would make every assertion below a weaker
# claim than it reads as.
if ! grep -q '^{"id":3,.*"items":\[\]' "$work/session"; then
  echo "FAIL  the fixture no longer compiles clean; the server's answers are about a broken file" >&2
  sed -n 's/^{"id":3,\(.*\)$/\1/p' "$work/session" | cut -c1-400 >&2
  exit 1
fi

data=$(sed -n 's/^{"id":2,.*"data":\[\([0-9,]*\)\].*$/\1/p' "$work/session")
if [ -z "$data" ]; then
  echo "FAIL  the server returned no semantic tokens for the fixture" >&2
  exit 1
fi

# The protocol's relative encoding, undone: five numbers a token, each row and
# character counted from the token before it. The legend is the one
# `cli/src/language_server/semantic_tokens.rs` declares.
printf '%s\n' "$data" | tr ',' '\n' | awk -v src="$copy" '
  BEGIN {
    split("namespace type interface enumMember property function method variable keyword comment string number operator typeParameter", kind, " ")
    rows = 0
    while ((getline line < src) > 0) { text[rows] = line; rows++ }
  }
  {
    field[count % 5] = $1
    count++
    if (count % 5 != 0) next
    if (field[0] > 0) { row += field[0]; character = field[1] } else { character += field[1] }
    print row, character, kind[field[3] + 1], substr(text[row], character + 1, field[2]), field[4]
  }
' >"$work/semantic"

printf '%s\n' "$SERVER" >"$work/expected.server"

failures=$(
  awk '
    BEGIN { bit["declaration"] = 1; bit["definition"] = 2; bit["readonly"] = 4 }

    FILENAME == ARGV[1] {
      got[$1 " " $4] = got[$1 " " $4] " " $3
      mods[$1 " " $4] = mods[$1 " " $4] " " $5
      next
    }
    # The excused list only asks that the server say something; the table below
    # it asks for a particular answer, and is read second so it wins.
    FILENAME == ARGV[2] && NF == 2 { want[$1 " " $2] = "*"; next }
    FILENAME == ARGV[3] && NF == 4 { want[$1 " " $2] = $3; modifier[$1 " " $2] = $4; next }
    FILENAME == ARGV[3] && NF == 3 { want[$1 " " $2] = $3; next }

    END {
      for (key in want) {
        if (!(key in got)) {
          print "FAIL  row " key " has no semantic token"
          bad++
          continue
        }
        if (want[key] == "*") continue
        n = split(got[key], kinds, " ")
        for (i = 1; i <= n; i++) {
          if (kinds[i] != want[key]) {
            print "FAIL  row " key " is " kinds[i] ", expected " want[key]
            bad++
          }
        }
        if (!(key in modifier)) continue
        name = modifier[key]
        wanted = 1
        if (substr(name, 1, 1) == "!") { wanted = 0; name = substr(name, 2) }
        n = split(mods[key], sets, " ")
        for (i = 1; i <= n; i++) {
          carries = int(int(sets[i]) / bit[name]) % 2
          if (carries != wanted) {
            print "FAIL  row " key " " (carries ? "carries" : "is missing") " " name
            bad++
          }
        }
      }
      exit bad > 0
    }
  ' "$work/semantic" "$work/uncoloured" "$work/expected.server"
) || true

if [ -n "$failures" ]; then
  printf '%s\n' "$failures" >&2
  echo "the server's colours are not what semantic_tokens.rs claims" >&2
  exit 1
fi

server_count=$(printf '%s' "$SERVER" | grep -c .)
uncoloured_count=$(printf '%s' "$UNCOLOURED" | grep -c .)

echo "$grammar_count tokens have the capture highlights.scm gives them, \
$uncoloured_count are left to the server on purpose, \
and $server_count have the type the server gives them"
