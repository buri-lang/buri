#!/usr/bin/env bash
#
# The stencil libraries this toolchain was built with are not empty.
#
# THE LIVENESS GATE, and the reason it exists is that the failure it catches is
# SILENT. `cli/build.rs` degrades rather than breaks: a host whose `cc` cannot
# compile the generated C gets an EMPTY library, `stencil::AVAILABLE` reads the
# emptiness and is false, and every test in `cli/tests/native/stencil.rs` opens
# with `if !supported() { return; }`. Such a runner therefore runs the whole
# suite, reports every test as PASSED, and proves nothing at all. `cargo test`'s
# exit status cannot tell that apart from a real green run — this can, because
# `available_for` is literally `!blob(t).0.is_empty()`
# (cli/src/compiler/backend/stencil/mod.rs) and the blobs are files.
#
# It also asserts the *reverse* degrade: a Linux host cannot build the
# `macos-arm64` library (`cli/build.rs` gates that target on an
# `aarch64-apple-darwin` TARGET), so on Linux that blob must be empty. An
# assertion that only ever says "present" would not notice a build script that
# started writing every target unconditionally.
#
# Usage: bash .github/scripts/assert-stencils.sh [target-dir]
# Runs on macOS too, unchanged, where the expectation is the other way round.

set -euo pipefail

target=${1:-target}

case "$(uname -s)" in
  Darwin) host_slug=macos-arm64 ; empty_slug=""            ;;
  Linux)  host_slug=""          ; empty_slug=macos-arm64   ;;
  *)      echo "::error::this script knows macOS and Linux only" ; exit 1 ;;
esac

# On Linux the host library is whichever Linux triple this machine is; on macOS
# it is `macos-arm64`. Both Linux libraries are cross-compilable from any clang,
# so both are required on every host — that is what makes `linux-arm64` a
# container port rather than a second backend.
required="linux-arm64 linux-x86_64 ${host_slug}"

# The largest blob written for a slug. There is one `OUT_DIR` per build-script
# instantiation and a checkout can hold several; taking the largest is right
# because "some build produced a real library" is the claim.
size_of() {
  local slug=$1 best=0 f sz
  while IFS= read -r f; do
    sz=$(wc -c < "$f" | tr -d ' ')
    [ "$sz" -gt "$best" ] && best=$sz
  done < <(find "$target" -path "*/out/stencils-$slug.bin" -type f 2>/dev/null)
  echo "$best"
}

found_any=0
status=0

for slug in $required; do
  [ -n "$slug" ] || continue
  sz=$(size_of "$slug")
  if [ "$sz" -gt 0 ]; then
    found_any=1
    echo "stencils-$slug.bin: $sz bytes"
  else
    echo "::error::stencils-$slug.bin is empty or absent — this toolchain has no $slug stencils, so every test guarded on availability will skip and the suite will pass vacuously. Check that CC is clang and that clang can cross-compile."
    status=1
  fi
done

for slug in $empty_slug; do
  [ -n "$slug" ] || continue
  sz=$(size_of "$slug")
  if [ "$sz" -eq 0 ]; then
    echo "stencils-$slug.bin: empty, as a $(uname -s) host must leave it"
  else
    echo "::error::stencils-$slug.bin is $sz bytes on a $(uname -s) host — cli/build.rs is meant to degrade that target here, so either the gate moved or these are not the bytes they claim to be."
    status=1
  fi
done

if [ "$found_any" -eq 0 ]; then
  echo "::error::no stencil blobs were found under $target at all — nothing has been built yet, so this assertion was vacuous."
  exit 1
fi

exit "$status"
