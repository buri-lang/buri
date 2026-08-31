#!/usr/bin/env bash
#
# **Nothing was skipped that `.github/known-skips.txt` did not sign for.**
#
# The third liveness gate, beside `assert-stencils.sh` (the bytes on disk) and
# `assert-suite-ran.sh` (what the suite printed). Those two catch a suite that
# ran and proved nothing. This one catches a suite that did not run part of
# itself at all — and libtest's exit status cannot tell the difference, because
# an ignored test and a filtered-out test are both a green run.
#
# Two numbers off every `test result:` line in the log, summed over the binaries:
#
#   * **ignored** — `#[ignore]`. Held to the number of rows in
#     `.github/known-skips.txt`, exactly, when nothing was filtered, and to no
#     more than that when something was. **Zero today**: the two rows it shipped
#     with were both compiler slices, both were written, and the file is now
#     prose and no rows. Zero is a number this script handles like any other —
#     it is the `-ne` branch below that makes the first new `#[ignore]` fail.
#   * **filtered out** — a `--skip` or a name filter on the command line. That
#     is a deliberate split of the suite across steps (the linux jobs run the
#     corpus census alone and then everything else), so it is a failure only
#     where the invocation asked for the whole suite. `--allow-filtered` is how
#     a step says it asked for a part.
#
# And the case neither number catches: a log with no `test result:` line in it
# at all, which is what a build failure piped through `tee` looks like from
# here.
#
# The other half of "no skips" is not in this script and cannot be: a runtime
# `if !supported() { return; }` reports as PASSED and appears in no count.
# `cli/tests/harness/ci.rs` is what handles those — `BURI_CI=1` turns every one
# of them into a panic — and this script is what handles the ones libtest knows
# about by name.
#
# Usage: bash .github/scripts/assert-no-skips.sh <log> [--allow-filtered]

set -euo pipefail

log=${1:-}
allow_filtered=${2:-}

if [ -z "$log" ] || [ ! -f "$log" ]; then
  echo "::error::assert-no-skips.sh needs the path to a test log (got '${log}')"
  exit 1
fi

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
known="$here/../known-skips.txt"
if [ ! -f "$known" ]; then
  echo "::error::$known is missing, so the number of signed-for skips is unknown and this assertion would be a guess."
  exit 1
fi

# Rows, not lines: `#` is a comment and blank lines separate the prose above
# from the list.
allowed=$(grep -cE '^[^#[:space:]]' "$known" || true)
allowed=${allowed:-0}

# libtest prints, once per binary:
#   test result: ok. 123 passed; 0 failed; 2 ignored; 0 measured; 7 filtered out; …
# `--nocapture` can put a test's own output on the same line as its name but
# never on this one, so the anchor is safe here in a way it is not in
# `assert-suite-ran.sh`.
results=$(grep -c '^test result:' "$log" || true)
results=${results:-0}
if [ "$results" -eq 0 ]; then
  echo "::error::there is no \`test result:\` line in $log, so no test binary reported a summary. The run did not get as far as running tests — look for a compile error above rather than for a failing assertion."
  exit 1
fi

sum() {
  # The count that precedes a given word on every summary line, added up. `awk`
  # rather than `grep -o | paste -sd+ | bc`, because `bc` is not on every runner
  # and an arithmetic failure here reads as a zero.
  awk -v word="$1" '
    /^test result:/ {
      for (i = 1; i <= NF; i++) {
        if ($i == word || $i == word";") { total += $(i - 1) }
      }
    }
    END { print total + 0 }
  ' "$log"
}

ignored=$(sum "ignored")
filtered=$(sum "filtered")
passed=$(sum "passed")

echo "$results test binaries: $passed passed, $ignored ignored, $filtered filtered out"

status=0

if [ "$passed" -eq 0 ]; then
  echo "::error::every summary in $log reports zero tests passed. A suite that ran nothing is not a suite that passed."
  status=1
fi

if [ "$filtered" -gt 0 ] && [ "$allow_filtered" != "--allow-filtered" ]; then
  echo "::error::$filtered test(s) were filtered out of an invocation that asked for the whole suite. A name filter or a \`--skip\` reached this step's command line; either it is deliberate, in which case pass --allow-filtered here and say in the step why, or it is a typo that quietly removed tests."
  status=1
fi

if [ "$ignored" -gt "$allowed" ]; then
  echo "::error::$ignored test(s) were ignored and .github/known-skips.txt signs for $allowed. A new \`#[ignore]\` has been added. It gets fixed, or \`#[cfg]\`ed out on the host that cannot answer it, or deleted — and if it is genuinely a compiler slice of its own, it gets a row in that file naming the slice. \`cli/tests/ci.rs\` names which one it is."
  status=1
elif [ "$filtered" -eq 0 ] && [ "$ignored" -ne "$allowed" ]; then
  echo "::error::$ignored test(s) were ignored and .github/known-skips.txt signs for $allowed, with nothing filtered out — so a signed-for skip no longer exists. That is good news and a stale file: delete the row whose defect is fixed."
  status=1
else
  echo "ignored: $ignored of the $allowed signed for in .github/known-skips.txt"
fi

# The deferrals, printed rather than counted. `ci::deferred_to` writes one line
# per test that is run by a different job in this same workflow, and
# `cli/tests/ci.rs` is what holds each of those job names to a job that exists.
if grep -q 'deferred to the' "$log"; then
  echo "--- deferred to another job in this workflow ---"
  grep -h 'deferred to the' "$log" | sort -u
fi

exit "$status"
