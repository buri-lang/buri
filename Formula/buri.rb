# This repository is its own Homebrew tap: `brew tap buri-lang/buri
# https://github.com/buri-lang/buri.git` makes `Formula/` the tap's formula
# directory, and `brew install buri-lang/buri/buri` installs what is below.
class Buri < Formula
  # The one owner/repo string in this file: change it here and the stable url,
  # the head spec, and the homepage all follow.
  REPO = "https://github.com/buri-lang/buri"

  desc "Buri toolchain: compiler, build system, test runner, formatter, and linter"
  homepage REPO

  # There is no release yet, so this names a tag that does not exist and the
  # sha256 is a placeholder. Until one is cut, the working install is
  # `brew install --HEAD buri-lang/buri/buri`, which uses the `head` spec below.
  #
  # Cutting a release, in the order it has to happen:
  #
  #   1. tag the commit `v<x.y.z>`, matching `version` in `cli/Cargo.toml` --
  #      that is what `buri version` prints, and a tag that disagrees with it
  #      makes `brew test` fail on exactly the mismatch it should.
  #   2. push the tag, so GitHub generates the archive this url points at.
  #   3. `brew fetch --formula ./Formula/buri.rb` and copy the SHA-256 it
  #      reports, or `curl -sL <url> | shasum -a 256`.
  #   4. paste it below, and bump the version in the url to match the tag.
  url "#{REPO}/archive/refs/tags/v0.3.0.tar.gz"
  sha256 "0000000000000000000000000000000000000000000000000000000000000000" # PLACEHOLDER: see above
  # The SPDX id Homebrew wants, matching the repository's own `LICENSE` and the
  # `license` field in `cli/Cargo.toml`.
  license "MIT"

  head "#{REPO}.git", branch: "main"

  depends_on "rust" => :build

  # No runtime dependency on a JavaScript runtime: `bun` is a development
  # tool, not something an install should carry. `buri test` compiles a suite
  # to a native binary and needs only `cc` to link it; where it falls back, and
  # for `buri run` on a binary that declares no output, a runtime is resolved
  # from `PATH` (or `BURI_JS`) when it is used.

  def install
    # `std_cargo_args` supplies `--locked` and `--root #{prefix}`. The lockfile
    # is the build: the toolchain has no dependencies (see the workspace
    # `Cargo.toml`), so `--locked` resolves nothing and reaches nothing.
    system "cargo", "install", *std_cargo_args(path: "cli")
  end

  test do
    # Outside a Buri repository `buri version` prints one line and exits 0 --
    # there is no pin to report and that is not an error. Matching the shape
    # rather than a literal because a `--HEAD` build and a tagged one differ
    # here and both must pass.
    assert_match(/^buri \d+\.\d+\.\d+$/, shell_output("#{bin}/buri version"))

    # The documentation is compiled into the binary and served without a
    # checkout, so this runs anywhere and fails loudly if the topics did not
    # make it into the build.
    assert_match "guide/goals", shell_output("#{bin}/buri docs")
  end
end
