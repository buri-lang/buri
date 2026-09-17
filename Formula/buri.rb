# This repository is its own Homebrew tap: `brew tap buri-lang/buri
# https://github.com/buri-lang/buri.git` makes `Formula/` the tap's formula
# directory, and `brew install buri-lang/buri/buri` installs what is below.
#
# The stable install downloads a PREBUILT `buri` from the matching GitHub
# release — no compile, no LLVM build dependency, and on Linux no musl caveat:
# the published binary is already a static-PIE musl executable that runs on any
# Linux. `brew install --HEAD` still builds from source (the `head do` block).
#
# There is no aarch64/x86_64 macOS split — the only macOS target is arm64.
#
# The version, urls, and sha256s are rewritten each night by
# .github/workflows/nightly.yml; every field it edits carries a `# nightly:...`
# marker so the rewrite is a deterministic sed. The values here are placeholders
# until the first release is cut.
class Buri < Formula
  # The one owner/repo string in this file: change it here and the release urls,
  # the head spec, and the homepage all follow.
  REPO = "https://github.com/buri-lang/buri"

  desc "Buri toolchain: compiler, build system, test runner, formatter, and linter"
  homepage REPO
  license "MIT"

  version "0.3.2" # nightly:version

  on_macos do
    on_arm do
      url "#{REPO}/releases/download/v#{version}/buri-#{version}-aarch64-apple-darwin.tar.gz"
      sha256 "d011553c4a180feca3d56e06134f33bca9782e8098a0a3bc36aa992ee1b4fb61" # nightly:sha256:aarch64-apple-darwin
    end
  end

  on_linux do
    on_arm do
      url "#{REPO}/releases/download/v#{version}/buri-#{version}-aarch64-unknown-linux-musl.tar.gz"
      sha256 "d37e1098fc7ed9abba1734ad0c4a839cf15d3ea5bd5f6b8010a5fcafaa010094" # nightly:sha256:aarch64-unknown-linux-musl
    end
    on_intel do
      url "#{REPO}/releases/download/v#{version}/buri-#{version}-x86_64-unknown-linux-musl.tar.gz"
      sha256 "241894b3f318c1ecc8a7868b10af5fd3faa5aabc9bae29b2b99fb8b2e1ac30fe" # nightly:sha256:x86_64-unknown-linux-musl
    end
  end

  # The from-source escape hatch: `brew install --HEAD buri-lang/buri/buri`.
  # Building `--release` artifacts needs the optimizing backend, which needs LLVM
  # 21 exactly — cli/Cargo.toml pins the `llvm21-1` inkwell feature, and the
  # unversioned `llvm` is already past 21, so it would not satisfy it. `llvm@21`
  # is keg-only, which is why `install` reaches it by `opt_prefix` below rather
  # than trusting it on PATH. These deps are the HEAD build's alone: the prebuilt
  # path needs neither rust nor LLVM.
  head do
    url "#{REPO}.git", branch: "main"
    depends_on "rust" => :build
    depends_on "llvm@21" => :build
  end

  def install
    if build.head?
      # `llvm-sys` refuses to guess where LLVM is; point it at the keg-only
      # `llvm@21`'s prefix. `std_cargo_args` supplies `--locked` and `--root
      # #{prefix}`, and `--features backend-llvm` ships the optimizing backend.
      ENV["LLVM_SYS_211_PREFIX"] = Formula["llvm@21"].opt_prefix
      system "cargo", "install", "--features", "backend-llvm", *std_cargo_args(path: "cli")
    else
      # The prebuilt tarball is a single `buri` binary; install it as-is.
      bin.install "buri"
    end
  end

  test do
    # Outside a Buri repository `buri version` prints one line and exits 0.
    # Matching the shape rather than a literal because a `--HEAD` build and a
    # tagged one differ here and both must pass.
    assert_match(/^buri \d+\.\d+\.\d+$/, shell_output("#{bin}/buri version"))

    # The documentation is compiled into the binary and served without a
    # checkout, so this runs anywhere and fails loudly if the topics did not make
    # it into the build.
    assert_match "guide/goals", shell_output("#{bin}/buri docs")
  end
end
