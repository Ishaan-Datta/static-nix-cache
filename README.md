# static-nix-cache

Publish Nix build results as a binary cache backed by GitHub Releases and
GitHub Pages. NAR files live in a release while Pages serves the small
`nix-cache-info` and `.narinfo` files Nix needs to find them. No cache server is
required.

## Deploy From GitHub Actions

Enable GitHub Pages with **Source: GitHub Actions**, then add this workflow:

```yaml
name: Deploy Nix cache

on:
  push:
    branches: [main]

permissions:
  contents: write
  pages: write
  id-token: write

concurrency:
  group: nix-cache
  cancel-in-progress: false

jobs:
  cache:
    runs-on: ubuntu-latest
    environment:
      name: github-pages
      url: ${{ steps.cache.outputs.page-url }}
    steps:
      - uses: actions/checkout@v6
      - uses: DeterminateSystems/nix-installer-action@main
      - run: nix build
      - id: cache
        uses: Ishaan-Datta/static-nix-cache/deploy@v0.1.0
```

That final action is the complete publishing pipeline. It:

1. Finds the `result` and `result-*` links created by `nix build`.
2. Copies their closures into a compressed local binary cache.
3. Publishes the NARs and durable metadata to the repository's `nix-cache`
   GitHub Release.
4. Generates the static cache index, uploads it as a Pages artifact, and
   deploys it.

The permissions must remain in the workflow because a GitHub Action cannot
grant permissions to itself. Use `paths` when the build does not leave standard
result links:

```yaml
- run: nix build .#package -o package-result
- uses: Ishaan-Datta/static-nix-cache/deploy@v0.1.0
  with:
    paths: package-result
```

Multiple paths can be provided one per line.

## Use The Cache

The cache URL is the repository's Pages URL:

```text
https://<owner>.github.io/<repo>/
```

Add it to a flake:

```nix
{
  nixConfig.extra-substituters = [ "https://<owner>.github.io/<repo>/" ];
}
```

For a signed cache, store the private key in a GitHub Actions secret and pass
it to the action:

```yaml
- id: cache
  uses: Ishaan-Datta/static-nix-cache/deploy@v0.1.0
  with:
    signing-key: ${{ secrets.NIX_CACHE_SIGNING_KEY }}
```

Consumers must add the corresponding public key to
`extra-trusted-public-keys`.

## Action Options

| Input | Default | Description |
| --- | --- | --- |
| `paths` | `result`, `result-*` | Newline-separated build result paths |
| `compression` | `zstd` | NAR compression: `none`, `xz`, or `zstd` |
| `signing-key` | | Nix secret key in `keyname:base64-ed25519-private` format |
| `deploy-pages` | `true` | Set to `false` to publish without deploying Pages |

The action outputs `page-url` and `static-path`. When `deploy-pages` is false,
`static-path` can be uploaded to any static host without modification.

The action supports Linux and macOS runners on x86_64 and ARM64. It downloads
the matching checksum-verified publisher binary and requires Nix to already be
installed.

## Matrix Builds

Use `save` in each build job. A final `deploy` automatically combines those
temporary artifacts and publishes them:

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
      - run: nix build
      - uses: Ishaan-Datta/static-nix-cache/save@v0.1.0
        with:
          name: ${{ matrix.os }}

  deploy:
    needs: build
    runs-on: ubuntu-latest
    permissions:
      contents: write
      pages: write
      id-token: write
    environment:
      name: github-pages
      url: ${{ steps.cache.outputs.page-url }}
    steps:
      - uses: DeterminateSystems/nix-installer-action@main
      - id: cache
        uses: Ishaan-Datta/static-nix-cache/deploy@v0.1.0
```

Set workflow concurrency as shown in the single-job example so two runs do not
update the same release simultaneously.

## Command Line

The publisher can also publish an existing local file cache:

```bash
nix copy --to 'file:///tmp/publish-cache?compression=zstd' /nix/store/...

GITHUB_TOKEN=... static-nix-cache publish \
  --cache-dir /tmp/publish-cache \
  --owner example \
  --repo project \
  --output-dir ./site
```

| Option | Environment | Default | Description |
| --- | --- | --- | --- |
| `--cache-dir` | | required | Cache produced by `nix copy --to file://...` |
| `--output-dir` | | | Static metadata output directory |
| `--owner` | `GITHUB_OWNER` | repository context | Destination owner |
| `--repo` | `GITHUB_REPO` | repository context | Destination repository |
| `--token` | `GITHUB_TOKEN` | required | Token with `contents: write` |
| `--release-tag` | `GITHUB_RELEASE_TAG` | `nix-cache` | Release containing cache assets |
| `--store-dir` | `STORE_DIR` | `/nix/store` | Store directory advertised to Nix |
| `--priority` | `CACHE_PRIORITY` | `30` | Binary cache priority |

`GITHUB_REPOSITORY=owner/repo` supplies the owner and repository when they are
not passed explicitly. Compression is selected when creating the local cache;
the publisher preserves Nix's NAR bytes and metadata rather than recompressing
them.

## Storage Design And Limits

The release stores one compressed NAR and one raw `.narinfo` asset per store
path. Keeping raw metadata on the release allows every deployment to rebuild a
complete index, including paths from previous runs. The Pages copy rewrites
each NAR URL to its absolute release asset URL; this does not invalidate Nix
signatures.

GitHub limits a release to 1,000 assets and each asset to 2 GiB. This design
therefore supports roughly 500 store paths per repository release. Release
downloads also need to be publicly accessible to ordinary Nix clients.

## Development

```bash
nix develop
cargo test --locked
nix build
```

## License

[MIT](LICENSE)
