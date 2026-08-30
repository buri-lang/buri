#!/bin/sh
# The agreement test: the tree-sitter grammar and the compiler's own parser
# say the same thing about every Buri source in the repository.
#
# `grammar.js` is GENERATED from `cli/src/docs/grammar.ebnf`, and a cargo test
# holds the two together byte for byte. That leaves one thing a cargo test
# cannot check, because it needs the tree-sitter CLI and the toolchain may not
# depend on an external tool: whether the generated grammar actually parses
# Buri. That is this script.
#
#   editors/tree-sitter-buri/check.sh
#
# The arbiter is the toolchain itself, asked here and now:
#
#   cargo run -q -p buri --example parse_verdicts < paths
#
# prints `parses` or `rejects` for each path. Nothing is recorded between the
# two — a checked-in file of verdicts would be a readout of what the compiler
# does, and it would go stale exactly when the answer starts to matter.
#
# A program the compiler turns away is turned away at a stage, and only the
# first stage is a claim about syntax: a case in `cli/tests/reject/` that fails
# type checking is well-formed Buri, and a syntax tree of it must be clean. So:
#
#   parses   zero ERROR and zero MISSING nodes.
#   rejects  at least one, unless the file is named in DIVERGENCES below.
#
# The `rejects` direction is the half that catches a grammar which has become
# too permissive, which is the failure a corpus of valid programs cannot see.
#
# Doc-example fences are not here: they are fragments as often as modules, and
# `cargo test -p buri --test docs examples::` compiles every one of them
# against the real compiler, which is a stronger check than a syntax tree.

set -eu

here=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
root=$(CDPATH= cd -- "$here/../.." && pwd)

for tool in tree-sitter cargo; do
  if ! command -v "$tool" >/dev/null 2>&1; then
    echo "$tool is not on PATH" >&2
    exit 2
  fi
done

# Four files where the compiler and a syntax tree are *meant* to disagree.
# Each is a rule the grammar deliberately does not encode, because encoding it
# would mean an editor showing a red squiggle exactly where the compiler has a
# sentence to say instead:
#
#   chained_comparison            comparison is non-associative in the EBNF;
#                                 tree-sitter is given left-associativity so
#                                 that `chained-comparison` is what a reader
#                                 sees rather than a parse failure. That
#                                 non-associativity is also what makes
#                                 `f<A>(x)` readable as a call, so this is the
#                                 one divergence the grammar leans on twice:
#                                 the left-associative reading is what keeps
#                                 `a < b > c` a comparison here, and the
#                                 `@conflicts` group's dynamic precedence is
#                                 what makes `a < b > (c)` a call in both.
#   reserved_word_return_as_function,
#   reserved_word_while_as_binding
#                                 `while` is not a keyword, it is a word the
#                                 lexer refuses. A grammar cannot refuse a word
#                                 without refusing the program the compiler's
#                                 own error message is about.
#   reserved_word_test_as_function
#                                 a keyword where an identifier is expected.
#                                 tree-sitter's keyword extraction reads it as
#                                 an identifier, which is what makes error
#                                 recovery work at all.
#
# This is an authored list with a reason for each line, not a record of what
# happened last time it ran. It may only shrink: a file on it that has started
# to agree is reported below, so it cannot quietly become an excuse for
# something else.
DIVERGENCES="\
cli/tests/reject/chained_comparison/main.buri
cli/tests/reject/reserved_word_return_as_function/main.buri
cli/tests/reject/reserved_word_test_as_function/main.buri
cli/tests/reject/reserved_word_while_as_binding/main.buri"

# The corpus: every file in the repository whose text is Buri rather than
# textproto.
#
#   `expected/` under `tests/repositories` is left out, because a recorded
#   `buri gen` output is a `BUILD.buri` under a name ending in `.buri` and is
#   not Buri at all.
#   `.buri/` is a build output directory that a test run leaves behind, and it
#   is also a directory whose own name ends in `.buri`.
sources() {
  find "$@" \
    -type d -name '.buri' -prune -o \
    -type f \
    -name '*.buri' \
    ! -name '*BUILD.buri' \
    ! -name '*REPO.buri' \
    ! -path '*/expected/*' \
    -print \
    | sort
}

STDLIB_DIR=cli/src/compiler/standard_library/sources
CORPUS_DIRS="cli/src/docs/harness
cli/tests/example
cli/tests/conformance
cli/tests/crash
cli/tests/golden_javascript
cli/tests/reject
cli/tests/repositories"

echo "asking the toolchain what parses..." >&2
# Two questions, because there are two dialects and only one of them is the
# language a person writes: inside a bundled standard library module a `fn` may
# be declared with no body, and everywhere else that parses and is then turned
# away by a rule. Asking the wrong one would report a rejection the compiler
# never makes of these files.
verdicts=$(
  cd "$root"
  cargo build -q -p buri --example parse_verdicts
  sources "$STDLIB_DIR" | cargo run -q -p buri --example parse_verdicts -- --stdlib
  # shellcheck disable=SC2086
  sources $CORPUS_DIRS | cargo run -q -p buri --example parse_verdicts
)

cd "$here"
tree-sitter generate >/dev/null

# Every query has to compile too. A highlight query naming a node the grammar
# no longer has fails silently in an editor — it just stops colouring.
# The queries live once, where Zed needs them. A second copy here would be a
# second thing to keep in step.
for q in ../zed/languages/buri/*.scm; do
  if ! tree-sitter query "$q" "$root/$STDLIB_DIR/option.buri" >/dev/null 2>&1; then
    echo "FAIL  $q does not compile against the grammar" >&2
    tree-sitter query "$q" "$root/$STDLIB_DIR/option.buri" >&2 || true
    exit 1
  fi
done

# A query that compiles can still colour nothing. `check_highlighting.sh` runs
# the query engine and the language server over one fixture and asserts a named
# capture and a named token type for each of its tokens.
"$here/check_highlighting.sh"

failures=0
count=0
diverged=0

while read -r expected file; do
  [ -n "$file" ] || continue
  count=$((count + 1))

  if tree-sitter parse "$root/$file" 2>&1 | grep -q 'ERROR\|MISSING'; then
    tree=rejects
  else
    tree=parses
  fi

  known=no
  case "
$DIVERGENCES
" in *"
$file
"*) known=yes ;;
  esac

  if [ "$known" = yes ]; then
    # A divergence is a file the compiler rejects and the grammar accepts.
    # Anything else about it, including agreement, is news.
    diverged=$((diverged + 1))
    if [ "$expected" != rejects ] || [ "$tree" != parses ]; then
      failures=$((failures + 1))
      echo "FAIL  $file is listed as a divergence but is not one" >&2
      echo "      the parser says $expected and the syntax tree says $tree" >&2
    fi
    continue
  fi

  if [ "$tree" != "$expected" ]; then
    failures=$((failures + 1))
    if [ "$expected" = parses ]; then
      echo "FAIL  $file parses, but its syntax tree has an error" >&2
      tree-sitter parse "$root/$file" 2>&1 | grep -m 1 'ERROR\|MISSING' >&2
    else
      echo "FAIL  $file does not parse, but its syntax tree is clean" >&2
      echo "      the grammar accepts something the compiler rejects" >&2
    fi
  fi
done <<EOF
$verdicts
EOF

# A corpus that discovers nothing passes every assertion, which is the same
# floor the Rust harness insists on (`cli/tests/harness/mod.rs`).
if [ "$count" -lt 400 ]; then
  echo "only $count files were asked about; the corpus is not where this script thinks" >&2
  exit 1
fi

if [ "$failures" -ne 0 ]; then
  echo "$failures of $count files disagree" >&2
  exit 1
fi

echo "$count files agree with the compiler ($diverged known divergences), and every query compiles"
