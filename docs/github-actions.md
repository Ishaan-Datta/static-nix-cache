# GitHub Actions

The repository provides `setup`, `save`, `restore`, and `deploy` composite
actions. `deploy` publishes only to GitHub Releases; the former server, local,
and S3 backends are not part of the Rust implementation.

## Standard Build

Run `setup` before building and `deploy` afterward:

```yaml
- uses: Ishaan-Datta/static-nix-cache/setup@v0.1.0
- run: nix build
- uses: Ishaan-Datta/static-nix-cache/deploy@v0.1.0
  with:
    github-token: ${{ github.token }}
    compression: zstd
    static: ./site
```

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
      - uses: actions/checkout@v4
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
      - uses: Ishaan-Datta/static-nix-cache/deploy@v0.1.0
        with:
          github-token: ${{ github.token }}
          compression: zstd
          static: ./site
```

## Deploy Inputs

| Input | Required | Default | Description |
|---|---:|---|---|
| `github-token` | yes | | Token with `contents: write` access to the destination repository |
| `paths-file` | no | | File containing store paths to publish |
| `export-dir` | no | | Existing `file://` binary cache used as the copy source |
| `snapshot-path` | no | `/tmp/static-nix-cache-setup/store-paths-before.txt` | Snapshot created by `setup` |
| `store-dir` | no | `/nix/store` | Logical and local Nix store directory |
| `artifact-pattern` | no | `static-nix-cache-*` | Saved artifact pattern |
| `github-owner` | no | current owner | Destination repository owner |
| `github-repo` | no | current repository | Destination repository name |
| `github-release-tag` | no | `nix-cache` | Release used for NAR storage |
| `signing-key` | no | | Nix secret signing key |
| `static` | no | | Static metadata output directory |
| `compression` | no | `none` | `none`, `xz`, or `zstd` |

Path discovery uses an explicit paths file, restored save artifacts, the setup
snapshot, and finally a running magic-nix-cache daemon, in that order.

The action downloads a checksum-verified `v0.1.0` Rust binary for Linux or
macOS on X64 or ARM64. This download is public and does not use the consumer's
GitHub token.
