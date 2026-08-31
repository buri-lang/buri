#!/usr/bin/env bash
#
# The runtime archive this toolchain was built with is real, is the right size,
# carries every networking crate something calls into, and carries no networking
# crate that nothing does.
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
#      figure is, and it is a RATCHET: when one is hit, the right response is to
#      find out what grew, and then either fix it or re-measure and re-state the
#      number here with the new one written down.
#
#      IT HAS BEEN HIT ONCE, and the budgets below are the re-statement. Two
#      slices moved the archive after the one that measured it. B6 linked the
#      carrier runtime — `cli/runtime/rt.rs`, the reactor and its timer wheel —
#      which is 185 424 bytes of tokio on all three triples. C7 linked the TLS
#      client, which is 1.72 MiB, about 845 KB of it `ring`'s own C and assembly
#      object files, which a `staticlib` carries whether the linker needs them
#      or not. Together they put aarch64-apple-darwin at 8 198 904 bytes, over
#      the 7 MiB this script used to allow. The budgets below are the new
#      measurements: about 15 % of headroom over the figure measured here, and
#      about a fifth over the projected Linux one.
#
#      THE `net-h3` ARCHIVE IS HELD TO THE SAME BUDGET, and that is a measured
#      claim rather than an omission. `quinn` is referenced by nothing but
#      `net.rs`'s `size_of`, so fat LTO drops it whole: on aarch64-apple-darwin
#      the h3 archive is 8 198 992 bytes against the `net` one's 8 199 032 — the
#      h3 build is the SMALLER of the two by forty bytes, which is the refusal
#      string it does not need. A separate, larger h3 budget would therefore be
#      a number with nothing behind it, and would hide exactly the growth this
#      shared one catches: the slice that first CALLS into quinn has to come
#      back here and move a number, the same way B6 and C7 did.
#
#   3. THE NETWORKING CRATES, ON WHICHEVER SIDE OF THE LINE THEY ARE ON.
#      `net` brings five crates in. Three of them are now reached and the
#      archive MUST carry them: `tokio`, by `rt.rs`, since B6; `rustls` and
#      `ring`, by `cli/runtime/tls.rs`, since C7. An archive built with the
#      feature and carrying no TLS code would be a toolchain whose `https://`
#      fails at run time for a reason no test here would have caught, and one
#      carrying no reactor would be a toolchain whose every suspending host call
#      does. Two of them — `hyper` and `tungstenite` — are still referenced by
#      nothing but `net.rs`'s `size_of`, `lto = "fat"` drops them whole, and the
#      archive must NOT carry them. The slice that first links one of THOSE
#      moves it across the same way, deliberately, in the commit that does it.
#
#      `mio` and `socket2` reach the archive through tokio and are deliberately
#      on neither list: they are that crate's platform layer, not a seventh and
#      eighth dependency, and `dependencies_stay_behind_the_bar` is what holds
#      the direct set to six. The same goes for the twenty-odd crates `quinn`
#      brings with it, which is why it is named here and they are not.
#
#      `quinn` is the sixth crate and it is on the ABSENT list on every leg,
#      h3 included, for exactly the reason `hyper` and `tungstenite` are: the
#      feature brings the crate in and nothing calls it yet. F2 is the slice
#      that moves it, and it moves the budget above at the same time.
#
#      Which side is which is read from `libburi_rt.a.features`, written beside
#      the archive by the same run of `cli/build.rs` that produced it: a
#      `net`-off archive (no C compiler, an unresolvable tree, or
#      `BURI_RUNTIME_NET=0`) must carry none of the six, and the file is how
#      this script knows which of the claims to make. `net-h3` is a second whole
#      line in that file, never a substring of the first: `grep -qx` is what
#      keeps `net` and `net-h3` from reading as each other.
#
#   4. ONE CRYPTO PROVIDER, ON THE h3 LEG ESPECIALLY. `aws_lc` has been on the
#      absent list since C7 although it was never a dependency, because a second
#      provider appearing means a feature was enabled somewhere that quietly
#      doubled the cryptography in every binary. `quinn` is precisely the crate
#      that would do it — its own defaults are `rustls-aws-lc-rs` and
#      `platform-verifier` — and `manifest.toml` turns both off and asks for
#      `rustls-ring` instead. This script is what says so when quinn's defaults
#      next change.
#
# Usage: bash .github/scripts/assert-runtime-archive.sh [target-dir]
# Runs on macOS and Linux, unchanged.

set -euo pipefail

target=${1:-target}

case "$(uname -s)" in
  # 8 199 032 measured (8 198 992 with `net-h3`); 9 MiB leaves ~15 %.
  Darwin) budget=9437184 ;;
  # 8 469 832 was measured on the larger of the two Linux triples before either
  # the reactor or TLS; the macOS deltas put it near 10.5 MB and CI is where
  # that is confirmed. 12 MiB is ~20 % on that projection.
  Linux)  budget=12582912 ;;
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

# What the build script said it built, beside the bytes it built. Read before
# the budget rather than after, because since `net-h3` the feature list is what
# selects the budget as well as the symbol lists. Absent is not a pass: an
# archive with no feature file is an archive this script cannot make any claim
# about.
features="$best_file.features"
if [ ! -f "$features" ]; then
  echo "::error::$features is missing, so which networking crates this archive should carry is unknown and the assertions below would be a guess."
  exit 1
fi

if grep -qx "net-h3" "$features"; then
  echo "libburi_rt.a: built with \`net-h3\`"
  if ! grep -qx "net" "$features"; then
    echo "::error::$features names \`net-h3\` without \`net\`, and the manifest makes net-h3 imply net. Either cli/build.rs wrote a state cargo cannot produce, or the file belongs to a different archive."
    exit 1
  fi
fi

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
# as "no match" — so a piped version of these loops would report a clean archive
# precisely when the archive is not clean.
absent() {
  for crate in "$@"; do
    if grep -qi -- "$crate" <<<"$symbols"; then
      echo "::error::libburi_rt.a carries symbols from \`$crate\`, and nothing in the runtime is supposed to reach it. Either something new does — in which case this list and the size budget above both move, in the commit that made it deliberate — or a crate was added to cli/runtime/manifest.toml without an argument for it."
      status=1
    fi
  done
}

present() {
  for crate in "$@"; do
    if ! grep -qi -- "$crate" <<<"$symbols"; then
      echo "::error::libburi_rt.a carries NO symbol from \`$crate\`, but it was built with the runtime's \`net\` feature, which is what links the reactor and the TLS client. An archive in this state has an \`https://\` or a suspending host call that fails at run time and a test suite that never noticed. Check cli/runtime/rt.rs and cli/runtime/tls.rs are still reached."
      status=1
    fi
  done
}

# `$status` may already be 1 from the budget, so the "and it was fine" line is
# guarded by the status *these* checks left rather than printed unconditionally
# — a green sentence under a red one is how a reader ends up believing the wrong
# half of a log.
before=$status
if grep -qx "net" "$features"; then
  present tokio rustls ring_core
  # `aws_lc` is here although it was never a dependency: it is the OTHER
  # provider `rustls` ships, and a second one appearing means a feature was
  # enabled somewhere that quietly doubled the cryptography in every binary.
  # `quinn` is the crate most likely to do it, and it is on this list on both
  # legs anyway because nothing calls into it yet.
  absent hyper tungstenite quinn aws_lc
  if [ "$status" -eq "$before" ]; then
    echo "libburi_rt.a: the reactor and TLS are linked, and the unreferenced crates are still unreferenced"
  fi
else
  absent tokio hyper rustls tungstenite quinn ring_core aws_lc
  if [ "$status" -eq "$before" ]; then
    echo "libburi_rt.a: built without \`net\`, and carries none of the networking crates"
  fi
fi

exit "$status"
