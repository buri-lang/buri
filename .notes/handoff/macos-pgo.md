# macOS PGO in the nightly

## State

PGO is back on the macOS nightly as two jobs. The next scheduled nightly tests it.

- **`pgo-train`** runs on `macos-latest`. It runs `sh .github/pgo.sh aarch64-apple-darwin train` and uploads `target/pgo/buri.profdata` as the artifact `profdata-aarch64-apple-darwin`.
- **`build`, macOS leg** runs on a fresh runner. It downloads that artifact into `target/pgo` and runs `sh .github/pgo.sh aarch64-apple-darwin use`, the release build with `-Cprofile-use`. Publish reads its tarball as before.
- **Linux legs** still run `pgo.sh <target>`, which defaults to step `all`. They now wait for `pgo-train`, because a matrix's legs share one `needs`.
- The macOS toolchain steps (rustup, Homebrew LLVM 21) moved to `.github/actions/macos-toolchain`, so both macOS jobs share them.
- `cli/tests/ci.rs`'s `the_release_build_keeps_its_measured_settings` checks the three `pgo.sh` calls, the upload and the download.

## What training holds on disk now

- The instrumented build keeps only its binary: its target directory is deleted before the suites build.
- Only the six training suites are built (`--test build --test native ...`), with `CARGO_PROFILE_TEST_DEBUG=0`.
- The suites and raw profiles are deleted after the merge, as before.

## Local measurement

`train` ran once on this Mac (M-series, 12 cores) with `CARGO_BUILD_JOBS=3`, three suites at once, no sccache and a cold `target/pgo`. It took 580 s.

- **Memory:** the largest single process was 2.9 GB (`/usr/bin/time -l`), the instrumented build's LTO link. The whole process tree, sampled every 10 s, peaked at 2.9 GB, and stayed under 1.1 GB during training.
- **Disk:** `target/pgo` peaked at 3.6 GB at the end of training: about 2.3 GB of suite binaries, the rest raw profiles and test output. The test temp directory held 18 MB.
- **On the runner:** add the toolchain, the cargo registry and Homebrew LLVM, a few GB. Both peaks leave room under 7 GB and 14 GB. The runner's 3 slower cores make it longer, not larger.

## If the split still loses the runner: the fallback

Not built. Linux trains and macOS only uses the profile:

1. Change the profile's source. Instead of `pgo-train`, an `ubuntu-24.04-arm` job runs `pgo.sh aarch64-unknown-linux-musl train` in Alpine and uploads the profile.
2. The macOS leg downloads it and runs `pgo.sh aarch64-apple-darwin use`.
3. Expect a weaker profile. Both are aarch64, but symbol names differ where code is `cfg`'d by OS, and functions without a match get no profile. rustc warns about them; turn the warning off with `-Cllvm-args=-pgo-warn-mismatch=false` if it is noisy.
4. `cli/tests/ci.rs` must then expect the training job on Linux.
