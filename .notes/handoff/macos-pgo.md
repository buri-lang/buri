# macOS PGO in the nightly

## State

PGO is back on the macOS nightly as two jobs. The next scheduled nightly tests it.

- **`pgo-train`** runs on `macos-latest`. It runs `sh .github/pgo.sh aarch64-apple-darwin train` and uploads `target/pgo/buri.profdata` as the artifact `profdata-aarch64-apple-darwin`.
- **`build-macos`** needs `pgo-train` and runs on a fresh runner. It downloads that artifact into `target/pgo` and runs `sh .github/pgo.sh aarch64-apple-darwin use`, the release build with `-Cprofile-use`.
- **`build`** is now the two Linux legs only. They still run `pgo.sh <target>`, which defaults to step `all`, and start at once.
- **`publish`** needs `build` and `build-macos`.
- Shared steps live in composite actions: `.github/actions/macos-toolchain` (rustup, Homebrew LLVM 21) and `.github/actions/release-tarball` (smoke test, tarball, sidecar, upload).
- `cli/tests/ci.rs`'s `the_release_build_keeps_its_measured_settings` checks the three `pgo.sh` calls, the upload, the download and publish's `needs`.

## What training holds on disk now

- The instrumented build keeps only its binary: its target directory is deleted before the suites build.
- Only the six training suites are built (`--test build --test native ...`), with `CARGO_PROFILE_TEST_DEBUG=0`.
- The suites and raw profiles are deleted after the merge, as before.

## Local measurement

`train` ran once on this Mac (M-series, 12 cores) with `CARGO_BUILD_JOBS=3`, three suites at once, no sccache and a cold `target/pgo`. It took 580 s.

- **Memory:** the largest single process was 2.9 GB (`/usr/bin/time -l`), the instrumented build's LTO link. The whole process tree, sampled every 10 s, peaked at 2.9 GB, and stayed under 1.1 GB during training.
- **Disk:** `target/pgo` peaked at 3.6 GB at the end of training: about 2.3 GB of suite binaries, the rest raw profiles and test output. The test temp directory held 18 MB.
- **On the runner:** add the toolchain, the cargo registry and Homebrew LLVM, a few GB. Both peaks leave room under 7 GB and 14 GB. The runner's 3 slower cores make it longer, not larger.

## Why the single job lost the runner

The measured peaks fit 7 GB and 14 GB, so memory or disk alone doesn't explain losing the runner.

- The run included the instrumented build and the LLVM-linked suite builds, link peaks too. A 10 s sample can miss a short spike, but `time -l`'s 2.9 GB is the true single-process peak.
- The old script also kept the instrumented target directory while the suites built, and built every test target. That adds a few GB at most.
- Training output goes to `/dev/null`, so log volume is small.
- Run 42536342333 spent 23:42 to 00:38 in the PGO step. Locally, the instrumented build took 3 min and training about 5, on 12 fast cores. On 3 slow cores it plausibly was still training at 56 min.

The best explanation is CPU starvation during training. Three suites, two test threads each, and each test's `buri`, linker and program saturate 3 vCPUs for tens of minutes. The runner's listener then misses its heartbeats, and GitHub reports it lost, with no logs.

The split only partly addresses this. It gives training a fresh disk and moves the release build off it, but `pgo-train` trains as hard as before. If it is lost again, first try training below the runner's priority (`nice`) and with fewer threads, such as `--test-threads=1`, before the fallback.

## If the split still loses the runner: the fallback

Not built. Linux trains and macOS only uses the profile:

1. Change the profile's source. Instead of `pgo-train`, an `ubuntu-24.04-arm` job runs `pgo.sh aarch64-unknown-linux-musl train` in Alpine and uploads the profile.
2. The macOS leg downloads it and runs `pgo.sh aarch64-apple-darwin use`.
3. Expect a weaker profile. Both are aarch64, but symbol names differ where code is `cfg`'d by OS, and functions without a match get no profile. rustc warns about them; turn the warning off with `-Cllvm-args=-pgo-warn-mismatch=false` if it is noisy.
4. `cli/tests/ci.rs` must then expect the training job on Linux.
