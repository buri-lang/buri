#!/usr/bin/env bash
# The protobuf conformance suite, against the codecs Buri generates from a
# `.proto` schema.
#
# Out of `cargo test` on purpose, the way `editors/tree-sitter-buri/check.sh` is:
# the runner is a C++ binary from another project, and a test suite that cannot
# run without one is a test suite that does not run. `cli/tests/proto_vectors.rs`
# is the part that does run under cargo — it replays exchanges recorded here.
#
#   ./run.sh              build the testee and run the suite
#   ./run.sh --update     the same, then rewrite failure_list.txt's names
#   ./run.sh --record     the same, then re-record vectors.txt
#
# `conformance_test_runner` has to be on PATH. README.md says how to get one.
set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
repo="$here/repo"
root="$(cd "$here/../../.." && pwd)"
mode="${1:-}"

runner="${CONFORMANCE_TEST_RUNNER:-conformance_test_runner}"
if ! command -v "$runner" >/dev/null 2>&1 && [ ! -x "$runner" ]; then
  cat >&2 <<'MSG'
error: conformance_test_runner is not on PATH.

It is protobuf's own C++ test driver, and nixpkgs does not package it — it is a
test binary rather than a shipped one. Build it from the protobuf release this
directory vendors (see README.md for the version), then put it on PATH or point
CONFORMANCE_TEST_RUNNER at it:

  curl -LO https://github.com/protocolbuffers/protobuf/releases/download/v35.1/protobuf-35.1.tar.gz
  tar xzf protobuf-35.1.tar.gz && cd protobuf-35.1
  nix-shell -p cmake ninja abseil-cpp zlib pkg-config --run '
    cmake -S . -B build -G Ninja -DCMAKE_BUILD_TYPE=Release \
      -Dprotobuf_BUILD_TESTS=OFF -Dprotobuf_BUILD_CONFORMANCE=ON \
      -Dprotobuf_ABSL_PROVIDER=package &&
    cmake --build build --target conformance_test_runner -j8'
  export CONFORMANCE_TEST_RUNNER=$PWD/build/conformance_test_runner
MSG
  exit 2
fi

buri="${BURI:-$root/target/debug/buri}"
if [ ! -x "$buri" ]; then
  echo "error: no buri at $buri — run \`cargo build -p buri\`, or set BURI" >&2
  exit 2
fi

js="$(command -v bun || command -v node || true)"
if [ -z "$js" ]; then
  echo "error: neither bun nor node is on PATH; the testee is a JavaScript artifact" >&2
  exit 2
fi

echo "building the testee"
(cd "$repo" && "$buri" build //cmd/testee --force >/dev/null)
artifact="$repo/.buri/out/js/cmd/testee/testee.mjs"

work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT

cat > "$work/testee.sh" <<EOF
#!/bin/sh
exec "$js" "$artifact"
EOF
chmod +x "$work/testee.sh"

testee="$work/testee.sh"
if [ "$mode" = "--record" ]; then
  : > "$work/frames.txt"
  # Node, specifically: the tap pumps `process.stdin` by event, and Bun's
  # standard input does not deliver those the way the recording needs.
  node="$(command -v node || true)"
  if [ -z "$node" ]; then
    echo "error: --record needs node (bun's stdin events do not pump the tap)" >&2
    exit 2
  fi
  cat > "$work/record.sh" <<EOF
#!/bin/sh
RECORD_TO="$work/frames.txt" TESTEE="$artifact" JS="$js" exec "$node" "$here/record.mjs"
EOF
  chmod +x "$work/record.sh"
  testee="$work/record.sh"
fi

set +e
"$runner" --output_dir "$work" --failure_list "$here/failure_list.txt" "$testee" \
  | tee "$work/report.txt"
status=$?
set -e

grep "CONFORMANCE SUITE" "$work/report.txt" || true

case "$mode" in
  --update)
    if [ -s "$work/failing_tests.txt" ]; then
      echo
      echo "unexpected failures, written to $here/unexpected.txt — classify each one"
      echo "and move it under the matching heading in failure_list.txt."
      cp "$work/failing_tests.txt" "$here/unexpected.txt"
    else
      echo "nothing unexpected: failure_list.txt is current"
    fi
    ;;
  --record)
    python3 "$here/record.py" "$work/frames.txt" "$here/vectors.txt"
    echo "re-recorded $here/vectors.txt"
    ;;
esac

exit $status
