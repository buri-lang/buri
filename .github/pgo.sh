#!/bin/sh
# Builds target/<target>/release/buri with profile-guided optimization: an
# instrumented build runs half the e2e suites, and the release build reads the
# profile. Run from the repository root: `sh .github/pgo.sh aarch64-apple-darwin`.
#
# The other half (language docs failing fuzz linting adversarial ci) is held out,
# so a profile tuned to its own training shows up as a smaller win there.
set -eu
target=$1
work=$PWD/target/pgo
rm -rf "$work"
host=$(rustc -vV | sed -n 's/^host: //p')
profdata="$(rustc --print sysroot)/lib/rustlib/$host/bin/llvm-profdata"

# `test-hooks` lets the suites drive the instrumented build as they drive their
# own. Enough value counters keep the profiler from warning on stderr.
RUSTFLAGS="-Cprofile-generate=$work/raw -Cllvm-args=-vp-counters-per-site=8" \
  cargo build --release --features backend-llvm,test-hooks -p buri \
  --target "$target" --target-dir "$work/instrumented"

CARGO_TERM_COLOR=never cargo test -p buri --features backend-llvm --no-run --target-dir "$work/suites" 2>"$work/suites.log" \
  || { cat "$work/suites.log"; exit 1; }
cp "$work/instrumented/$target/release/buri" "$work/suites/debug/buri"
: > "$work/units.txt"
for unit in build native checking recovery formatting vectors; do
  sed -n "s|^ *Executable .* (\(.*/$unit-[0-9a-f]*\))\$|\1|p" "$work/suites.log" >> "$work/units.txt"
  grep -q "/$unit-[0-9a-f]*\$" "$work/units.txt" || { echo "no $unit suite to train on" >&2; exit 1; }
done
# Only the profile matters here, so a failing test doesn't stop the build.
xargs -P 8 -I{} sh -c '{} --test-threads=2 >/dev/null 2>&1 || true' < "$work/units.txt"

"$profdata" merge -o "$work/buri.profdata" "$work/raw"
rm -rf "$work/instrumented" "$work/suites" "$work/raw"

RUSTFLAGS="-Cprofile-use=$work/buri.profdata" \
  cargo build --release --features backend-llvm -p buri --target "$target"
