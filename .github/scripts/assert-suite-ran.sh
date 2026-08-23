#!/usr/bin/env bash
#
# The native suite RAN, and the conformance corpus is at macOS parity.
#
# The second half of the liveness gate `assert-stencils.sh` opens. That one
# looks at the bytes on disk; this one looks at what the suite printed, which is
# the only thing that can tell a real run from a vacuous one *after* the fact:
# every test in `cli/tests/native/stencil.rs` opens with
# `if !supported() { return; }`, so a runner where either the runtime archive or
# the stencil library is missing runs the file, reports every test as PASSED,
# and has proved nothing. The corpus census's own `println!` sits on the far
# side of that guard.
#
# So: the line has to be there, and it has to say what the macOS column says —
# the same twenty-five compiled and the same six refused, by name.
#
# Usage: bash .github/scripts/assert-suite-ran.sh <log from `cargo test … -- --nocapture`>

set -euo pipefail

log=${1:-}
if [ -z "$log" ] || [ ! -f "$log" ]; then
  echo "::error::assert-suite-ran.sh needs the path to a test log (got '${log}')"
  exit 1
fi

status=0
fail() { echo "::error::$*"; status=1; }

# Nothing below is anchored with `^`, and that is a correction rather than
# laziness. libtest prints `test <name> ... ` WITHOUT a newline and only
# terminates the line with `ok` once the test returns, so the first `println!`
# a test makes lands on the same line as its own name:
#
#   test stencil::the_corpus_census_is_a_ratchet ... stencil refuses json/decoding.buri: …
#
# An anchored pattern silently drops exactly one refusal, which is the kind of
# off-by-one that makes a parity check pass while the parity is wrong. Found by
# running this script against a real log before it ever reached a runner.
# `grep -o` takes the match wherever it starts.

# ------------------------------------------------------------- liveness ----
census=$(grep -oE 'stencil compiles [0-9]+ of [0-9]+ conformance files \([0-9]+ refused, [0-9]+ not asked\)' "$log" | head -1 || true)
if [ -z "$census" ]; then
  fail "the corpus census never printed, so the suite SKIPPED. Either the runtime archive or the stencil library is missing on this host and every test returned early. Nothing below was checked."
  exit 1
fi
echo "$census"

# --------------------------------------------------------------- parity ----
case "$census" in
  *"(6 refused, 0 not asked)"*)
    ;;
  *)
    fail "the corpus census does not match the macOS column, which is 6 refused and 0 not asked"
    ;;
esac

# The six, by name. `the_corpus_census_is_a_ratchet` already pins the twenty-five
# that compile from inside the suite; this pins the complement from outside it,
# because "the same six packages" is the claim parity actually makes and a
# census that refused six DIFFERENT files would satisfy the count.
expected=$(cat <<'EOF'
json/decoding.buri
json/encoding.buri
numbers/conversions.buri
numbers/floats.buri
proto/json.buri
text/json.buri
EOF
)
actual=$(grep -oE 'stencil refuses [^:]+:' "$log" | sed -E 's/^stencil refuses //; s/:$//' | sort -u)

if [ "$actual" != "$expected" ]; then
  fail "the refused set is not the macOS one"
  echo "--- expected ---"; printf '%s\n' "$expected"
  echo "--- actual   ---"; printf '%s\n' "$actual"
fi

# The reasons themselves are compiler prose and are not pinned here — a reworded
# diagnostic is a change to the product and belongs in a diff, not in a CI grep.
# They are printed so that a run's evidence carries them.
echo "--- why each is refused ---"
grep -E 'stencil refuses ' "$log" | sort -u || true

exit "$status"
