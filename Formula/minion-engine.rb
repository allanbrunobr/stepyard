# Homebrew formula for Minion Engine.
#
# Pulls pre-compiled binaries produced by `.github/workflows/release.yml` on
# each `v*` tag. The SHA256 placeholders below must be replaced per release
# with values from the `checksums.sha256` artifact attached to the GitHub
# Release — the release.yml workflow generates this file automatically.
#
# To publish via a custom tap:
#   brew tap allanbrunobr/minion-engine https://github.com/allanbrunobr/homebrew-minion-engine
#   brew install minion-engine
#
# To test locally without a tap:
#   brew install --build-from-source ./Formula/minion-engine.rb

class MinionEngine < Formula
  desc "AI workflow engine that orchestrates Claude Code CLI"
  homepage "https://github.com/allanbrunobr/minion-engine"
  version "0.7.6"
  license "MIT"

  on_macos do
    on_arm do
      url "https://github.com/allanbrunobr/minion-engine/releases/download/v#{version}/minion-macos-aarch64"
      sha256 "REPLACE_WITH_MACOS_AARCH64_SHA256"
    end
    on_intel do
      url "https://github.com/allanbrunobr/minion-engine/releases/download/v#{version}/minion-macos-x86_64"
      sha256 "REPLACE_WITH_MACOS_X86_64_SHA256"
    end
  end

  on_linux do
    on_arm do
      url "https://github.com/allanbrunobr/minion-engine/releases/download/v#{version}/minion-linux-aarch64"
      sha256 "REPLACE_WITH_LINUX_AARCH64_SHA256"
    end
    on_intel do
      url "https://github.com/allanbrunobr/minion-engine/releases/download/v#{version}/minion-linux-x86_64"
      sha256 "REPLACE_WITH_LINUX_X86_64_SHA256"
    end
  end

  def install
    # The release asset is the raw binary named like `minion-<platform>`.
    # Install it as `minion` in the Homebrew bin directory.
    binary = Dir["minion-*"].first
    odie "Expected a single pre-compiled minion-* binary in the download" if binary.nil?
    bin.install binary => "minion"
  end

  def caveats
    <<~EOS
      Minion Engine runs workflows in a Docker container by default.
      Docker Desktop 4.40+ is recommended for the built-in sandbox.
      Pass `--no-sandbox` to run on the host instead.

      To use AI steps (agent/chat), export your Anthropic API key:
        export ANTHROPIC_API_KEY="sk-ant-..."
    EOS
  end

  test do
    assert_match version.to_s, shell_output("#{bin}/minion --version")
  end
end
