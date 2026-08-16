#!/bin/sh
# The divergence test: every `.buri` file in the repository parses with no
# ERROR and no MISSING node.
#
# This is what keeps the tree-sitter grammar honest against `grammar.ebnf`.
# There is no generator between the two — the hardest parts of the grammar (the
# expression cascade, templates) need shapes a transliteration cannot produce,
# so the grammar is written by hand and *this* is the guarantee, not the
# authorship.
#
# It cannot live in `cargo test`: it needs the tree-sitter CLI, which is an
# external tool and not something the toolchain is allowed to depend on. Run it
# after changing `grammar.js`, `src/scanner.c`, or the language's syntax.
#
#   editors/tree-sitter-buri/check.sh
#
# A file that the *compiler* also rejects is not a failure here — tree-sitter
# is a syntax tree, not a type checker — so the corpus is deliberately the
# files that do compile: the standard library, the example repository, and the
# conformance suite. `cli/tests/reject/` is excluded, because a case there is
# meant to be wrong.

set -eu

here=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
root=$(CDPATH= cd -- "$here/../.." && pwd)

if ! command -v tree-sitter >/dev/null 2>&1; then
  echo "tree-sitter is not on PATH; see https://tree-sitter.github.io" >&2
  exit 2
fi

cd "$here"
tree-sitter generate >/dev/null

# Every query has to compile too. A highlight query naming a node the grammar
# no longer has fails silently in an editor — it just stops colouring.
# The queries live once, where Zed needs them. A second copy here would be a
# second thing to keep in step, and the failure mode of a stale highlight query
# is silent — an editor just stops colouring.
for q in ../zed/languages/buri/*.scm; do
  if ! tree-sitter query "$q" "$root/cli/src/std/option.buri" >/dev/null 2>&1; then
    echo "FAIL  $q does not compile against the grammar" >&2
    tree-sitter query "$q" "$root/cli/src/std/option.buri" >&2 || true
    exit 1
  fi
done

failures=0
count=0
for f in $(find \
    "$root/cli/src/std" \
    "$root/cli/src/docs/harness" \
    "$root/cli/tests/example" \
    "$root/cli/tests/conformance" \
    -name '*.buri' ! -name 'BUILD.buri' ! -name 'REPO.buri' | sort); do
  count=$((count + 1))
  if tree-sitter parse "$f" 2>&1 | grep -q 'ERROR\|MISSING'; then
    failures=$((failures + 1))
    echo "FAIL  ${f#"$root"/}" >&2
    tree-sitter parse "$f" 2>&1 | grep -m 1 'ERROR\|MISSING' >&2
  fi
done

# A corpus that discovers nothing passes every assertion, which is the same
# floor the Rust harness insists on (`cli/tests/harness/mod.rs`).
if [ "$count" -lt 50 ]; then
  echo "only $count files found; the corpus is not where this script thinks" >&2
  exit 1
fi

if [ "$failures" -ne 0 ]; then
  echo "$failures of $count files did not parse" >&2
  exit 1
fi

echo "$count files parse, and every query compiles"
