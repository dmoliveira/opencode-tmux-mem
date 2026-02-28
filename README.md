# opencode-tmux-mem 🔍

Friendly tiny CLI to inspect OpenCode memory usage and map each process back to its tmux pane.

It answers questions like:

- Which `opencode` process is swapping the most? 🧠
- Which tmux `window.pane` owns that PID? 🪟
- How much pane history text is currently retained? 📜

## Why this exists

When many OpenCode sessions run in tmux, memory pressure can rise fast. This tool gives a single, readable report sorted by highest swap first, so cleanup decisions are obvious.

## Tech stack ⚙️

- **Language:** Rust (stable)
- **System tools used:** `pgrep`, `ps`, `vmmap`, `tmux`
- **Design:** no runtime dependencies, single binary, simple CLI

## Install

### Option 1: Build from source

```bash
git clone https://github.com/dmoliveira/opencode-tmux-mem.git
cd opencode-tmux-mem
cargo build --release
./target/release/opencode-tmux-mem --help
```

### Option 2: Homebrew tap (recommended for macOS) 🍺

```bash
brew tap dmoliveira/tap
brew install opencode-tmux-mem
```

Formula is published in `dmoliveira/tap` and built from `v0.1.0`.

## Usage

Default output is a human-readable table:

```bash
opencode-tmux-mem
```

Useful flags:

```bash
# Match only command name == opencode (default)
opencode-tmux-mem --match-mode exact

# Match full command lines (e.g. opencode --continue)
opencode-tmux-mem --match-mode full --process opencode

# Export as JSON/CSV/YAML/Markdown
opencode-tmux-mem --export report.json
opencode-tmux-mem --format markdown --export report.md

# Faster run: skip pane capture for history byte estimation
opencode-tmux-mem --no-history-bytes
```

## Output fields

- `PID`: process id
- `Tmux window.pane`: tmux owner, like `ai:6.0`
- `Swap`: swapped bytes (human-readable)
- `Physical`: physical footprint
- `RSS`: resident memory from `ps`
- `PaneHistory`: captured history text bytes (lower-bound estimate)

## Testing ✅

```bash
cargo test
cargo clippy -- -D warnings
```

## Notes

- `vmmap` and tmux inspection are macOS/tmux oriented.
- If tmux is unavailable, process memory still works (pane mapping becomes `?`).
- History text bytes are practical estimates, not tmux internal memory accounting.

## License

MIT
