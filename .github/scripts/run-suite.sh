#!/usr/bin/env bash
#
# **The whole suite, with its binaries overlapped instead of queued.**
#
# `cargo test -p buri` runs the test binaries **one at a time**. That is not a
# tuning knob — it is what the command does — and it is the single largest thing
# between this repository and its five-minute budget on a four-core runner.
# Measured on a ten-core M-series mac against `9f5584a0`, warm:
#
#   the fifteen units, one after another (what `cargo test` does)   169.2 s
#   the same fifteen, started together                               73.0 s
#
# Not because any test got faster. Because the suites are **latency**-bound
# rather than core-bound: `native` spends 61 s of wall clock on 95 s of CPU,
# waiting on `cc`, on a linker and on a child process, and while it waits the
# other fourteen binaries are not allowed to start. Their sum is 372 CPU-seconds,
# so four cores can retire the whole thing in 93 seconds and the scheduler is
# what decides whether they do.
#
# ## Why this and not `nextest`
#
# `nextest` is the tool that solves exactly this, and this repository does not
# use it, for a reason recorded in three places and which is not a preference:
# two of the three liveness gates read **libtest's own output**.
# `assert-no-skips.sh` sums the `ignored` and `filtered out` counts off every
# `test result:` line, and `assert-suite-ran.sh` reads a census out of a
# `--nocapture` log. nextest prints neither shape, so adopting it would disarm
# the machinery this repository built to stop a suite passing vacuously, while
# everything stayed green.
# `cli/tests/ci.rs::no_runner_config_promises_a_cap_nothing_reads` is the test
# that holds that line, and this script is written to stay on the right side of
# it: **every unit here is a libtest binary, run directly**, so the log this
# writes is the same text `cargo test` wrote, with the same summary lines. The
# gates keep working unchanged, and `assert-no-skips.sh` still reads this log.
#
# ## The set is derived, never listed
#
# The one thing a runner like this must not do is quietly stop running a
# binary. So the set is not written down here; it is asked of cargo —
#
#   cargo test -p buri --no-run --message-format=json-render-diagnostics
#
# — and every artifact cargo reports with `"test":true` is a unit. That is by
# construction the set `cargo test -p buri` would have run, so a `.rs` file
# added under `cli/tests/` tomorrow is picked up on the day it is added with
# nothing here to update. `cli/tests/ci.rs::the_suite_is_asked_for_as_a_whole`
# is what stops a future edit replacing the derivation with a list.
#
# The doctests are the one unit cargo cannot hand over as an executable, so they
# run as their own `cargo test --doc` and their summary joins the rest. There
# are zero of them today; the point is that a doctest written tomorrow is not
# silently dropped.
#
# ## The width, and why it is not "all of them"
#
# Every unit at once is what the 73-second number was measured with, and it is
# the wrong default on a runner: fifteen libtest binaries each starting `nproc`
# threads, each of which spawns a real `buri`, is sixty compilers on a four-core
# box. The wall clock would not improve — four cores are four cores — and the
# memory might not be there.
#
# So the width is a budget rather than a count: **`jobs` binaries at a time,
# each with `--test-threads=threads`, chosen so the two multiply to about twice
# the core count.** Twice, not once, because what is being hidden is a stall on
# `cc` and on `exec`, and a core with one runnable thread on it idles through
# them. Both halves are overridable — `BURI_SUITE_JOBS` and
# `BURI_SUITE_TEST_THREADS` — for a machine the arithmetic does not suit.
#
# The pool is `xargs -P`, which hands the next unit to whichever worker is free.
# A static split into lanes was tried first and is worse for the same reason
# `cargo test` is: one long unit at the back of a lane holds the run open.
# Long units are therefore started first — the order is a **hint** and nothing
# more, and a unit this script has never heard of sorts to the front, so a new
# suite is never the one left holding the tail. Being wrong about the order
# costs seconds, never coverage.
#
# ## Usage
#
#   bash .github/scripts/run-suite.sh <log> [extra cargo args...]
#
# `<log>` is written with every unit's output concatenated in a stable order,
# for `assert-no-skips.sh` to read. Extra arguments go to both cargo
# invocations, which is how the `release` job asks for `--features backend-llvm`.
# The exit status is non-zero if any unit failed, and every unit runs whatever
# happens: this is `--no-fail-fast` by construction, because a domain that fails
# must not hide the other twelve.

set -uo pipefail

log=${1:-}
if [ -z "$log" ]; then
  echo "::error::run-suite.sh needs the path of the log to write (got nothing)"
  exit 1
fi
shift

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$root" || exit 1

# The unit list is passed to the pool as whitespace-separated fields, so a
# checkout under a path with a space in it would split one executable into two
# arguments and run neither. Said here, once, rather than discovered as a suite
# that reported nothing.
case "$root" in
  *[[:space:]]*)
    echo "::error::this checkout is at '$root', and run-suite.sh cannot pass a path with whitespace in it to its worker pool. Move the checkout, or run \`cargo test -p buri\` and pay the serial run."
    exit 1
    ;;
esac

# `nproc` on Linux, `sysctl` on macOS, four if a future runner has neither — a
# wrong core count here is a slower run and never a wrong answer.
cores=$( { nproc || sysctl -n hw.logicalcpu || echo 4 ; } 2>/dev/null | head -1)
case "$cores" in ''|*[!0-9]*) cores=4 ;; esac
[ "$cores" -lt 1 ] && cores=1

# `jobs * threads ~= 2 * cores`, with each half at least two: one binary at a
# time is what this script exists to stop doing, and one test thread inside it
# would serialize the suites that are genuinely parallel.
jobs=${BURI_SUITE_JOBS:-$cores}
[ "$jobs" -lt 2 ] && jobs=2
threads=${BURI_SUITE_TEST_THREADS:-$(( cores / 2 ))}
[ "$threads" -lt 2 ] && threads=2

echo "run-suite: $cores core(s); $jobs binaries at a time, $threads test thread(s) each"

logs="$root/target/suite-logs"
rm -rf "$logs"
mkdir -p "$logs"

# ---------------------------------------------------------------------- units
#
# Asked of cargo rather than listed. `--no-run` builds whatever is not built and
# reports every artifact; `json-render-diagnostics` keeps the artifact records
# machine-readable on stdout while a compile error is still rendered as text on
# stderr, so a broken build reads here like a broken build.
artifacts="$logs/artifacts.json"
if ! cargo test -p buri --no-run --message-format=json-render-diagnostics "$@" > "$artifacts"; then
  echo "::error::the test binaries did not build, so no suite ran"
  exit 1
fi

# **`profile.test`, not `target.test`**, and the difference is a whole extra
# executable: cargo marks a `[[bin]]` as testable in the *target* object whether
# or not the artifact it is reporting was built as a test harness, so the plain
# `buri` binary carries `"test":true` too. Running it is running the CLI with
# `--test-threads` as its subcommand, which fails and says so. The **profile**'s
# `test` flag is the one that means "this artifact is a libtest harness".
#
# The name is taken from the same record rather than from the file name, because
# two of the units are called `buri` — the library's unit tests and the
# binary's — and two logs called `buri.log` is one log with a race in it.
awk '
  /"reason":"compiler-artifact"/ {
    if ($0 !~ /"executable":"/) next
    prof = $0; sub(/.*"profile":\{/, "", prof); sub(/\}.*/, "", prof)
    if (prof !~ /"test":true/) next
    exe = $0; sub(/.*"executable":"/, "", exe); sub(/".*/, "", exe)
    tgt = $0; sub(/.*"target":\{/, "", tgt); sub(/\},"profile".*/, "", tgt)
    kind = tgt; sub(/.*"kind":\["/, "", kind); sub(/".*/, "", kind)
    nm = tgt; sub(/.*"name":"/, "", nm); sub(/".*/, "", nm)
    print (kind == "test" ? nm : kind "-" nm), exe
  }
' "$artifacts" | sort -u > "$logs/executables"

if [ ! -s "$logs/executables" ]; then
  echo "::error::cargo reported no test executables. A suite with nothing in it is not a suite that passed."
  exit 1
fi

# The order hint. Longest first, measured warm on a ten-core mac; a name that is
# not here sorts *before* everything, because an unknown unit might be the long
# one and starting it first is the cheap mistake.
hint() {
  case "$1" in
    native)      echo 20 ;;
    fuzz)        echo 21 ;;
    build)       echo 22 ;;
    recovery)    echo 23 ;;
    adversarial) echo 24 ;;
    language)    echo 25 ;;
    failing)     echo 26 ;;
    docs)        echo 27 ;;
    linting)     echo 28 ;;
    checking)    echo 29 ;;
    vectors)     echo 30 ;;
    lib-buri)    echo 31 ;;
    formatting)  echo 32 ;;
    ci)          echo 33 ;;
    bin-buri)    echo 34 ;;
    *)           echo 10 ;;
  esac
}

# One name per unit, asserted rather than assumed. A unit's name is its log's
# name, so two units sharing one would share one file, and the loser's summary
# line — the whole of what `assert-no-skips.sh` reads — would be gone with no
# failure anywhere. Cargo's records do not collide today; this is what says so
# on the day they do.
units=$(wc -l < "$logs/executables" | tr -d ' ')
distinct=$(awk '{ print $1 }' "$logs/executables" | sort -u | wc -l | tr -d ' ')
if [ "$units" -ne "$distinct" ]; then
  echo "::error::$units test executables carry only $distinct distinct names, so two of them would write one log and one summary would be lost."
  awk '{ print $1 }' "$logs/executables" | sort | uniq -d
  exit 1
fi

: > "$logs/ordered"
while read -r name exe; do
  echo "$(hint "$name") $name $exe" >> "$logs/ordered"
done < "$logs/executables"
sort -n -k1,1 -k2,2 -o "$logs/ordered" "$logs/ordered"

# ------------------------------------------------------------------- the run
started=$(date +%s)

# The doctests first and in the background: one `cargo` invocation, a second of
# wall clock, and the only unit that is not an executable.
( cargo test -p buri --doc "$@" > "$logs/zz-doctests.log" 2>&1 \
  || echo zz-doctests >> "$logs/failed" ) &
doctests=$!

# `xargs -n 2` hands each worker one pair, so `$0` is the unit's name and `$1`
# is its executable; `-P` is the width. A worker that fails records its name and
# exits non-zero, which is also what makes `xargs` itself exit non-zero.
export BURI_SUITE_LOGS="$logs"
export BURI_SUITE_T="$threads"
awk '{ print $2, $3 }' "$logs/ordered" | tr ' ' '\n' | xargs -n 2 -P "$jobs" bash -c '
  set -u
  if "$1" --test-threads="$BURI_SUITE_T" > "$BURI_SUITE_LOGS/$0.log" 2>&1; then
    echo "  ok   $0"
  else
    echo "  FAIL $0"
    echo "$0" >> "$BURI_SUITE_LOGS/failed"
    exit 1
  fi
'
# `xargs` exits 123 when a worker did, which the file below already records —
# and anything else is `xargs` itself failing, which records nothing and would
# otherwise be a green run that ran no tests.
pool=$?
if [ "$pool" -ne 0 ] && [ "$pool" -ne 123 ]; then
  echo "::error::the worker pool exited $pool without any unit reporting. No suite ran."
  exit 1
fi
wait "$doctests"

# ------------------------------------------------------------------- the log
# A stable order, so a diff between two runs is a diff about the tests. Every
# unit's whole output, because that is what `cargo test | tee` used to write and
# what `assert-no-skips.sh` reads.
: > "$log"
for unit in "$logs"/*.log; do
  name=$(basename "$unit" .log)
  {
    echo
    echo "--- $name ---"
    cat "$unit"
  } >> "$log"
done
cat "$log"

echo "run-suite: $(wc -l < "$logs/ordered" | tr -d ' ') binaries and the doctests, in $(( $(date +%s) - started ))s"

if [ -s "$logs/failed" ]; then
  echo "::error::$(tr '\n' ' ' < "$logs/failed")failed"
  exit 1
fi
exit 0
