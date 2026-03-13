# typed: false
# frozen_string_literal: true

# Homebrew formula for minion-engine.
#
# To use this tap:
#   brew tap allanbrunobr/minion-engine https://github.com/allanbrunobr/minion-engine
#   brew install allanbrunobr/minion-engine/minion-engine
#
# Or install directly from the tap shorthand once published:
#   brew install minion-engine
class MinionEngine < Formula
  desc "AI workflow engine that orchestrates Claude Code CLI"
  homepage "https://github.com/allanbrunobr/minion-engine"
  version "0.1.0"
  license "MIT"

  # Platform-specific pre-compiled bottle URLs
  # These are updated automatically by the release workflow after each tag.
  on_macos do
    if Hardware::CPU.arm?
      url "https://github.com/allanbrunobr/minion-engine/releases/download/v#{version}/minion-macos-aarch64"
      sha256 "0000000000000000000000000000000000000000000000000000000000000000" # updated on release
    else
      url "https://github.com/allanbrunobr/minion-engine/releases/download/v#{version}/minion-macos-x86_64"
      sha256 "0000000000000000000000000000000000000000000000000000000000000000" # updated on release
    end
  end

  on_linux do
    if Hardware::CPU.arm?
      url "https://github.com/allanbrunobr/minion-engine/releases/download/v#{version}/minion-linux-aarch64"
      sha256 "0000000000000000000000000000000000000000000000000000000000000000" # updated on release
    else
      url "https://github.com/allanbrunobr/minion-engine/releases/download/v#{version}/minion-linux-x86_64"
      sha256 "0000000000000000000000000000000000000000000000000000000000000000" # updated on release
    end
  end

  def install
    # The downloaded artifact is the pre-compiled binary — install it directly.
    binary = Dir["minion-*"].first || "minion"
    bin.install binary => "minion"
  end

  # Optional: build from source as a fallback for unsupported platforms.
  # Homebrew will use this when no bottle is available.
  head do
    url "https://github.com/allanbrunobr/minion-engine.git", branch: "main"
    depends_on "rust" => :build

    def install
      system "cargo", "install", *std_cargo_args
    end
  end

  test do
    # Verify the binary is installed and responds to --version
    assert_match version.to_s, shell_output("#{bin}/minion --version")
  end
end
