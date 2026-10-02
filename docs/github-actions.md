# GitHub Actions

The repository provides `setup`, `save`, `restore`, and `deploy` composite
actions. `deploy` publishes only to GitHub Releases; the former server, local,
and S3 backends are not part of the Rust implementation.

## Standard Build

Run `setup` before building and `deploy` afterward:

```yaml
- uses: Ishaan-Datta/static-nix-cache/setup@v0.1.0
- run: nix build
- id: cache
  uses: Ishaan-Datta/static-nix-cache/deploy@v0.1.0
  with:
    compression: zstd
```

The generated static metadata directory is available as
`${{ steps.cache.outputs.static-path }}`.

## Deferred Deployment

Matrix jobs can use `save` to upload temporary workflow artifacts. A final
`deploy` job automatically downloads and aggregates matching artifacts:

```yaml
jobs:
  build:
    strategy:
      matrix:
        os: [ubuntu-latest, macos-latest]
    runs-on: ${{ matrix.os }}
    steps:
      - uses: actions/checkout@v6
      - uses: DeterminateSystems/nix-installer-action@main
      - uses: Ishaan-Datta/static-nix-cache/setup@v0.1.0
      - run: nix build
      - uses: Ishaan-Datta/static-nix-cache/save@v0.1.0
        with:
          name: ${{ matrix.os }}

  deploy:
    needs: build
    runs-on: ubuntu-latest
    steps:
      - id: cache
        uses: Ishaan-Datta/static-nix-cache/deploy@v0.1.0
        with:
          compression: zstd
```

## Deploy Inputs

| Input | Required | Default | Description |
|---|---:|---|---|
| `signing-key` | no | | Nix secret signing key |
| `compression` | no | `zstd` | `none`, `xz`, or `zstd` |

The action publishes to the current repository using `${{ github.token }}` and
the `nix-cache` release tag. Path discovery uses restored `save` artifacts, the
`setup` snapshot, and finally a running magic-nix-cache daemon, in that order.
The calling job must grant `contents: write`.

The action downloads a checksum-verified `v0.1.0` Rust binary for Linux or
macOS on X64 or ARM64. This download is public and does not use the consumer's
GitHub token.
