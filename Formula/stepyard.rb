# Homebrew formula for Stepyard.
#
# Release v0.7.6 exists and publishes the SQLite binaries. This formula is
# not in a Homebrew tap, so `brew install stepyard` does not work from this
# file alone. The sha256 values are the hashes of those four release assets,
# measured on 2026-11-09 from the public download URLs.
#
# To publish it, copy this file into a tap repository, for example
# https://github.com/allanbrunobr/homebrew-stepyard, then:
#   brew tap allanbrunobr/stepyard
#   brew install stepyard

class Stepyard < Formula
  desc "Deterministic YAML workflows that orchestrate AI coding agents"
  homepage "https://github.com/allanbrunobr/stepyard"
  version "0.7.6"
  license "MIT"

  on_macos do
    on_arm do
      url "https://github.com/allanbrunobr/stepyard/releases/download/v#{version}/stepyard-macos-aarch64"
      sha256 "7314200a975734db711b2c129dd18278ec7221efca7b140d89a75f09f3dbd40e"
    end
    on_intel do
      url "https://github.com/allanbrunobr/stepyard/releases/download/v#{version}/stepyard-macos-x86_64"
      sha256 "8ba2c949cd99642756b595e10c236eb42ace9421a237e742adaaedfc1fc59119"
    end
  end

  on_linux do
    on_arm do
      url "https://github.com/allanbrunobr/stepyard/releases/download/v#{version}/stepyard-linux-aarch64"
      sha256 "630fa13e6bb71af79a747d98681bef416bdfc398377ff63d46219e83d812a755"
    end
    on_intel do
      url "https://github.com/allanbrunobr/stepyard/releases/download/v#{version}/stepyard-linux-x86_64"
      sha256 "0540cbc73fc0ecffcef62784ec8eff6bf5a87639ec45d8cf26466ea1dd32335f"
    end
  end

  def install
    # The release asset is the raw binary named `stepyard-<platform>`.
    binary = Dir["stepyard-*"].first
    odie "Expected a single pre-compiled stepyard-* binary in the download" if binary.nil?
    bin.install binary => "stepyard"
  end

  def caveats
    <<~EOS
      This bottle is the SQLite release build. Sessions are stored in
      .stepyard/sessions.db. This binary does not use DATABASE_URL.

      Steps run on the host by default. To use a container, pass
      `--sandbox-runtime docker` or `--sandbox-runtime podman`.

      PostgreSQL (needed for `stepyard session` and the dashboard) is a
      source install:
        cargo install --locked --git https://github.com/allanbrunobr/stepyard

      AI steps need a provider key, for example:
        export ANTHROPIC_API_KEY="sk-ant-..."
    EOS
  end

  test do
    assert_match version.to_s, shell_output("#{bin}/stepyard --version")
  end
end
