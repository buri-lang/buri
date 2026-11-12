#!/bin/sh
# Builds target/<target>/release/buri with profile-guided optimization: an
# instrumented build runs half the e2e suites, and the release build reads the
# profile. Run from the repository root: `sh .github/pgo.sh <target> [step]`.
#
#   all    (the default) trains, then builds with the profile: the Linux legs.
#   train  stops at target/pgo/buri.profdata: the macOS training job.
#   use    builds with a target/pgo/buri.profdata already there: the macOS
#          build job, on a fresh runner, with the training job's profile.
#
# The other half (language docs failing fuzz linting adversarial ci) is held out,
# so a profile tuned to its own training shows up as a smaller win there.
set -eu
target=$1
step=${2:-all}
work=$PWD/target/pgo
case $step in
  all|train) rm -rf "$work" ;;
  use) ;;
  *) echo "unknown step $step: want all, train or use" >&2; exit 2 ;;
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

if [ "$step" != use ]; then
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

  # Only the six training suites, and without debug information: nothing here
  # reads a backtrace, and the binaries are deleted after training.
  set --
  for unit in build native checking recovery formatting vectors; do
    set -- "$@" --test "$unit"
  done
  CARGO_PROFILE_TEST_DEBUG=0 RUSTFLAGS="$link" CARGO_TERM_COLOR=never \
    cargo test -p buri --features backend-llvm --no-run --target-dir "$work/suites" "$@" \
    2>"$work/suites.log" || { cat "$work/suites.log"; exit 1; }
  mv "$work/buri-instrumented" "$work/suites/debug/buri"
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
  # profile then covers less. So each suite's output is dropped but its result
  # line is kept, and one line sums them; a suite that ends without one counts
  # as a crash.
  cores=$(getconf _NPROCESSORS_ONLN)
  : > "$work/results.txt"
  NEXTEST_RUN_ID=pgo-$$ xargs -P "$cores" -I{} sh -c \
    '{} --test-threads=2 2>/dev/null | grep "^test result:" || echo "test result: crashed"' \
    < "$work/units.txt" >> "$work/results.txt"
  awk '/crashed/ { crashed++ }
    { for (i = 1; i < NF; i++) if ($(i+1) ~ /^(passed|failed);$/) n[$(i+1)] += $i }
    END { printf "training: %d passed, %d failed, %d suites without a result\n", n["passed;"], n["failed;"], crashed }' \
    "$work/results.txt"

  "$profdata" merge -o "$work/buri.profdata" "$work/raw"
  rm -rf "$work/suites" "$work/raw"
fi

if [ "$step" != train ]; then
  test -s "$work/buri.profdata" || { echo "no profile at $work/buri.profdata" >&2; exit 1; }
  env "$cflags=-fno-profile-use" RUSTFLAGS="-Cprofile-use=$work/buri.profdata $link" \
    cargo build --release --features backend-llvm -p buri --target "$target"
fi
