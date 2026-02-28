class OpencodeTmuxMem < Formula
  desc "Inspect OpenCode memory and map PIDs to tmux panes"
  homepage "https://github.com/dmoliveira/opencode-tmux-mem"
  url "https://github.com/dmoliveira/opencode-tmux-mem/archive/refs/tags/v0.1.0.tar.gz"
  sha256 "REPLACE_WITH_RELEASE_TARBALL_SHA256"
  license "MIT"

  depends_on "rust" => :build

  def install
    system "cargo", "install", *std_cargo_args(path: "./")
  end

  test do
    assert_match "Usage:", shell_output("#{bin}/opencode-tmux-mem --help")
  end
end
