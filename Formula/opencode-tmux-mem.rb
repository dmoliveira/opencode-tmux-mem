class OpencodeTmuxMem < Formula
  desc "Inspect OpenCode memory and map PIDs to tmux panes"
  homepage "https://github.com/dmoliveira/opencode-tmux-mem"
  url "https://github.com/dmoliveira/opencode-tmux-mem/archive/refs/tags/v0.1.0.tar.gz"
  sha256 "437ee5d9bc9d79063ad1334aef31474a41814dcfb43a14d524a5e2a87e57b213"
  license "MIT"

  depends_on "rust" => :build

  def install
    system "cargo", "install", *std_cargo_args(path: "./")
  end

  test do
    assert_match "Usage:", shell_output("#{bin}/opencode-tmux-mem --help")
  end
end
