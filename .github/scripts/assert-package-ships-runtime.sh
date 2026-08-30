#!/usr/bin/env bash
#
# The published `buri` crate carries the native runtime's sources.
#
# `cli/build.rs` compiles `cli/runtime/` into `libburi_rt.a` at toolchain-build
# time, so a registry tarball without those files is a `cargo install buri` that
# fails in the build script with nothing to compile. That is exactly what
# happened once: the runtime became a cargo package, and `cargo package` skips
# any subdirectory of a package that holds a `Cargo.toml` — unconditionally,
# ahead of `include`/`exclude`, in both its git-driven and its
# filesystem-driven file listers — so the whole directory silently stopped
# shipping. Nothing said so, because a checkout still builds.
#
# The fix is that the runtime's manifest is `manifest.toml` and its lockfile is
# `manifest.lock`, neither named the way Cargo would name it, and `cli/build.rs`
# assembles the real package in `OUT_DIR`. This is the assertion that the fix
# holds; `dependencies_stay_behind_the_bar` holds the invariant underneath it
# (no second `Cargo.toml` under `cli/`).
#
# `--list` rather than a real `cargo package`: the tarball is not wanted, only
# its file list, and building it would compile the whole toolchain a second
# time. `--allow-dirty` because CI checks out a tree with no git state to speak
# of and this assertion is about the manifest's rules, not about the index.

set -euo pipefail

listed=$(cargo package -p buri --list --allow-dirty)

missing=0
# A here-string rather than a pipe into `grep -q`: `set -o pipefail` and a grep
# that exits on its first match make the pipeline's status SIGPIPE, which reads
# as "no match" and would turn every one of these assertions inside out.
for f in lib.rs manifest.toml manifest.lock; do
  if ! grep -qx "runtime/$f" <<<"$listed"; then
    echo "::error::runtime/$f is not in the published buri crate. A cargo install from a registry tarball would fail in cli/build.rs with no runtime to compile. The usual cause is a Cargo.toml that has appeared under cli/ — cargo package skips the directory that holds one."
    missing=1
  fi
done

sources=$(grep -c '^runtime/.*\.rs$' <<<"$listed" || true)
# Sixteen at the time of writing. The bar is a floor rather than an equality so
# that adding a runtime source is not a CI failure, and it is not merely `> 0`
# so that shipping one file out of sixteen is not a pass.
if [ "$sources" -lt 16 ]; then
  echo "::error::the published buri crate carries $sources runtime sources; cli/runtime holds more than that. cargo package is dropping some of them."
  missing=1
fi

if [ "$missing" -ne 0 ]; then
  exit 1
fi

echo "the published buri crate carries $sources runtime sources, manifest.toml and manifest.lock"
