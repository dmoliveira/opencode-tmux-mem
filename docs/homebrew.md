# Homebrew release guide 🍺

This repo keeps a formula template in `Formula/opencode-tmux-mem.rb`.

Your public tap is `dmoliveira/homebrew-tap`, exposed as `dmoliveira/tap` in Homebrew.

## 1) Create a tagged release in this repo

```bash
VERSION=v0.1.1
git tag "$VERSION"
git push origin "$VERSION"
gh release create "$VERSION" --generate-notes
```

## 2) Compute source tarball SHA256

```bash
VERSION=v0.1.1
curl -fsSL "https://github.com/dmoliveira/opencode-tmux-mem/archive/refs/tags/${VERSION}.tar.gz" -o "/tmp/opencode-tmux-mem-${VERSION}.tar.gz"
shasum -a 256 "/tmp/opencode-tmux-mem-${VERSION}.tar.gz"
```

## 3) Update formula in tap repo

In your `homebrew-tap` repository, place/update:

- `Formula/opencode-tmux-mem.rb`

Set:

- `url` to the new tag archive
- `sha256` to the computed hash

## 4) Validate and publish

```bash
brew audit --strict --online dmoliveira/tap/opencode-tmux-mem
brew install --build-from-source dmoliveira/tap/opencode-tmux-mem
git add Formula/opencode-tmux-mem.rb
git commit -m "opencode-tmux-mem ${VERSION}"
git push
```
