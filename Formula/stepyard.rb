# Homebrew formula for Stepyard.
#
# NOT PUBLISHED YET. There is no tap and no GitHub Release with binaries.
# The formula pulls the pre-compiled binaries that
# `.github/workflows/release.yml` attaches to each `v*` tag. Before you
# publish it:
#   1. Push a `v<version>` tag that matches `version` below and let the
#      Release workflow succeed.
#   2. Replace each SHA256 placeholder with the value from the
#      `checksums.sha256` asset of that release.
#   3. Copy this file into a tap repository, for example
#      https://github.com/allanbrunobr/homebrew-stepyard, then:
#        brew tap allanbrunobr/stepyard
#        brew install stepyard
#
# Local test without a tap:
#   brew install --build-from-source ./Formula/stepyard.rb

class Stepyard < Formula
  desc "Deterministic YAML workflows that orchestrate AI coding agents"
  homepage "https://github.com/allanbrunobr/stepyard"
  version "0.7.6"
  license "MIT"

  on_macos do
    on_arm do
      url "https://github.com/allanbrunobr/stepyard/releases/download/v#{version}/stepyard-macos-aarch64"
      sha256 "REPLACE_WITH_MACOS_AARCH64_SHA256"
    end
    on_intel do
      url "https://github.com/allanbrunobr/stepyard/releases/download/v#{version}/stepyard-macos-x86_64"
      sha256 "REPLACE_WITH_MACOS_X86_64_SHA256"
    end
  end

  on_linux do
    on_arm do
      url "https://github.com/allanbrunobr/stepyard/releases/download/v#{version}/stepyard-linux-aarch64"
      sha256 "REPLACE_WITH_LINUX_AARCH64_SHA256"
    end
    on_intel do
      url "https://github.com/allanbrunobr/stepyard/releases/download/v#{version}/stepyard-linux-x86_64"
      sha256 "REPLACE_WITH_LINUX_X86_64_SHA256"
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
      The release binaries use the PostgreSQL session store. Export
      DATABASE_URL before you run a workflow:
        export DATABASE_URL="postgres://user:password@localhost:5432/stepyard"

      Workflows run in a Docker container by default. Pass `--no-sandbox`
      to run on the host instead.

      AI steps need a provider key, for example:
        export ANTHROPIC_API_KEY="sk-ant-..."
    EOS
  end

  test do
    assert_match version.to_s, shell_output("#{bin}/stepyard --version")
  end
end
