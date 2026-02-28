# Homebrew release guide 🍺

This repo keeps a formula template in `Formula/opencode-tmux-mem.rb`.

Your public tap is `dmoliveira/homebrew-tap`, exposed as `dmoliveira/tap` in Homebrew.

## 1) Create a tagged release in this repo

```bash
git tag v0.1.0
git push origin v0.1.0
gh release create v0.1.0 --generate-notes
```

## 2) Compute source tarball SHA256

```bash
curl -fsSL "https://github.com/dmoliveira/opencode-tmux-mem/archive/refs/tags/v0.1.0.tar.gz" -o /tmp/opencode-tmux-mem-v0.1.0.tar.gz
shasum -a 256 /tmp/opencode-tmux-mem-v0.1.0.tar.gz
```

## 3) Update formula in tap repo

In your `homebrew-tap` repository, place/update:

- `Formula/opencode-tmux-mem.rb`

Set:

- `url` to the new tag archive
- `sha256` to the computed hash

## 4) Validate and publish

```bash
brew audit --strict --online Formula/opencode-tmux-mem.rb
brew install --build-from-source Formula/opencode-tmux-mem.rb
git add Formula/opencode-tmux-mem.rb
git commit -m "opencode-tmux-mem v0.1.0"
git push
```
