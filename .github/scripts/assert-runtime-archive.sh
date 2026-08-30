#!/usr/bin/env bash
#
# The runtime archive this toolchain was built with is real, is the right size,
# and carries no networking crate that nothing calls into.
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
#      the difference is `std` and the platform, not anything of ours. Slice B6
#      then linked the reactor, which is 185 424 bytes of the same code on all
#      three: 6 220 904 measured on aarch64-apple-darwin, and the Linux pair
#      rise by that. The budgets below are those numbers with room for the
#      runtime to grow, and they are a RATCHET: when one is hit, the right
#      response is to find out what grew, and then either fix it or re-measure
#      and re-state the number here with the new one written down.
#
#   3. NO NETWORKING CODE THAT NOTHING CALLS. The `net` feature was landed on
#      the claim that its four crates are referenced by NOTHING, so
#      `lto = "fat"` drops all of it and the archive grows by twenty-four
#      bytes. The claim is checkable directly — the archive's symbol table must
#      not mention a crate nothing reaches, nor a crypto provider — and it is
#      checked here rather than trusted, because the whole point of landing the
#      crates ahead of the code was to know what they cost before anything
#      depended on them.
#
#      TOKIO IS NO LONGER ON THE LIST, and this is the deliberate move the
#      previous version of this comment said would come. `cli/runtime/rt.rs`
#      (design/native track B, slice B6) is the carrier runtime — the reactor,
#      the run baton, the carrier pool and the task table — and
#      `Clock::sleepMillis` and `Net::fetch` wait on it, so the reactor's code
#      is in the archive on purpose. Measured on aarch64-apple-darwin at the
#      commit that did it:
#
#          before  6 035 480 bytes      (tokio in the tree, referenced by nothing)
#          after   6 220 904 bytes      +185 424, the reactor and its timer wheel
#
#      Which is 85 % of the Darwin budget below, so the budget does not move
#      with it — see the ratchet in 2 for what to do when it does. `mio` and
#      `socket2` reach the archive through tokio and are deliberately NOT on
#      the list: they are that crate's platform layer, not a fifth and sixth
#      dependency, and `dependencies_stay_behind_the_bar` is what holds the
#      direct set to four.
#
#      The other three are still called by nothing but `net.rs`'s `size_of`,
#      and the list is still meant to be MOVED — by the slice that first links
#      one of THEM (design/native: C7 routes `https://` through hyper and
#      rustls). Moving it deliberately is the point; discovering the growth in
#      a binary six months later is what it prevents.
#
# Usage: bash .github/scripts/assert-runtime-archive.sh [target-dir]
# Runs on macOS and Linux, unchanged.

set -euo pipefail

target=${1:-target}

case "$(uname -s)" in
  # 6 220 904 measured since the reactor was linked; 7 MiB leaves ~15 %.
  Darwin) budget=7340032 ;;
  # 8 469 832 measured before it, on the larger of the two Linux triples, and
  # the reactor is 185 424 bytes of the same code there. 10 MiB leaves ~17 %.
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
for crate in hyper rustls tungstenite ring_core aws_lc; do
  if grep -qi -- "$crate" <<<"$symbols"; then
    echo "::error::libburi_rt.a carries symbols from \`$crate\`. The net feature was landed on the claim that its crates are referenced by NOTHING and therefore cost twenty-four bytes; something now references this one. If that is deliberate, this list and the size budget above both move, in the commit that made it deliberate — as slice B6 moved \`tokio\` off it, with the measurement in this file's header."
    status=1
  fi
done
if [ "$status" -eq 0 ]; then
  echo "libburi_rt.a: no symbols from a networking crate nothing calls into"
fi

exit "$status"
