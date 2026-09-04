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
#      IT HAS BEEN HIT ONCE, and the budgets below are that re-statement. Two
#      slices moved the archive after the one that measured it. B6 linked the
#      carrier runtime — `cli/runtime/rt.rs`, the reactor and its timer wheel —
#      which is 185 424 bytes of tokio on all three triples. C7 linked the TLS
#      client, which is 1.72 MiB, about 845 KB of it `ring`'s own C and assembly
#      object files, which a `staticlib` carries whether the linker needs them
#      or not. Together they put aarch64-apple-darwin at 8 198 904 bytes, over
#      the 7 MiB this script used to allow, and 9 MiB is what replaced it.
#
#      A THIRD SLICE HAS SINCE MOVED IT. F4 linked `hyper`'s HTTP/2 server —
#      and with it `h2`, `tokio-util` and HPACK's tables — for 771 688 bytes,
#      putting aarch64-apple-darwin at 8 970 592. That is UNDER the 9 MiB above,
#      by about 4.9 %, so the Darwin number is left where it is: this budget
#      moves when it is hit, and it was not hit.
#
#      A FOURTH SLICE HAS SINCE MOVED IT, AND THE MARGIN IS NOW THIN ENOUGH TO
#      SAY SO IN CAPITALS. F7 linked `tungstenite` — RFC 6455 framing, the
#      accept-key hash, and the `http`/`httparse` layers it shares with hyper —
#      and re-measured all three archives on aarch64-apple-darwin, from scratch,
#      on the tree that did it:
#
#        net off                                             6 322 696 bytes
#        net on, before F7 (the reactor, TLS, HTTP/2)         9 013 416
#        net on, after F7 (WebSockets as well)                9 090 776    +77 360
#        net-h3 on as well                                    9 091 024       +248
#
#      Seventy-seven kilobytes for a whole protocol is small and it is not a
#      surprise: `http` and `httparse` were already live for hyper's sake, so
#      what arrived is the framing, the masking and one SHA-1. The Darwin budget
#      is STILL NOT HIT — 9 090 776 is under 9 MiB by 346 408 bytes — so it does
#      not move, for the reason the paragraph above gives. WHAT HAS CHANGED IS
#      THE HEADROOM: 4.9 % became 3.7 %, and the next slice to add anything at
#      all to this archive should expect to be the one that re-measures and
#      re-states this number rather than the one that squeezes under it.
#
#      F8 IS THAT SLICE AND IT DID, though what it added is small: the
#      `sockets()` test double, which is about two hundred lines of `testing.rs`
#      over the handle table that was already there and links no dependency at
#      all. All three archives re-measured from scratch on aarch64-apple-darwin,
#      on the tree that did it:
#
#        net off                                             6 329 104 bytes  +6 408
#        net on                                              9 097 192        +6 416
#        net-h3 on as well                                   9 097 432          +240
#
#      Six kilobytes for a whole double is the shape to expect from anything
#      that is code rather than a crate, and the h3 delta is 240 bytes against
#      F7's 248, which is the same "quinn is dropped whole" measurement taken
#      again. The Darwin budget is STILL NOT HIT — 9 097 192 is under 9 MiB by
#      339 992 bytes — so it does not move. THE HEADROOM IS NOW 3.6 %, and the
#      sentence above stands unchanged for whoever comes next.
#
#      LINUX WAS RE-MEASURED IN THE SAME RUN, in the container
#      `scripts/test-linux.sh` runs: 13 799 068 bytes on
#      aarch64-unknown-linux-gnu, which is +10 896 for the same two hundred
#      lines — 1.70x the Darwin delta against an archive 1.52x as large, the
#      same shape of ratio F4 and F7 both measured. Not hit, does not move,
#      ~6.0 % left.
#
#      THE LINUX BUDGET IS A DIFFERENT STORY AND IT IS THE CAUTIONARY ONE.
#      Every Linux figure this script has ever carried was a PROJECTION — the
#      macOS delta added to an 8 469 832-byte measurement taken before the
#      reactor, TLS or HTTP/2 — and the comment that carried it said CI was
#      where a projection becomes a measurement. F4 is when that happened, and
#      the projection was wrong by 1.9 MB in the direction that matters:
#
#        aarch64-unknown-linux-gnu, `net`
#          before F4 (the projection said ~10.5 MB)   12 407 786 bytes
#          after F4                                   13 611 228   +1 203 442
#
#      So Linux had been sitting at 98.6 % of a 12 MiB budget for two slices
#      with nobody the wiser, and hyper's legitimate bytes are what pushed it
#      over. The growth itself is honest: Linux is 1.51x Darwin's archive before
#      the slice and hyper costs 1.56x there — the same code, at ELF's price per
#      byte, and not a strip that stopped working. 14 MiB is the re-measurement,
#      with about 7 % over the larger of the two Linux figures.
#
#      RE-MEASURED AGAIN BY F7, in the same container `scripts/test-linux.sh`
#      runs: 13 788 172 bytes on aarch64-unknown-linux-gnu, which is +176 944
#      for the WebSocket framing — 2.3x the Darwin delta against an archive that
#      is 1.52x as large, which is ELF's price for the same code and the same
#      shape of ratio F4 measured for hyper. The Linux budget is NOT hit and
#      does not move: 6.1 % of it is left, which is more headroom than Darwin
#      now has.
#
#      The lesson for the next re-statement is in the numbers rather than in
#      this paragraph: a budget nothing has measured is not a budget, and the
#      two Linux triples should both be read off a real job before either is
#      trusted again.
#
#      THE `net-h3` ARCHIVE IS HELD TO THE SAME BUDGET, and that is a measured
#      claim rather than an omission. `quinn` is referenced by nothing but
#      `net.rs`'s `size_of`, so fat LTO drops it whole: on aarch64-apple-darwin
#      the h3 archive is 9 091 024 bytes against the `net` one's 9 090 776 — a
#      QUIC implementation for 248 bytes, which is the refusal string an h3
#      build does not need going away and the feature line arriving. A separate,
#      larger h3 budget would therefore be a number with nothing behind it, and
#      would hide exactly the growth this shared one catches: the slice that
#      first CALLS into quinn has to come back here and move a number, the same
#      way B6 and C7 did.
#
#   3. THE NETWORKING CRATES, ON WHICHEVER SIDE OF THE LINE THEY ARE ON.
#      `net` brings five crates in and ALL FIVE are now reached, so the archive
#      MUST carry every one of them: `tokio`, by `rt.rs`, since B6; `rustls` and
#      `ring`, by `cli/runtime/tls.rs`, since C7; `hyper`, by
#      `cli/runtime/net.rs`, since F4 — which is the HTTP/2 half of the
#      acceptor, the multiplexing and the ALPN that F2 and F3 both deferred to
#      it by name; and `tungstenite`, by the same file, since F7 — which is the
#      RFC 6455 framing behind `listenUpgrade` and `listenReceive`. An archive
#      built with the feature and carrying no TLS code would be a toolchain
#      whose `https://` fails at run time for a reason no test here would have
#      caught; one carrying no reactor would be a toolchain whose every
#      suspending host call does; one carrying no `hyper` would be a server that
#      negotiates `h2` in its handshake and then has nothing to frame it with;
#      and one carrying no `tungstenite` would be a server that answers a
#      WebSocket upgrade with a `101` and then cannot read a frame.
#
#      THE ABSENT LIST IS DOWN TO ITS LAST REAL ENTRY, which is worth saying
#      because it is what this check is *for*: every crate that has crossed did
#      so in the commit that first called into it, and the size budget was
#      re-measured in the same commit each time.
#
#      `mio` and `socket2` reach the archive through tokio, and `h2`,
#      `tokio-util` and `http` reach it through hyper. All five are deliberately
#      on neither list: they are their crate's own layers, not further
#      dependencies, and `dependencies_stay_behind_the_bar` is what holds the
#      direct set to six. The same goes for the twenty-odd crates `quinn` brings
#      with it, which is why it is named here and they are not.
#
#      `quinn` is the sixth crate and it is on the ABSENT list on every leg,
#      h3 included, for the reason `tungstenite` was until F7: the feature
#      brings the crate in and nothing calls it yet. The slice that first drives
#      HTTP/3 moves it, and re-measures the budget above at the same time.
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
#   5. THE ENTROPY DOOR, ON WHICHEVER SIDE OF `crypto` THE ARCHIVE IS ON.
#      `buri_rt_host_entropy_bytes` is compiled under `#[cfg(feature =
#      "crypto")]` and is the one symbol the backends emit a call to for
#      `Entropy`. It is checked by SYMBOL and not by crate, unlike everything in
#      3 above, because `ring` brings a `getrandom` of its own: a `crypto`-off,
#      `net`-on archive carries getrandom code regardless, so no claim about the
#      crate name could be true. The claim about the symbol is exact in both
#      directions — present with the feature, absent without it — and each
#      direction names a real failure. Missing with the feature on is a
#      toolchain whose every `crypto.randomBytes` dies at the system linker;
#      present with the feature off is a compile-time refusal standing in front
#      of a body that was there all along.
#
# Usage: bash .github/scripts/assert-runtime-archive.sh [target-dir]
# Runs on macOS and Linux, unchanged.
#
# THERE IS NO ESCAPE HATCH, AND THERE WAS ONE. `BURI_ARCHIVE_LIBC_MAY_BE_GLIBC=1`
# used to downgrade the "WHICH LIBC" assertion below — and only that one — from
# an error to a warning, for exactly one caller: `flake.nix`, whose sandbox had
# no musl `rust-std` and no `rustup` to add one with, and whose Linux archive
# was therefore a `gnu` archive. That flake now brings its own musl-capable
# toolchain (`rust-overlay`, `targets = [ "<arch>-unknown-linux-musl" ]`) and
# builds a musl archive like everything else, so the last caller is gone and
# the variable with it. It is not kept "just in case": a build that needs it is
# a build that has disarmed the one check standing between it and a silently
# non-portable release, and the honest way to ask for that is to write the
# argument down here, in a diff someone reviews.

set -euo pipefail

target=${1:-target}

case "$(uname -s)" in
  # 9 107 264 measured with `net` on aarch64-apple-darwin, which is 10 072 bytes
  # more than F8's 9 097 192: `Fs` and `Env` reaching both runtime tables
  # (buri-lang/buri#36) plus `removeDir` (buri-lang/buri#38). The bodies were
  # already in the archive and unreferenced, so what arrived is what `lto =
  # "fat"` had been deleting — eleven `std::fs` call sites and one `std::env`.
  # 9 MiB leaves ~3.5 %, which is the thinnest this has been. See the note above.
  Darwin) budget=9437184 ;;
  # 13 799 068 measured on aarch64-unknown-linux-gnu since F8 added the socket
  # double; F7's figure for the same triple was 13 788 172, and F4's were
  # 13 611 228 here and 13 515 516 on CI. 14 MiB leaves ~6.0 % on the largest.
  # THESE NUMBERS REPLACED A PROJECTION, and the projection was wrong by
  # 1.9 MB — see the note below.
  #
  # THE MUSL SWITCH DID NOT NEED THIS RAISED, and that is a measurement of the
  # real archive rather than a projection — this file has been wrong that way
  # once already, so the number below was taken from a built one and not
  # computed. 13 938 046 on aarch64-unknown-linux-musl, against the 13 799 068
  # for the gnu triple above: 139 KB and 1.0 % larger, which is musl's standard
  # library rather than anything this repository did. 14 MiB still holds, with
  # 5.05 % left rather than 6.0 %, and the next 740 KB is what will need a new
  # figure here.
  #
  # The reason the switch costs so little is the same measurement that forced
  # `cli/build.rs` to bake a sysroot at all: rustc does *not* fold musl's
  # `libc.a` into a staticlib, so `nm` over the archive above finds `malloc`,
  # `free`, `memcpy`, `mmap` and `_Unwind_*` all still undefined. Those 6.6 MB
  # land in the toolchain binary, through `src/build/musl.rs`, and not in this
  # file. This budget is over the archive alone and always has been.
  Linux)  budget=14680064 ;;
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

# WHICH LIBC, AND ON LINUX THERE IS ONLY ONE RIGHT ANSWER.
#
# `cli/build.rs` builds the runtime for `<arch>-unknown-linux-musl` on a Linux
# host so that the executables `buri build` produces are static-PIE binaries
# that run on any Linux. It degrades to the host's glibc triple when that
# target's `rust-std` is not installed — with a `cargo:warning`, which is the
# right answer for a contributor on a plane and the wrong one for a release.
#
# A toolchain built that way is not broken; it is quietly *less portable*, and
# nothing downstream fails in a way anyone would notice. Every executable it
# links just carries a dependency on the CI runner's glibc version, and the
# first report of it comes from a user on an older distro. That is precisely the
# silent green this script exists to refuse, so on Linux a `gnu` sidecar is an
# error rather than a warning. `rustup target add <arch>-unknown-linux-musl` in
# the workflow is the fix; the build log names it too.
#
# On macOS the file is empty and that is correct — there is no Linux libc there
# to have an opinion about.
libc="$best_file.libc"
if [ ! -f "$libc" ]; then
  echo "::error::$libc is missing, so which C library this archive was built against is unknown. cli/build.rs writes it on every path that writes an archive, so a missing file means the archive and this script came from different builds."
  exit 1
fi
libc_says=$(cat "$libc")
case "$(uname -s)" in
  Linux)
    case "$libc_says" in
      musl) echo "libburi_rt.a: built against musl" ;;
      gnu)
        echo "::error::libburi_rt.a was built against glibc, not musl. Executables this toolchain links will depend on the build machine's glibc and will not run on a Linux with an older one — which fails for users and for nobody in CI. cli/build.rs falls back to the host triple when the musl standard library is missing: add \`rustup target add \$(rustc -vV | sed -n 's/host: //p' | sed 's/-linux-gnu/-linux-musl/')\` to the job that built this."
        status=1
        ;;
      *)
        echo "::error::$libc says '$libc_says', and on Linux the only values that mean anything are \`musl\` and \`gnu\`. An empty file here means the archive above was written by a path that thought there was no archive at all."
        status=1
        ;;
    esac
    ;;
  Darwin)
    if [ -n "$libc_says" ]; then
      echo "::error::$libc says '$libc_says' on macOS, where there is no Linux libc to name. cli/build.rs writes this file empty on Darwin, so a value here means the file belongs to a different build than the archive beside it."
      status=1
    else
      echo "libburi_rt.a: no Linux libc to name (macOS)"
    fi
    ;;
esac

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
raw_symbols=$(nm "$best_file" 2>/dev/null || true)
if [ -z "$raw_symbols" ]; then
  echo "::error::nm produced no symbols for $best_file, so the networking assertion below would pass vacuously."
  exit 1
fi

# THE INSTANTIATING CRATE IS NOT THE DEFINING CRATE, and a substring grep over
# mangled names cannot tell them apart. This line is what makes the lists below
# mean "code FROM this crate" rather than "this crate's name appears somewhere".
#
# Rust's v0 mangling ends a monomorphised generic with the crate that
# instantiated it: `<alloc::raw_vec::RawVec<http::header::map::Bucket<..>>>::grow_one`
# compiled in tungstenite's codegen unit is
#
#     _RNvMs4_NtCs<..>_5alloc7raw_vec..._4http6header3map6Bucket...Cs<..>_11tungstenite
#          ^^^^^^^^^^^^^^^^ the path: `alloc`, over `http`'s types      ^^^^^^^^^^^^^ who instantiated it
#
# — `alloc`'s code, over `http`'s types, and tungstenite only because LLVM kept
# that copy when it deduplicated the identical instantiation every other crate
# also needs. Which copy survives is arbitrary and it differs by platform, which
# is exactly how this bit us: F4 linked `hyper`, `http`'s `HeaderMap` became
# live for the first time, and three such symbols appeared in the Linux archive
# and none in the Darwin one. The archive carried NO tungstenite code on either
# — `ar t` lists no member from it and every one of the three names begins
# `NtCs<..>_5alloc` — and the six Linux jobs went red anyway.
#
# The tag is the LAST thing on its line and a defining path never is: a path is
# a crate plus at least one item, so a crate that owns code in this archive is
# always followed by something. That is the whole of the test the two loops
# below make — `$crate` followed by a non-blank — and it is a position rather
# than a strip on purpose. Cutting the tag off with a regex was tried first and
# is a trap: `Cs<disambiguator>_` occurs at the START of a mangled path too, so
# leftmost-longest matching deletes the entire symbol and every list goes
# quiet — `present tokio` and `present hyper` both failed against an archive
# that plainly carries them.
#
# The one thing that IS stripped is `.llvm.<n>`, LLVM's own suffix on a symbol
# it internalised, which would otherwise sit behind the tag and put a character
# after it.
#
# `present` reads the same dump and makes the same positional test,
# deliberately: a crate whose only appearance was an instantiation tag would
# otherwise satisfy a claim that it is linked, which is the same confusion
# pointing the other way.
symbols=$(sed -E 's/\.llvm\.[0-9]+$//' <<<"$raw_symbols")

# A here-string rather than a pipe into `grep -q`: `set -o pipefail` and a grep
# that exits on its first match make the pipeline's status SIGPIPE, which reads
# as "no match" — so a piped version of these loops would report a clean archive
# precisely when the archive is not clean.
absent() {
  for crate in "$@"; do
    if grep -qiE -- "$crate[^[:space:]]" <<<"$symbols"; then
      echo "::error::libburi_rt.a carries symbols from \`$crate\`, and nothing in the runtime is supposed to reach it. Either something new does — in which case this list and the size budget above both move, in the commit that made it deliberate — or a crate was added to cli/runtime/manifest.toml without an argument for it."
      status=1
    fi
  done
}

present() {
  for crate in "$@"; do
    if ! grep -qiE -- "$crate[^[:space:]]" <<<"$symbols"; then
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
  present tokio rustls ring_core hyper tungstenite
  # `aws_lc` is here although it was never a dependency: it is the OTHER
  # provider `rustls` ships, and a second one appearing means a feature was
  # enabled somewhere that quietly doubled the cryptography in every binary.
  # `quinn` is the crate most likely to do it, and it is on this list on both
  # legs anyway because nothing calls into it yet.
  absent quinn aws_lc
  if [ "$status" -eq "$before" ]; then
    echo "libburi_rt.a: the reactor, TLS, HTTP/2 and WebSockets are linked, and the unreferenced crates are still unreferenced"
  fi
else
  absent tokio hyper rustls tungstenite quinn ring_core aws_lc
  if [ "$status" -eq "$before" ]; then
    echo "libburi_rt.a: built without \`net\`, and carries none of the networking crates"
  fi
fi

# THE `crypto` LEG, AND IT IS CHECKED BY SYMBOL RATHER THAN BY CRATE.
#
# The two loops above ask "does this archive carry code from crate X", which is
# the right question for the networking stack and the wrong one for this
# feature: `ring` depends on a `getrandom` of its own, so a `crypto`-off,
# `net`-on archive carries getrandom code either way and neither `present` nor
# `absent` could say anything true about it.
#
# What is unambiguous is the door. `buri_rt_host_entropy_bytes` is compiled only
# under `#[cfg(feature = "crypto")]`, it is `no_mangle`, and it is the one symbol
# the backends emit a call to for `Entropy` — so its presence and its absence are
# exactly the two claims the feature makes. An archive that declared `crypto`
# and did not export it would be a toolchain whose every `crypto.randomBytes`
# fails at the system linker; one that did not declare it and exported it anyway
# would be a compile-time refusal standing in front of a symbol that was there
# all along.
entropy_symbol=$(grep -c "buri_rt_host_entropy_bytes" <<<"$symbols" || true)
if grep -qx "crypto" "$features"; then
  if [ "$entropy_symbol" -eq 0 ]; then
    echo "::error::libburi_rt.a was built with the runtime's \`crypto\` feature and exports no \`buri_rt_host_entropy_bytes\`. Every program that calls \`crypto.randomBytes\` or \`crypto.token\` would fail at the system linker, and the compile-time refusal that exists for exactly this case would not fire, because the feature file says the archive has it."
    status=1
  else
    echo "libburi_rt.a: built with \`crypto\`, and the entropy door is in it"
  fi
elif [ "$entropy_symbol" -ne 0 ]; then
  echo "::error::libburi_rt.a exports \`buri_rt_host_entropy_bytes\` and its feature file does not name \`crypto\`. Either cli/build.rs wrote a sidecar for a different build, or the symbol escaped its \`#[cfg]\` — and a toolchain in that state refuses \`Entropy\` at compile time while carrying the body it refused."
  status=1
else
  echo "libburi_rt.a: built without \`crypto\`, and carries no entropy door"
fi

exit "$status"
