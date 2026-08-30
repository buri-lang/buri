#!/usr/bin/env bash
#
# The runtime archive this toolchain was built with is real, is the right size,
# and carries none of the networking stack.
#
# THE LIVENESS GATE, and it is the runtime's half of the one
# `assert-stencils.sh` performs for the stencil libraries — for the same reason,
# which is that the failure it catches is SILENT. `cli/build.rs` degrades rather
# than breaks: a host with no runtime, or one whose dependency tree cannot be
# resolved, gets an EMPTY archive, `runtime_native::AVAILABLE` reads the
# emptiness and is false, and every native test opens by asking whether the
# backend is available. Such a runner therefore runs the whole suite, reports
# every test as PASSED, and proves nothing about a native artifact at all.
# `cargo test`'s exit status cannot tell that apart from a real green run — this
# can, because `AVAILABLE` is literally `!ARCHIVE.is_empty()` and the archive is
# a file.
#
# Three assertions, and each catches something different:
#
#   1. NOT EMPTY. The degradation above, and since the `net` feature there is a
#      second way into it: an unresolvable dependency tree. A stale
#      `cli/runtime/manifest.lock` degrades exactly like an unreachable
#      registry, which is right for a contributor on a plane and wrong for CI.
#
#   2. UNDER BUDGET. The archive is `include_bytes!`d into every `buri` binary,
#      so its size is the toolchain's size. The budget is per-OS because the
#      figure is: measured on this repository at the commit that added `net`,
#      5 987 496 bytes on aarch64-apple-darwin, 8 210 866 on
#      x86_64-unknown-linux-gnu and 8 469 832 on aarch64-unknown-linux-gnu —
#      the difference is `std` and the platform, not anything of ours. The
#      budgets below are those numbers with room for the runtime to grow, and
#      they are a RATCHET: when one is hit, the right
#      response is to find out what grew, and then either fix it or re-measure
#      and re-state the number here with the new one written down.
#
#   3. NO NETWORKING CODE. The claim the `net` feature was landed on: four
#      crates enter the dependency tree and NOTHING references them, so
#      `lto = "fat"` drops all of it and the archive grows by twenty-four bytes.
#      That claim is checkable directly — the archive's symbol table must not
#      mention any of the four, nor a crypto provider — and it is checked here
#      rather than trusted, because the whole point of landing the crates ahead
#      of the code is to know what they cost before anything depends on them.
#
#      This assertion is meant to be MOVED, once, by the slice that first links
#      one of them for real (design/native: C7 routes `https://` through hyper
#      and rustls). Moving it deliberately is the point; discovering the growth
#      in a binary six months later is what it prevents.
#
# Usage: bash .github/scripts/assert-runtime-archive.sh [target-dir]
# Runs on macOS and Linux, unchanged.

set -euo pipefail

target=${1:-target}

case "$(uname -s)" in
  # 5 987 496 measured; 7 MiB is ~17 % of headroom for the runtime itself.
  Darwin) budget=7340032 ;;
  # 8 469 832 measured, on the larger of the two Linux triples. 10 MiB is ~24 %.
  Linux)  budget=10485760 ;;
  *)      echo "::error::this script knows macOS and Linux only" ; exit 1 ;;
esac

# The largest archive written under the target directory. There is one
# `OUT_DIR` per build-script instantiation and a checkout can hold several;
# taking the largest is right because "some build produced a real archive" is
# the claim. `-path '*/out/*'` keeps this to the build script's own output and
# away from the nested target directory it also writes under `OUT_DIR/rt`.
best=0
best_file=
while IFS= read -r f; do
  sz=$(wc -c < "$f" | tr -d ' ')
  if [ "$sz" -gt "$best" ]; then
    best=$sz
    best_file=$f
  fi
done < <(find "$target" -path "*/out/libburi_rt.a" -type f 2>/dev/null)

if [ -z "$best_file" ]; then
  echo "::error::no libburi_rt.a was found under $target at all — nothing has been built yet, so this assertion was vacuous."
  exit 1
fi

status=0

if [ "$best" -eq 0 ]; then
  echo "::error::libburi_rt.a is empty — this toolchain has no native runtime, so every native test will skip and the suite will pass vacuously. cli/build.rs writes an empty archive on an unsupported host and on a dependency tree it cannot resolve; the build log carries a cargo:warning saying which."
  exit 1
fi
echo "libburi_rt.a: $best bytes ($best_file)"

if [ "$best" -gt "$budget" ]; then
  echo "::error::libburi_rt.a is $best bytes, over the $budget-byte budget for $(uname -s). Every buri binary carries these bytes. Find out what grew — the usual answer is a dependency whose code now reaches the archive, or a native library a crate compiled in — then fix it, or re-measure and re-state the budget in this script."
  status=1
else
  echo "libburi_rt.a: within the $budget-byte budget for $(uname -s)"
fi

# `nm` over an archive lists every member's symbols. A failure to read it is
# not a pass: `|| true` would turn a missing `nm` into a green assertion, so the
# symbol dump is taken once and its emptiness is checked.
symbols=$(nm "$best_file" 2>/dev/null || true)
if [ -z "$symbols" ]; then
  echo "::error::nm produced no symbols for $best_file, so the networking assertion below would pass vacuously."
  exit 1
fi

# A here-string rather than a pipe into `grep -q`: `set -o pipefail` and a grep
# that exits on its first match make the pipeline's status SIGPIPE, which reads
# as "no match" — so a piped version of this loop would report a clean archive
# precisely when the archive is not clean.
for crate in tokio hyper rustls tungstenite ring_core aws_lc; do
  if grep -qi -- "$crate" <<<"$symbols"; then
    echo "::error::libburi_rt.a carries symbols from \`$crate\`. The net feature was landed on the claim that the four crates are referenced by NOTHING and therefore cost twenty-four bytes; something now references one. If that is deliberate, this list and the size budget above both move, in the commit that made it deliberate."
    status=1
  fi
done
if [ "$status" -eq 0 ]; then
  echo "libburi_rt.a: no networking symbols, as a runtime nothing calls into must have none"
fi

exit "$status"
