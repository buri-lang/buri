#!/usr/bin/env bash
#
# **The Linux legs of CI, on this machine, for no minutes.**
#
# `.github/workflows/ci.yml` is the workflow that says whether a change is good,
# and until now it was also the only thing that could say so: the two questions
# the Linux jobs answer — does the stencil backend build a real library under
# `clang`, and do the ELF artifacts it links actually run — cannot be asked on
# an arm64 mac at all. So every push asked a paid runner, and a branch that took
# six pushes to get right paid for six matrices. This script asks the same
# questions of a container on the developer's own machine, and leaves GitHub as
# the final check on `main` rather than as the inner loop.
#
# It is a MIRROR and not a second opinion. Every command below is copied from a
# `run:` line in `ci.yml`, in that file's order, with the same environment and
# the same assertion scripts out of `.github/scripts/`; where it departs from
# the workflow it says so in a comment and says why. The value of a local run is
# that a green one here predicts a green one there, and a mirror that improves
# on its subject predicts nothing.
#
# ## What it runs
#
# Two jobs, both of them Linux, both of them for one architecture at a time:
#
#   * **`test`** — the whole Rust suite, which is the leg named
#     `test (arm64, blacksmith-4vcpu-ubuntu-2404-arm)` in the workflow's matrix:
#     the tool assertions, `cargo build -p buri --tests`, the two liveness gates
#     over the bytes the build script wrote, the hoisted runtime-crate step,
#     `cargo test -p buri --no-fail-fast` through `assert-no-skips.sh`, clippy,
#     and the benchmark's `--validate --quick` gate.
#   * **`linux-arm64`** (or `linux-x86_64`) — the job that RUNS the artifacts:
#     the corpus census alone through `assert-suite-ran.sh`, the rest of
#     `stencil::` through `assert-no-skips.sh --allow-filtered`, leak parity,
#     `--check-reproducible`, and both linkers through `assert-elf.sh`.
#
# `--job test` or `--job programs` runs one of them; the default is both,
# because both is what a push buys and one of them is not a branch's answer.
#
# ## What it deliberately does NOT run
#
# **`nextest`.** There is none in this repository and there must not be one
# here. Two of the three liveness gates read libtest's own output —
# `assert-no-skips.sh` sums the `ignored` and `filtered out` counts off every
# `test result:` line, and `assert-suite-ran.sh` reads the census's `println!`
# out of a `--nocapture` log — and nextest prints neither shape. Swapping the
# runner would silently disarm the machinery this repository built to stop a
# suite from passing vacuously, which is the exact failure every one of those
# scripts exists to catch. The `Summary` line this script prints at the end is
# therefore computed from the libtest summaries, and it is the last thing on
# stdout for the same reason nextest puts one there.
#
# **The `net-h3` step.** The workflow guards it with
# `matrix.os == 'ubuntu-24.04'`, and no row of that matrix has `ubuntu-24.04`
# as its `os` — the three rows are `blacksmith-*` runners. The step therefore
# never fires on CI today, and a local run that fired it would be stricter than
# the thing it mirrors, would rebuild the runtime archive under `net-h3`, and
# would leave every step after it measuring a toolchain no CI job has. Running
# it here is a change to CI wearing a script's clothes; fixing the condition is
# a change to `ci.yml`, which is where it belongs.
#
# **`release`, `lean`, `tree-sitter`, `nix`, `minimal`, `language-server-budget`.**
# None of them is a Linux *test* job: they build a different feature set, a
# different language, or a different packaging, and each is cheap enough on CI
# that paying for it locally buys nothing.
#
# ## The pins, and why every one of them is exact
#
# A local run whose toolchain floats is a local run whose green means "it worked
# on whatever came down the wire today". So: the base image is an exact patch
# version of `rust:<version>-trixie` and never `latest`; node and bun are exact
# versions fetched from their own release hosts and checked against the digests
# those hosts publish beside them. The one thing that is not pinned is Debian's
# apt state, which is `trixie`'s frozen archive and moves only for security.
#
# **trixie and NOT bookworm, and that is measured rather than preferred.** The
# runner is Ubuntu noble, whose `clang` is 18; Debian trixie's is 19 and Debian
# bookworm's is 14. `clang` is not a detail of the environment here — it is the
# compiler that turns `stencil/sources.rs`'s generated C into the stencil
# library, so its version is part of the machine code this toolchain pastes
# together. bookworm was tried first, and under its clang 14 the natively linked
# `test-runner` for `cli/tests/failing/aborts` SPUN IN USER SPACE FOREVER: no
# syscalls, no signals, no page faults, `utime` climbing and `minflt` frozen at
# 60 for half an hour. That is a miscompile, and a mirror that reproduces one
# the runner does not have is worse than no mirror, because the first thing it
# costs is a day spent looking for the bug in the compiler under test. Four
# major versions was too far; one is close enough to be worth checking, and a
# divergence at that distance shows up as a clang diagnostic rather than as a
# silent one. An Ubuntu base with `rustup` bolted on would close the last
# version — and trade a pinned, reproducible image for a hand-rolled one.
#
# ## Where the state lives
#
# **Nothing is installed on the host but the podman machine.** The image, the
# cargo registry and the target directory all live inside it, the last two in
# named volumes (`buri-linux-cargo`, `buri-linux-target-<arch>`) so that the
# second run pays for the tests and not for the build.
#
# **The checkout is mounted READ-ONLY** at `/repo` and copied — `target/` and
# `.git` excluded — into a scratch tree inside the container, which is thrown
# away with it. A run therefore cannot write a file into the working tree, and
# `cli/tests/README.md`'s rule that nothing may write into a checked-in tree is
# enforced by the mount rather than by everyone remembering it. Forty-seven
# megabytes is a second and a half of `tar`; an overlay would save that second
# and cost a mount option that behaves differently on every storage driver.
#
# ## Usage
#
#   scripts/test-linux.sh                    # the arm64 Linux legs, both jobs
#   scripts/test-linux.sh --job test         # the suite only
#   scripts/test-linux.sh --job programs     # the artifacts-run job only
#   scripts/test-linux.sh --x86-64           # the same, under emulation (SLOW)
#
# `--x86-64` exists for one class of defect and it is worth its minutes when
# that class is in reach: the x86-only ABI — `asm.rs`'s SysV `main`, `jit.rs`'s
# `rel32` and rip-relative patches, `glue.rs`'s SysV stub — is proved on CI's
# x86_64 leg or nowhere, and two bugs in it have already reached `main`. It runs
# every instruction through qemu, so budget several times the arm64 wall clock
# and read the warning the script prints before it starts.
#
# **AND ON THIS MACHINE IT DOES NOT GET THAT FAR, WHICH IS MEASURED.** The
# emulated image builds, and clang, node and bun all run under it; `rustc -vV`
# does not — it takes SIGSEGV inside qemu-user 10.2.2 in about five seconds. So
# the flag preflights the compiler after the image build and stops with that
# sentence rather than with a segfault in the middle of a cargo invocation,
# which is a wrong diagnosis dressed as a broken checkout. The wiring is whole
# and a machine with a working x86 emulator runs it unchanged; until there is
# one, `linux-x86_64` is the single Linux job this script cannot take off CI,
# and it is the reason CI is still the final check rather than a formality.
#
# The exit status is the suite's. The last line of stdout is the summary.

set -euo pipefail

# --------------------------------------------------------------------- pins --
# The base image. An exact patch version: `dtolnay/rust-toolchain@stable` on CI
# resolves to whatever stable is on the day, and the way to mirror that honestly
# is to write down which day's stable this is and move it deliberately.
RUST_IMAGE="docker.io/library/rust:1.98.0-trixie"

# `oven-sh/setup-bun@v2` with no version takes the latest bun, and the runners
# ship a node. Both are pinned here for the reason above.
NODE_VERSION="24.20.0"
BUN_VERSION="1.4.0"

# The podman machine, sized for this laptop: eight of ten cores and half the
# memory, which leaves the host usable while the suite runs, and sixty gigabytes
# because a cold target directory for two profiles plus the registry is about
# twenty and a ratchet with no headroom fails on a Tuesday.
MACHINE_NAME="podman-machine-default"
MACHINE_CPUS=8
MACHINE_MEMORY=16384
MACHINE_DISK=60

# ---------------------------------------------------------------- arguments --
arch=arm64
job=all

usage() {
    sed -n '2,/^set -euo/p' "${BASH_SOURCE[0]}" | sed 's/^# \{0,1\}//; $d'
}

while [ $# -gt 0 ]; do
    case "$1" in
        --x86-64|--x86_64) arch=x86_64 ;;
        --job) shift; job=${1:-} ;;
        --job=*) job=${1#--job=} ;;
        -h|--help) usage; exit 0 ;;
        *) echo "test-linux.sh: unknown argument '$1' (try --help)" >&2; exit 2 ;;
    esac
    shift
done

case "$job" in
    all|test|programs) ;;
    *) echo "test-linux.sh: --job takes all, test or programs (got '$job')" >&2; exit 2 ;;
esac

case "$arch" in
    arm64)  platform=linux/arm64  ; debarch=arm64 ; nodearch=arm64   ; bunarch=aarch64 ;;
    x86_64) platform=linux/amd64  ; debarch=amd64 ; nodearch=x64     ; bunarch=x64     ;;
esac

repo=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
state="${XDG_CACHE_HOME:-$HOME/.cache}/buri-linux-tests"
mkdir -p "$state"

# An EMPTY registry auth file, passed to every podman call that touches a
# registry. Without it podman reads `~/.docker/config.json`, and a machine that
# has ever had Docker Desktop or `gcloud` on it has a `credsStore` line in there
# naming a helper binary that is no longer installed — at which point an
# anonymous pull from docker.io fails with `error getting credentials`, which
# reads like a network problem and is not one. Nothing here needs a credential;
# saying so explicitly is cheaper than asking the developer to edit a file that
# belongs to another tool.
authfile="$state/auth.json"
printf '{"auths":{}}' > "$authfile"

image="localhost/buri-linux-test:$arch"
vol_cargo="buri-linux-cargo"
vol_target="buri-linux-target-$arch"

say() { printf '\n\033[1m==> %s\033[0m\n' "$*"; }

# ---------------------------------------------------------- the podman machine --
command -v podman >/dev/null || {
    echo "test-linux.sh: podman is not on PATH. \`brew install podman\`." >&2
    exit 1
}

if ! podman machine inspect "$MACHINE_NAME" >/dev/null 2>&1; then
    say "No podman machine yet — creating one ($MACHINE_CPUS cpus, ${MACHINE_MEMORY}M, ${MACHINE_DISK}G)."
    echo "    This downloads a VM image and happens exactly once."
    podman machine init --cpus "$MACHINE_CPUS" --memory "$MACHINE_MEMORY" \
        --disk-size "$MACHINE_DISK" "$MACHINE_NAME"
fi

machine_state=$(podman machine inspect --format '{{.State}}' "$MACHINE_NAME")
if [ "$machine_state" != "running" ]; then
    say "The podman machine is $machine_state — starting it."
    podman machine start "$MACHINE_NAME"
else
    echo "podman machine $MACHINE_NAME: running"
fi

if [ "$arch" = x86_64 ]; then
    cat >&2 <<'WARNING'

  ┌──────────────────────────────────────────────────────────────────────────┐
  │  --x86-64 runs EVERY INSTRUCTION under qemu user-mode emulation.         │
  │                                                                          │
  │  Expect several times the arm64 wall clock — the cold build alone is     │
  │  hours rather than tens of minutes, and the corpus census is the         │
  │  longest test in the suite before it is emulated.                        │
  │                                                                          │
  │  It is worth it for one thing: the x86-only ABI (the SysV entry point,   │
  │  rel32 and rip-relative patching, the SysV glue stub) is proved on an    │
  │  x86 machine or nowhere. If that is not what changed, drop the flag.     │
  └──────────────────────────────────────────────────────────────────────────┘

WARNING
fi

# ------------------------------------------------------------------- the image --
#
# Built rather than assembled at run time, so that the second run pays nothing
# for apt, node or bun: podman's layer cache keys on this file's text, and this
# file's text is pinned. `--tests` and the suite are the only things that should
# cost minutes twice.
build_dir=$(mktemp -d)
trap 'rm -rf "$build_dir"' EXIT

cat > "$build_dir/Containerfile" <<CONTAINERFILE
FROM $RUST_IMAGE

# The five packages the workflow installs, and the reason each is there is
# argued in ci.yml's header: clang because \`cli/build.rs\` degrades to an empty
# stencil library under gcc; mold AND lld because \`build/link.rs::choose\`
# prefers mold and both are asserted on their own output; llvm for the
# unversioned llvm-nm/objdump/readelf that \`cross_tools\` probes; binutils for
# \`readelf\` itself. xz-utils and unzip unpack node and bun below.
RUN apt-get update \\
 && apt-get install -y --no-install-recommends \\
      clang lld mold llvm binutils xz-utils unzip ca-certificates curl \\
 && rm -rf /var/lib/apt/lists/*

# Debian installs the UNVERSIONED llvm tool names under /usr/lib/llvm-<N>/bin
# and only the versioned ones in /usr/bin. \`cross_tools\` probes the
# unversioned names, so the directory goes on PATH — highest version wins,
# exactly as the workflow's \`ls -d /usr/lib/llvm-*/bin | sort -V | tail -1\`
# does, rather than this file guessing a version.
# The directory is recorded rather than baked into ENV because \`ENV\` cannot
# read a file; the step runner prepends it.
RUN ls -d /usr/lib/llvm-*/bin | sort -V | tail -1 > /etc/llvm-bin-dir

# The official Rust images install rustup with the MINIMAL profile, which has no
# clippy in it. The workflow runs clippy on the Linux legs, so it is added here
# rather than discovered missing after the suite has already run.
RUN rustup component add clippy

# node and bun, at exact versions, verified against the digests their own
# release hosts publish. Compiled programs are executed with bun, and the
# constant-stack tail-call tests additionally run under node — JavaScriptCore
# has proper tail calls and V8 does not, which is why both are here.
RUN set -eux; \\
    cd /tmp; \\
    f="node-v$NODE_VERSION-linux-$nodearch.tar.xz"; \\
    curl -fsSLO "https://nodejs.org/dist/v$NODE_VERSION/\$f"; \\
    curl -fsSL "https://nodejs.org/dist/v$NODE_VERSION/SHASUMS256.txt" -o SHASUMS256.txt; \\
    grep " \$f\$" SHASUMS256.txt | sha256sum -c -; \\
    tar -xJf "\$f" -C /usr/local --strip-components=1 \\
        --exclude=CHANGELOG.md --exclude=LICENSE --exclude=README.md; \\
    rm -f "\$f" SHASUMS256.txt; \\
    node --version

RUN set -eux; \\
    cd /tmp; \\
    f="bun-linux-$bunarch.zip"; \\
    base="https://github.com/oven-sh/bun/releases/download/bun-v$BUN_VERSION"; \\
    curl -fsSLO "\$base/\$f"; \\
    curl -fsSL "\$base/SHASUMS256.txt" -o SHASUMS256.txt; \\
    grep " \$f\$" SHASUMS256.txt | sha256sum -c -; \\
    unzip -q "\$f"; \\
    install -m 0755 "bun-linux-$bunarch/bun" /usr/local/bin/bun; \\
    rm -rf "\$f" SHASUMS256.txt "bun-linux-$bunarch"; \\
    bun --version

# The workflow's env: block, verbatim. BURI_CI is the switch that makes
# \`harness/ci.rs::skipped\` PANIC instead of returning, so a guard that fires on
# a host this script has just equipped is a failure rather than a vacuous pass —
# and \`cli/tests/ci.rs\` asserts the workflow still carries it.
ENV CARGO_TERM_COLOR=always \\
    RUST_BACKTRACE=1 \\
    BURI_CI=1 \\
    BURI_RT_TESTS_STAMP=/work/target/runtime-crate-tests.ok \\
    CC=clang \\
    CARGO_HOME=/cargo
CONTAINERFILE

say "Building the test image ($image, $platform)."
podman build --authfile "$authfile" --platform "$platform" \
    -t "$image" -f "$build_dir/Containerfile" "$build_dir"

# Handed to the steps below so that the target volume can refuse to be reused
# by an image it was not built with.
#
# THE RECIPE'S DIGEST AND NOT THE IMAGE'S ID, which was the first thing tried
# and was wrong: an image rebuilt after `podman image prune` gets a new id from
# identical inputs, so the id would have thrown away a perfectly good target
# directory every time the image store was swept. The Containerfile's text is
# the thing that decides which clang and which glibc these artifacts were built
# by, so it is the thing the artifacts should be keyed on, and it changes
# exactly when a pin above changes.
recipe_hash=$(sha256sum < "$build_dir/Containerfile" | cut -d' ' -f1)

# ------------------------------------------- can this machine host the toolchain? --
#
# Asked HERE, of `rustc` rather than of `uname`, and both of those are
# corrections to an earlier guess.
#
# `uname -m` under emulation answers `x86_64` on any machine whose kernel has a
# binfmt handler registered, and this one does — the image above builds under
# emulation, apt runs, and node and bun both print their versions. What does NOT
# run is the compiler: on this podman machine (`qemu-user-static` 10.2.2, from
# the Fedora CoreOS machine image),
#
#     podman run --platform linux/amd64 <this image> rustc -vV
#
# dies with `qemu: uncaught target signal 11 (Segmentation fault)` in about five
# seconds, with or without `QEMU_STACK_SIZE`. So `--x86-64` cannot compile the
# toolchain today, and the honest thing is to say which of the two things is
# broken and stop — five seconds of SIGSEGV inside a cargo invocation reads as a
# broken checkout, and that is a wrong diagnosis to hand somebody.
#
# The flag is still here and still wired end to end, because the thing that
# fails is one program in the chain rather than the arrangement around it: a
# machine with a working x86 emulator, or a real x86 host, runs everything below
# unchanged. Until then the x86-only ABI stays what it was — CI's `linux-x86_64`
# job, which is exactly the one leg of the workflow this script cannot take
# over.
if [ "$arch" = x86_64 ]; then
    say "Checking that the emulator can host the Rust toolchain."
    # `--timeout`, because the failure has two shapes and only one of them
    # returns: the same probe segfaults in five seconds on one run and hangs
    # indefinitely on the next, and a preflight that can hang is worse than the
    # thing it was written to prevent. Three minutes is many times what a
    # working `rustc -vV` costs even under emulation.
    if ! podman run --rm --timeout 180 --authfile "$authfile" --platform "$platform" \
            "$image" rustc -vV >/dev/null 2>&1; then
        cat >&2 <<EOF

test-linux.sh: this podman machine's qemu cannot run the x86-64 Rust toolchain.

  \`rustc -vV\` under linux/amd64 emulation does not survive its own version
  probe — it segfaults, or it hangs — so nothing below it (cargo, the build
  script, the suite) can run. Everything else about the emulated image works:
  it builds, and clang, node and bun all report their versions under it. The
  compiler is the one program that does not.

  What this means: the x86-only ABI (\`asm.rs\`'s SysV entry point, \`jit.rs\`'s
  rel32 and rip-relative patches, \`glue.rs\`'s SysV stub) is still proved by
  CI's \`linux-x86_64\` job or on a real x86 machine, and not here. The arm64
  legs — which is to say every other Linux job in the workflow — do run here:

      scripts/test-linux.sh

EOF
        exit 1
    fi
fi

# ------------------------------------------------------------------ the volumes --
#
# The registry is shared across architectures — a `.crate` file is source and
# has no architecture — and the target directory is not, because everything in
# it is the host's. That split is the same one `Swatinem/rust-cache`'s
# `shared-key: ${{ matrix.os }}-${{ matrix.arch }}` makes on CI, and for the
# same reason its comment gives.
for v in "$vol_cargo" "$vol_target"; do
    podman volume exists "$v" || { podman volume create "$v" >/dev/null; echo "created volume $v"; }
done

# ------------------------------------------------------------------- the steps --
#
# Piped in on stdin rather than mounted or baked, so that editing this file
# changes the next run without invalidating an image layer, and so that the
# container needs no writable path on the host at all.
runner=$(cat <<'RUNNER'
set -uo pipefail

arch=$1
job=$2
recipe_hash=$3

export PATH="$(cat /etc/llvm-bin-dir):$PATH"

status=0
started=$(date +%s)

# The log the Summary is computed from. The `test` job's whole-suite run is the
# one that answers "how many tests are there and did they pass"; the `programs`
# job re-runs a NAMED SUBSET of it, so summing the two would count `stencil::`
# twice and report a number that is true of no invocation. Whichever job ran
# first therefore claims the summary, and the other's log lives beside it.
primary_log=
test_log=/work/suite-test.log
programs_log=/work/suite-programs.log

# The Summary, last, whatever happened — including a build failure, where the
# honest summary is that no binary reported one. `trap` rather than a line at
# the bottom, because half the steps below exit early on purpose. The verdict
# and the wall clock go ABOVE it, so that the Summary is the last line on
# stdout the way nextest's is.
summary() {
    local code=$?
    local secs=$(( $(date +%s) - started ))
    printf '\n'
    printf '%s  —  %s Linux, job %s, %dm %ds\n' \
        "$([ "$code" = 0 ] && echo PASSED || echo FAILED)" "$arch" "$job" \
        $(( secs / 60 )) $(( secs % 60 ))
    if [ -n "$primary_log" ] && [ -f "$primary_log" ] && grep -q '^test result:' "$primary_log"; then
        awk '
            /^test result:/ {
                bins++
                for (i = 1; i <= NF; i++) {
                    if ($i == "passed" || $i == "passed;")   passed   += $(i-1)
                    if ($i == "failed" || $i == "failed;")   failed   += $(i-1)
                    if ($i == "ignored" || $i == "ignored;") ignored  += $(i-1)
                    if ($i == "filtered")                    filtered += $(i-1)
                }
            }
            END {
                printf "Summary  %d tests run across %d binaries: %d passed, %d failed, %d ignored, %d filtered out\n",
                    passed + failed, bins, passed, failed, ignored, filtered
            }
        ' "$primary_log"
    else
        echo "Summary  no test binary reported a result — the run did not get as far as running tests"
    fi
    exit "$code"
}
trap summary EXIT

# A KILLED RUN IS NOT A PASSING RUN, and without this line it reported as one:
# `podman kill` sends SIGTERM to this shell, bash unwinds, and the EXIT trap
# above sees `$?` of zero and prints PASSED over a suite that never finished.
# That is the shape of false green everything in `.github/scripts/` exists to
# refuse, so it is refused here too — and it was found by killing a real run
# rather than by reasoning about one.
trap 'echo "the run was signalled before it finished" >&2; exit 143' TERM INT HUP

step() { printf '\n\033[1m--- %s\033[0m\n' "$*"; }
die()  { printf '\033[31merror: %s\033[0m\n' "$*" >&2; exit 1; }

# ------------------------------------------------------------------ the tree --
# Copied out of the read-only mount, with `target/` excluded because it is a
# volume mounted inside the destination and `.git` because nothing in the suite
# shells `git` and a worktree's `.git` is a pointer at a directory that is not
# on this machine.
step "Copying the checkout out of the read-only mount"
mkdir -p /work
tar -C /repo --exclude=./target --exclude=./.git -cf - . | tar -C /work -xf -
cd /work

# The target volume belongs to ONE image, and cargo cannot tell that it does
# not. Nothing in a fingerprint records which `clang` compiled the stencil
# library or which glibc the artifacts link against, so moving the pinned base
# image would otherwise leave a directory half-built by the old one and produce
# a failure whose cause is two edits back. The image id is therefore written
# beside the artifacts and compared; a mismatch empties the directory, which
# costs one cold build exactly when a cold build is the correct answer.
stamp=/work/target/.image
if [ ! -f "$stamp" ] || [ "$(cat "$stamp")" != "$recipe_hash" ]; then
    if [ -f "$stamp" ]; then
        echo "the target volume was built by a different image — emptying it"
    fi
    find /work/target -mindepth 1 -maxdepth 1 -exec rm -rf {} +
    printf '%s' "$recipe_hash" > "$stamp"
fi

# ------------------------------- ci.yml: "Every tool this workflow assumes" ----
# Asserted, not assumed. A missing tool otherwise presents as a test that
# skipped or as a link failure inside a suite, and both read as "green" or as
# "a compiler bug" rather than as "the image changed".
step "Every tool this workflow assumes"
for t in cc clang node bun; do
    command -v "$t" >/dev/null || die "$t is not on PATH"
done
for t in ld.lld mold llvm-nm llvm-objdump llvm-readelf readelf; do
    command -v "$t" >/dev/null || die "$t is not on PATH"
done
cc --version | head -1
node --version
bun --version
echo "arch: $(uname -m)"

if [ "$job" = all ] || [ "$job" = test ]; then
    # ================================================== ci.yml job: `test` ====
    step "cargo build -p buri --tests"
    cargo build -p buri --tests || exit 1

    step "The stencil libraries are not empty"
    bash .github/scripts/assert-stencils.sh || exit 1

    step "The runtime archive is not empty"
    bash .github/scripts/assert-runtime-archive.sh || exit 1

    step "The runtime crate answers its own tests"
    bash .github/scripts/test-runtime-crate.sh || exit 1

    # `--no-fail-fast` because the test binaries are domains, and one failing
    # domain should not hide the others. Through `tee`, because the assertion
    # after it reads the summary rather than the exit status.
    step "cargo test -p buri --no-fail-fast"
    set -o pipefail
    primary_log=$test_log
    cargo test -p buri --no-fail-fast 2>&1 | tee "$test_log" || status=1

    step "Nothing was skipped"
    bash .github/scripts/assert-no-skips.sh "$test_log" || status=1
    [ "$status" -eq 0 ] || exit "$status"

    # Deliberately NOT `-D warnings`, for the reason ci.yml states: the bar is
    # the panic-free lint set in the workspace manifest, every lint in it is
    # `deny`, and a violation therefore fails this step already.
    step "cargo clippy -p buri --all-targets"
    cargo clippy -p buri --all-targets || exit 1

    # The validation gate: compile every corpus and measure nothing.
    step "cargo bench -p buri --bench compiler -- --validate --quick"
    cargo bench -p buri --bench compiler -- --validate --quick || exit 1
fi

if [ "$job" = all ] || [ "$job" = programs ]; then
    # ==================== ci.yml job: `linux-arm64` / `linux-x86_64` ==========
    #
    # Compiling is what the job above proves. This one runs the artifacts.
    [ "$job" = programs ] && { step "cargo build -p buri --tests"; cargo build -p buri --tests || exit 1; }

    # FIRST, and alone. Every test in `cli/tests/native/stencil.rs` opens with
    # `if !supported() { return; }`, so a host with no stencils runs the whole
    # file, reports every test as PASSED, and proves nothing. The census's
    # `println!` sits on the far side of that guard and is the only thing that
    # can tell the two apart. `--test-threads=1` because `--nocapture` with the
    # default thread count lets two tests write to one line.
    step "The suite was live, and the corpus is at macOS parity"
    set -o pipefail
    cargo test -p buri --test native stencil::the_corpus_census_is_a_ratchet \
        -- --nocapture --test-threads=1 2>&1 | tee /work/census.log || exit 1
    bash .github/scripts/assert-suite-ran.sh /work/census.log || exit 1

    # The rest, in parallel, which is how it is written to run. `--skip` the
    # census: it just ran and it is the longest test in the file.
    step "The programs run"
    [ -n "$primary_log" ] || primary_log=$programs_log
    cargo test -p buri --test native stencil:: -- --nocapture \
        --skip the_corpus_census_is_a_ratchet 2>&1 | tee "$programs_log" || status=1

    # `--allow-filtered` because this invocation asked for a part on purpose.
    step "Nothing was skipped"
    bash .github/scripts/assert-no-skips.sh "$programs_log" --allow-filtered || status=1
    [ "$status" -eq 0 ] || exit "$status"

    # Leak parity: heap-stats and NOT valgrind. The COUNT is asserted rather
    # than the probe's output — the harness only prints the child's stderr when
    # an assertion fails, so `live=0` never reaches this log.
    step "Leak parity through buri_rt_heap_stats"
    cargo test -p buri --test native -- \
        stencil::nothing_is_leaked \
        stencil::the_glue_balances \
        stencil::interpolating_in_a_loop_leaks_nothing \
        2>&1 | tee /work/leaks.log || exit 1
    grep -qE 'test result: ok\. 3 passed' /work/leaks.log \
        || die "the three heap-stats tests did not all run"

    # Two builds from two freshly opened sessions with the cache off, into two
    # directories, compared byte for byte. A repository written here rather
    # than one of the checked-in ones, because both native binaries in
    # `cli/tests/example` are refused by the dev build.
    step "--check-reproducible on a linked Linux artifact"
    case "$arch" in
        arm64)  buri_arch=ARM64  ; out=linux/arm64  ;;
        x86_64) buri_arch=X86_64 ; out=linux/x86_64 ;;
    esac
    cargo build -p buri || exit 1
    bin=$PWD/target/debug/buri
    work=$(mktemp -d)
    mkdir -p "$work/repo/cmd/app"
    printf '# a repository with no tags\n' > "$work/repo/REPO.buri"
    cat > "$work/repo/cmd/app/BUILD.buri" <<BUILD
binary {
    outputs: [
        { platform: LINUX, arch: $buri_arch },
    ]
}
BUILD
    cat > "$work/repo/cmd/app/main.buri" <<'SRC'
from "core/host" import { stdout };
export fn main(): Result<(), Str> {
  let _ = stdout.println("reproducible");
  .Ok(())
}
SRC
    ( cd "$work/repo" && "$bin" build //cmd/app "--output=$out" --check-reproducible ) || exit 1

    # mold AND ld.lld. `BURI_LINKER` is the documented override, and the test
    # prints which one it used, so the assertion is on what happened rather
    # than on what was asked for. `target/tmp` is cleared first so the artifact
    # found afterwards is unambiguously this step's.
    step "Both linkers, and the image each produces"
    for linker in mold lld; do
        echo "--- BURI_LINKER=$linker"
        rm -rf target/tmp
        BURI_LINKER=$linker cargo test -p buri --test native \
            stencil::the_products_own_link -- --nocapture 2>&1 | tee "/work/link-$linker.log" || exit 1
        grep -q "linked with cc+$linker" "/work/link-$linker.log" \
            || die "BURI_LINKER=$linker did not reach link::choose — the test may have skipped"
        app=$(find target/tmp -path '*/product-link/app' -type f -print -quit 2>/dev/null || true)
        [ -n "$app" ] || die "the product's link left no artifact, so the suite skipped"
        bash .github/scripts/assert-elf.sh "$app" || exit 1
        # The other half of the stack check: a linker that had to guess about
        # the stack says so, and `build/link.rs` captures its stderr, so a
        # warning that did not fail the link would otherwise pass unread.
        if grep -qiE 'requires executable stack|missing \.note\.GNU-stack' "/work/link-$linker.log"; then
            die "$linker warned about an executable stack"
        fi
        [ "$("$app")" = "500500" ] || die "the artifact $linker produced printed the wrong answer"
    done

    if [ "$arch" = x86_64 ]; then
        # This host emits `linux-arm64` objects, links them with `ld.lld` and
        # checks every relocation resolves. Running that on a second host is
        # what says the emission does not depend on the machine it was made on.
        step "Cross emission and cross link, from x86-64"
        cargo test -p buri --test native -- --nocapture \
            stencil::linux_arm64_objects_link_and_every_relocation_resolves \
            stencil::a_cross_emission_is_reproducible \
            2>&1 | tee /work/cross.log || exit 1
        grep -qE 'test result: ok\. 2 passed' /work/cross.log \
            || die "the two cross tests did not both run — ld.lld or the llvm tools may be missing, which makes cross_tools() skip"
    fi
fi

exit "$status"
RUNNER
)

say "Running the Linux legs (arch $arch, job $job)."
# Nothing is printed after this container: it ends with the verdict and then
# the `Summary` line, and the Summary is meant to be the last thing on stdout.
run_status=0
podman run --rm -i \
    --authfile "$authfile" \
    --platform "$platform" \
    -v "$repo:/repo:ro" \
    -v "$vol_cargo:/cargo" \
    -v "$vol_target:/work/target" \
    "$image" \
    bash -s -- "$arch" "$job" "$recipe_hash" <<<"$runner" || run_status=$?

exit "$run_status"
