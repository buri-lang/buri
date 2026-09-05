#!/bin/sh
# The agreement test for the build-file grammar: it parses every BUILD.buri and
# REPO.buri in the repository, and it refuses what the reader refuses.
#
#   editors/tree-sitter-buri-build/check.sh
#
# The sibling script next door asks the toolchain, file by file, what parses.
# This one does not have to: a build file in this repository is a build file the
# toolchain already reads — the tests would not run otherwise — so the corpus is
# its own list of accepted inputs, and the check is that not one of them has an
# ERROR or MISSING node.
#
# The other direction, the one a corpus of working files cannot see, is the
# REFUSED list below: a handful of inputs `cli/src/build/textproto.rs` turns
# away, which the syntax tree must turn away too. It is what says the grammar
# has not quietly become more permissive than the reader.

set -eu

here=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
root=$(CDPATH= cd -- "$here/../.." && pwd)

if ! command -v tree-sitter >/dev/null 2>&1; then
  echo "tree-sitter is not on PATH" >&2
  exit 2
fi

# The corpus: every build file in the repository.
#
#   `target/` holds build output and the fuzzer's corpora, which are the one
#   place a deliberately malformed build file lives.
#   `editors/zed/grammars/` is Zed's own shallow clone of this repository,
#   rebuilt from the pin in `../zed/extension.toml` — the same files twice.
#   `.buri/` is a build output directory a test run leaves behind.
#
# `expected/` is *not* left out here, unlike next door: a recorded `buri gen`
# output is a build file, which is exactly what this grammar is for.
build_files() {
  find "$root" \
    -type d \( -name target -o -name '.buri' -o -path "$root/editors/zed/grammars" \) -prune -o \
    -type f \( -name 'BUILD.buri' -o -name 'REPO.buri' \) \
    -print \
    | sort
}

# One file in the corpus is a harness template rather than a build file: the
# platform in it is filled in at run time, because it has to be a platform that
# is not the host's. `{{` is the placeholder marker — see cli/tests/harness/case.rs.
templated() {
  grep -q '{{' "$1"
}

# Inputs `textproto.rs` refuses, and a syntax tree must refuse too. Taken from
# that file's own list, minus the ones no grammar can see: an integer too large
# for an `i64` and a message nested past the reader's depth bound are both
# well-formed shapes, and the reader's sentence about them is the right one.
#
# The last line is the reason this language exists at all: `//` is Buri's
# comment marker and the reader has never accepted it, so an editor that offered
# it here would be writing a file `buri format` then refuses.
REFUSED='a:
a {
a: [1, 2
a: "unclosed
{
// not a comment'

scratch=$(mktemp -d)
trap 'rm -rf "$scratch"' EXIT INT TERM

cd "$here"
tree-sitter generate >/dev/null

# Every query has to compile too. A highlight query naming a node the grammar
# no longer has fails silently in an editor — it just stops colouring.
# The queries live once, where Zed needs them.
sample="$root/cli/tests/example/REPO.buri"
for q in ../zed/languages/buri-build/*.scm; do
  if ! tree-sitter query "$q" "$sample" >/dev/null 2>&1; then
    echo "FAIL  $q does not compile against the grammar" >&2
    tree-sitter query "$q" "$sample" >&2 || true
    exit 1
  fi
done

# ...against the grammar in this directory, where Zed compiles them against the
# grammar at the commit `../zed/extension.toml` pins. When the two drift apart
# the queries stop compiling in the editor, and a language whose queries do not
# compile is one Zed does not load: the file opens with no language rather than
# with no colour. The pinned tree is fetched from GitHub and cannot be read from
# here, so what is checked is the digest recorded beside the pin. `src/parser.c`
# is left out of it because it is generated above.
digest() {
  if command -v sha256sum >/dev/null 2>&1; then sha256sum; else shasum -a 256; fi \
    | cut -d' ' -f1
}

manifest=../zed/extension.toml
section='/^\[grammars\.buri_build\]$/,/^\[/'
recorded=$(sed -n "$section"'{ s/^# grammar-sha256 = "\([0-9a-f]*\)"$/\1/p; }' "$manifest")
actual=$(digest < grammar.js)

if [ -z "$recorded" ]; then
  echo "FAIL  $manifest has no \`# grammar-sha256\` under [grammars.buri_build]" >&2
  echo "      that line is what holds the pin to the grammar; it may not be dropped" >&2
  exit 1
fi

if [ "$recorded" != "$actual" ]; then
  echo "FAIL  the grammar has changed and $manifest still pins the old one" >&2
  echo "      recorded $recorded" >&2
  echo "      grammar  $actual" >&2
  echo "      Push this grammar, then set [grammars.buri_build] \`commit\` to the" >&2
  echo "      commit that holds it and \`# grammar-sha256\` to the digest above." >&2
  exit 1
fi

failures=0
count=0
skipped=0

build_files > "$scratch/corpus"
while IFS= read -r file; do
  [ -n "$file" ] || continue
  if templated "$file"; then
    skipped=$((skipped + 1))
    continue
  fi
  count=$((count + 1))
  if tree-sitter parse "$file" 2>&1 | grep -q 'ERROR\|MISSING'; then
    failures=$((failures + 1))
    echo "FAIL  ${file#"$root"/} is a build file, but its syntax tree has an error" >&2
    tree-sitter parse "$file" 2>&1 | grep -m 1 'ERROR\|MISSING' >&2
  fi
done < "$scratch/corpus"

# The refused half. Each is written to a name the grammar answers to, because
# that is how the tree-sitter CLI picks a parser.
while IFS= read -r src; do
  [ -n "$src" ] || continue
  printf '%s\n' "$src" > "$scratch/BUILD.buri"
  if ! tree-sitter parse "$scratch/BUILD.buri" 2>&1 | grep -q 'ERROR\|MISSING'; then
    failures=$((failures + 1))
    echo "FAIL  the reader refuses \`$src\`, and the syntax tree of it is clean" >&2
    echo "      the grammar accepts something the reader rejects" >&2
  fi
done <<EOF
$REFUSED
EOF

# A corpus that discovers nothing passes every assertion.
if [ "$count" -lt 200 ]; then
  echo "only $count build files were found; the corpus is not where this script thinks" >&2
  exit 1
fi

if [ "$failures" -ne 0 ]; then
  echo "$failures failures" >&2
  exit 1
fi

echo "$count build files parse cleanly (harness templates skipped: $skipped), the reader's refusals are refused, and every query compiles"
