#!/bin/sh
# Builds target/<target>/release/buri with profile-guided optimization: an
# instrumented build runs half the e2e suites, and the release build reads the
# profile. Run from the repository root: `sh .github/pgo.sh <target> [step]`.
#
#   all         (the default) instruments, trains on every test, then builds
#               with the profile: the Linux legs.
#   instrument  stops at target/pgo/buri-instrumented: the first macOS job.
#   train K/N   runs shard K of N of the training tests on a
#               target/pgo/buri-instrumented already there, and stops at
#               target/pgo/buri.profdata: the three macOS training jobs.
#   use P...    merges the profiles P, the shards', into
#               target/pgo/buri.profdata and builds with it: the macOS build.
#
# Shard K of N is every Nth test of each suite, in the order libtest lists them,
# starting at the Kth. So the N shards run each test once between them, and
# `llvm-profdata merge` sums their counts into what one run of all would count.
#
# The other half (language docs failing fuzz linting adversarial ci) is held out,
# so a profile tuned to its own training shows up as a smaller win there.
set -eu
target=$1
step=${2:-all}
work=$PWD/target/pgo
shard=1/1
case $step in
  all|instrument) rm -rf "$work" ;;
  train)
    shard=${3:-}
    case $shard in
      [1-9]/[1-9]) [ "${shard%/*}" -le "${shard#*/}" ] ;;
      *) false ;;
    esac || { echo "train wants a shard K/N with 1 <= K <= N <= 9, not '$shard'" >&2; exit 2; }
    test -s "$work/buri-instrumented" || { echo "no instrumented build at $work/buri-instrumented" >&2; exit 1; }
    ;;
  use)
    shift 2
    [ $# -gt 0 ] || { echo "use wants the profiles to merge" >&2; exit 2; }
    for p in "$@"; do test -s "$p" || { echo "no profile at $p" >&2; exit 1; }; done
    ;;
  *) echo "unknown step $step: want all, instrument, train or use" >&2; exit 2 ;;
esac
host=$(rustc -vV | sed -n 's/^host: //p')
profdata="$(rustc --print sysroot)/lib/rustlib/$host/bin/llvm-profdata"
# A plain release build drops LLVM's interpreter and JIT, but the instrumented
# build and the suites' debug build keep them, and on a musl target they need
# two things it lacks: `__clear_cache`, which only libgcc defines, and libffi,
# which inkwell asks for as a shared library. A shared libffi makes the binary
# dynamic, with a loader musl doesn't have, so `$work/static` offers only the
# archive and is searched first. Neither resolves anything else.
link=
case $target in *-linux-musl)
  mkdir -p "$work/static"
  ln -sf "$(cc -print-file-name=libffi.a)" "$work/static/libffi.a"
  link="-Clink-arg=-lgcc -Lnative=$work/static" ;;
esac
# cc-rs hands `-Cprofile-generate` and `-Cprofile-use` on to the C it compiles,
# and a clang of another LLVM writes and reads profiles rustc's tools don't, so
# the C stays out of the profile. cc-rs reads `CFLAGS_<target>` last, and
# llvm-sys overwrites plain `CFLAGS`, so that is where both are turned off.
cflags=CFLAGS_$(echo "$target" | tr - _)

if [ "$step" = all ] || [ "$step" = instrument ]; then
  # `test-hooks` lets the suites drive the instrumented build as they drive
  # their own. Enough value counters keep the profiler from warning on stderr.
  env "$cflags=-fno-profile-generate" \
  RUSTFLAGS="-Cprofile-generate=$work/raw -Cllvm-args=-vp-counters-per-site=8 $link" \
    cargo build --release --features backend-llvm,test-hooks -p buri \
    --target "$target" --target-dir "$work/instrumented"
  # Only the binary is kept, so its build's intermediate files leave the disk
  # before the suites build theirs.
  mv "$work/instrumented/$target/release/buri" "$work/buri-instrumented"
  rm -rf "$work/instrumented"
fi

if [ "$step" = all ] || [ "$step" = train ]; then
  rm -rf "$work/suites" "$work/raw"

  # Only the six training suites, and without debug information: nothing here
  # reads a backtrace, and the binaries are deleted after training.
  set --
  for unit in build native checking recovery formatting vectors; do
    set -- "$@" --test "$unit"
  done
  CARGO_PROFILE_TEST_DEBUG=0 RUSTFLAGS="$link" CARGO_TERM_COLOR=never \
    cargo test -p buri --features backend-llvm --no-run --target-dir "$work/suites" "$@" \
    2>"$work/suites.log" || { cat "$work/suites.log"; exit 1; }
  # A copy, with the execute bit an artifact download drops.
  cp "$work/buri-instrumented" "$work/suites/debug/buri"
  chmod +x "$work/suites/debug/buri"
  : > "$work/units.txt"
  for unit in build native checking recovery formatting vectors; do
    sed -n "s|^ *Executable .* (\(.*/$unit-[0-9a-f]*\))\$|\1|p" "$work/suites.log" >> "$work/units.txt"
    grep -q "/$unit-[0-9a-f]*\$" "$work/units.txt" || { echo "no $unit suite to train on" >&2; exit 1; }
  done
  # Only the profile matters here, so a failing test doesn't stop the build. The
  # suites share one run name, so their cases share one pool as wide as the
  # machine; each suite apart opens a pool that wide, and six at once starved the
  # 3-core macOS runner until it stopped answering.
  #
  # A test that fails often stops early, for a tool the machine lacks, and the
  # profile then covers less. So each suite's result line is kept, and one line
  # sums them; a suite that ends without one counts as a crash.
  #
  # Each finished test's line goes to the log as it happens, named by suite, and
  # so does libtest's "has been running for over 60 seconds", so a stall names
  # the test that stalled. Runs 44670440615 and 44693285711 lost the runner
  # mid-training and the log said nothing about which test was running.
  #
  # Every run of buri merges its counts into one pooled file, and a run whose
  # profile is laid out otherwise destroys that file for good: a copy of buri
  # without a build id, which a test makes, did so in run 43611982203. Such a
  # test gives its copy files of its own beside `LLVM_PROFILE_FILE`, so the
  # pool is named here, by the name the build already gave it.
  #
  # A shard names its tests to libtest with `--exact`. One with no test in a
  # suite skips it, since no names at all would run every test.
  cores=$(getconf _NPROCESSORS_ONLN)
  : > "$work/results.txt"
  SHARD=$shard LLVM_PROFILE_FILE="$work/raw/default_%m.profraw" NEXTEST_RUN_ID=pgo-$$ xargs -P "$cores" -I{} sh -c '
    set -f
    suite=$(basename "$1" | sed "s/-[0-9a-f]*\$//")
    set -- "$1"
    if [ "$SHARD" != 1/1 ]; then
      list=$("$1" --list --format terse) || { echo "test result: crashed"; exit; }
      names=$(echo "$list" | sed -n "s/: test\$//p" \
        | awk -v k="${SHARD%/*}" -v n="${SHARD#*/}" "(NR - 1) % n == k - 1")
      [ -n "$names" ] || { echo "test result: ok. 0 passed; 0 failed;"; exit; }
      set -- "$1" --exact $names
    fi
    "$@" --test-threads=2 2>/dev/null | awk -v suite="$suite" "
      /^test result:/ { print; done = 1; next }
      /^test / { print suite \": \" \$0 > \"/dev/stderr\"; fflush(\"/dev/stderr\") }
      END { if (!done) print \"test result: crashed\" }"' sh {} \
    < "$work/units.txt" >> "$work/results.txt"
  awk '/crashed/ { crashed++ }
    { for (i = 1; i < NF; i++) if ($(i+1) ~ /^(passed|failed);$/) n[$(i+1)] += $i }
    END { printf "training: %d passed, %d failed, %d suites without a result\n", n["passed;"], n["failed;"], crashed }' \
    "$work/results.txt"

  "$profdata" merge -o "$work/buri.profdata" "$work/raw"
  rm -rf "$work/suites" "$work/raw"
fi

if [ "$step" = use ]; then
  mkdir -p "$work"
  "$profdata" merge -o "$work/buri.profdata" "$@"
fi

if [ "$step" = all ] || [ "$step" = use ]; then
  test -s "$work/buri.profdata" || { echo "no profile at $work/buri.profdata" >&2; exit 1; }
  env "$cflags=-fno-profile-use" RUSTFLAGS="-Cprofile-use=$work/buri.profdata $link" \
    cargo build --release --features backend-llvm -p buri --target "$target"
fi
