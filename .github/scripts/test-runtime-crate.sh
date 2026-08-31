#!/usr/bin/env bash
#
# **The runtime crate's own tests, as a step rather than as a test.**
#
# `cli/runtime` is a cargo package that is deliberately not a workspace member
# and whose manifest is deliberately not called `Cargo.toml` — `cli/build.rs`'s
# header argues both — so `cargo test -p buri` cannot reach the `#[cfg(test)]`
# modules inside it. Ninety-seven assertions about the float formatter, the
# UTF-16 comparison, the handle table, the allocator, the reactor and the TLS
# client live there.
#
# `native::runtime::the_runtime_crate_answers_its_own_tests` is what started
# running them, by shelling a nested `cargo test` from inside a test. That
# works and it is the right thing on a laptop; on a runner it was the wrong
# shape for three reasons, and this script is each of them undone:
#
#   1. **It compiled tokio, hyper and rustls inside a test.** Sixty seconds and
#      more, every run, because the nested target directory was under
#      `CARGO_TARGET_TMPDIR` — which `harness/sweep.rs` deletes after two idle
#      hours and which no CI cache has ever heard of. As a step it gets a cache
#      key of its own (`cli/runtime/manifest.lock`), so the second run pays for
#      the link and nothing else.
#   2. **The time was invisible.** A step has a duration in the run summary and
#      a test inside a two-hundred-test binary does not, so "the suite got
#      slower" could not be attributed without running the suite by hand.
#   3. **A failure was reported as a test failing rather than as the runtime's
#      tests failing.** The nested `cargo`'s own output arrived as an
#      assertion message, indented inside another test harness's report.
#
# What did NOT change is that the tests run everywhere. Off a runner, the
# meta-test still shells the nested `cargo` exactly as it did; on a runner it
# asserts the stamp this script writes, so a job that drops the step below
# fails on the meta-test instead of quietly testing nothing. That is the whole
# of the arrangement and `cli/tests/native/runtime.rs` states the other half of
# it.
#
# Usage: bash .github/scripts/test-runtime-crate.sh [target-dir]
#
# `BURI_RT_TESTS_STAMP` names the file written on success. It is set in the
# workflow's `env:` block so that the step and the meta-test agree on the path
# without either of them computing it.

set -euo pipefail

target=${1:-target}
stamp=${BURI_RT_TESTS_STAMP:-$target/runtime-crate-tests.ok}

# WHICH assembled package, and the answer is "every distinct one".
#
# The build script writes one `$OUT_DIR/rt-pkg` per instantiation, and a target
# directory can hold several: a restored cache has the previous run's, a
# `--features` build gets its own, and cargo's own metadata hash decides which
# one the binary beside them was compiled against. Picking the newest by mtime
# is wrong and was wrong the first time it was tried — `assemble` does not
# rewrite a file whose bytes have not changed, so the newest manifest is not the
# one in use.
#
# So they are DEDUPLICATED BY THE ARCHIVE'S DIGEST — `libburi_rt.a.sha256`,
# written beside the archive by the same run of the build script — and each
# distinct one is tested. Two packages with the same digest are the same
# runtime and one run answers for both; two with different digests are two
# runtimes (a `net` build and a `net`-off one, say) and both deserve a run. The
# stamp records every digest tested, and
# `runtime_native::archive_hash()` is what the meta-test looks for in it, so
# "the tests that ran were the tests of the runtime in THIS binary" is a
# comparison of content rather than of paths.
manifests=
seen_digests=
found=0
while IFS= read -r candidate; do
  found=$((found + 1))
  out_dir=$(dirname "$(dirname "$candidate")")
  digest_file="$out_dir/libburi_rt.a.sha256"
  if [ ! -f "$digest_file" ]; then
    echo "::warning::$candidate has no libburi_rt.a.sha256 beside it, so which runtime it is cannot be established; skipping it."
    continue
  fi
  digest=$(cat "$digest_file")
  case " $seen_digests " in
    *" $digest "*) continue ;;
  esac
  seen_digests="$seen_digests $digest"
  manifests="${manifests}${candidate}
"
done < <(find "$target" -path "*/out/rt-pkg/Cargo.toml" -type f 2>/dev/null | sort)

if [ -z "${manifests:-}" ]; then
  echo "::error::no assembled runtime package was found under $target. \`cli/build.rs\` writes one at \$OUT_DIR/rt-pkg on every host it builds a runtime for, so either nothing has been built yet (run \`cargo build -p buri --tests\` first) or this host has no runtime — which \`assert-runtime-archive.sh\` would have caught before this step."
  exit 1
fi
echo "$found assembled package(s) under $target, $(printf '%s' "$manifests" | grep -c .) distinct runtime(s) to test"

# The same treatment `cli/build.rs`'s nested cargo gets and for the same
# reason: an inherited `CARGO_*` carries the outer invocation's target-directory
# lock and jobserver into a build that is not part of it. `CARGO_HOME` stays,
# because the registry is shared on purpose.
while IFS='=' read -r name _; do
  case "$name" in
    CARGO_HOME) ;;
    CARGO_*) unset "$name" ;;
  esac
done < <(env)
unset CARGO RUSTFLAGS

tested=
n=0
while IFS= read -r manifest; do
  [ -n "$manifest" ] || continue
  n=$((n + 1))
  out_dir=$(dirname "$(dirname "$manifest")")
  digest=$(cat "$out_dir/libburi_rt.a.sha256")

  # The features the archive beside this package was actually built with, so
  # this runs the tests of *that* runtime rather than of a differently-featured
  # one. Absent is not a pass: a guess about the feature set is a guess about
  # which tests exist.
  features="$out_dir/libburi_rt.a.features"
  if [ ! -f "$features" ]; then
    echo "::error::$features is missing, so which features that runtime was built with is unknown and this run would test a different crate from the one in the binary."
    exit 1
  fi

  # A target directory per distinct runtime, beside the archive's rather than
  # inside it: the build script's `$OUT_DIR/rt` holds a `--release --lib` build
  # for the host triple and this is a dev-profile build with dev-dependencies,
  # so sharing the directory would be two profiles fighting over one lock for
  # no reuse at all. It is named under `$target` so the workflow's cache step
  # can address it.
  args=(test --locked --manifest-path "$manifest" --target-dir "$target/runtime-crate-tests/$n")
  if grep -qx "net" "$features"; then
    echo "runtime ${digest:0:12}: features net"
  else
    echo "runtime ${digest:0:12}: features none (built without \`net\`)"
    args+=(--no-default-features)
  fi

  echo "+ cargo ${args[*]}"
  cargo "${args[@]}"
  tested="${tested}runtime: $digest
"
done <<EOF
$manifests
EOF

mkdir -p "$(dirname "$stamp")"
{
  echo "ok"
  printf '%s' "$tested"
  date -u +"ran: %Y-%m-%dT%H:%M:%SZ"
} >"$stamp"
echo "wrote $stamp"
cat "$stamp"
