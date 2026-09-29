# static-nix-cache

Publish Nix binary caches to GitHub Releases and serve their metadata from any
static host. No cache server runs in CI or production.

## How It Works

1. Nix serializes store paths into NARs, applies the selected compression, and
   writes matching `.narinfo` metadata into a local `file://` cache.
2. The Rust publisher uploads NARs and durable narinfo metadata to a GitHub
   Release.
3. The publisher optionally writes a static cache index whose NAR URLs point
   directly at the release assets.

Compression remains a Nix responsibility so `Compression`, `FileHash`, and
`FileSize` always describe the exact uploaded bytes.

## GitHub Action

```yaml
name: Deploy Cache

on:
  push:
    branches: [main]

permissions:
  contents: write
  pages: write
  id-token: write

jobs:
  cache:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - uses: DeterminateSystems/nix-installer-action@main
      - uses: Ishaan-Datta/static-nix-cache/setup@v0.1.0

      - run: nix build

      - uses: Ishaan-Datta/static-nix-cache/deploy@v0.1.0
        with:
          github-token: ${{ github.token }}
          compression: zstd
          static: ./site

      - uses: actions/upload-pages-artifact@v3
        with:
          path: ./site

      - uses: actions/deploy-pages@v4
```

The deploy action downloads the matching Rust binary from the public `v0.1.0`
release, verifies its checksum, and supports Linux and macOS on x86_64 and
ARM64.

## Command Line

```bash
nix copy --to 'file:///tmp/publish-cache?compression=zstd' /nix/store/...

GITHUB_TOKEN=... static-nix-cache publish \
  --cache-dir /tmp/publish-cache \
  --owner example \
  --repo project \
  --output-dir ./site
```

Omit `--output-dir` when only release publication is needed. See
[`docs/`](docs/) for action inputs, configuration, and static hosting details.

## Development

```bash
nix develop
cargo test --locked
nix build
```

## License

[MIT](LICENSE)
