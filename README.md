# static-nix-cache

Publish Nix build results as a binary cache backed by GitHub Releases and
GitHub Pages. NAR files live in a release while Pages serves the small
`nix-cache-info` and `.narinfo` files Nix needs to find them. No cache server is
required.

## Deploy From GitHub Actions

Enable GitHub Pages with Github actions by navigating to Settings -> Pages, then set the Build and Deployment source to Github Actions. Then add this workflow:

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
        with:
          # Omit `signing-key` if you want an unsigned cache
          signing-key: ${{ secrets.NIX_CACHE_SIGNING_KEY }}
```

That final action is the complete publishing pipeline. It:

1. Finds the `result` and `result-*` links created by `nix build`.
2. Copies their closures into a compressed local binary cache.
3. Publishes the NARs and durable metadata to `nix-cache` GitHub Release
   shards.
4. Generates the static cache index, uploads it as a Pages artifact, and
   deploys it.
5. Removes release assets that are not part of the newly deployed closure.

Use `paths` when the build does not leave standard result links:

```yaml
- name: Build packages
  run: |
    nix build .#package -o package-result
    nix build .#documentation -o documentation-result
    nix build .#container -o container-result

- uses: Ishaan-Datta/static-nix-cache/deploy@v1.0.1
  with:
    paths: |
      package-result
      documentation-result
      container-result
```

## Setting Up Cache Signing

Start by generating a public/private key pair:

```bash
nix-store --generate-binary-cache-key \
  wfas-nix-1 \
  cache-private-key.pem \
  cache-public-key.pem
```

Set the private key for the repository by navigating to Settings -> Secrets and variables -> Actions -> New Repository Secret, then add value for: `NIX_CACHE_SIGNING_KEY`.

You can verify the cache works by trying the substitution locally:

```bash
curl https://ishaan-datta.github.io/wfas-nix/nix-cache-info

nix build github:Ishaan-Datta/wfas-nix \
  --option extra-substituters https://ishaan-datta.github.io/wfas-nix/ \
  --option extra-trusted-public-keys 'wfas-nix-1:YOUR_PUBLIC_KEY'
```

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

For a signed cache, consumers must add the corresponding public key to
`extra-trusted-public-keys`.

## Action Options

| Input          | Default              | Description                                                   |
| -------------- | -------------------- | ------------------------------------------------------------- |
| `paths`        | `result`, `result-*` | Newline-separated build result paths                          |
| `compression`  | `zstd`               | NAR compression: `none`, `xz`, or `zstd`                      |
| `signing-key`  |                      | Nix secret key in `keyname:base64-ed25519-private` format     |
| `deploy-pages` | `true`               | Set to `false` to publish without deploying Pages             |
| `prune`        | `true`               | Remove paths absent from this deployment after Pages succeeds |

The action outputs `page-url` and `static-path`. When `deploy-pages` is false,
`static-path` can be uploaded to any static host without modification.

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

The matrix `save` jobs only upload workflow artifacts; they never create or
modify GitHub Releases. `needs: build` waits for every matrix job, then the one
final `deploy` job downloads their artifacts and is the sole release writer.

Set workflow concurrency as shown in the single-job example so two runs do not
update the same release simultaneously.

With the default `prune: true`, each deployment is an authoritative cache
snapshot. Every output that should remain available must be built and passed to
`deploy`, or saved by a required matrix job in the same workflow run. The final
deploy job must run only after all required matrix jobs succeed; do not use
`if: always()` to publish a partial matrix. Keep branches with different target
sets in separate caches, and keep `cancel-in-progress: false` so deployments do
not overlap. Prefer explicit result links or `paths` so the authoritative
closure represents intended outputs rather than incidental store activity.

Use `prune: false` when intentionally publishing only part of a cache. This
keeps prior paths in both the static index and the release shards. Pruned paths
from older commits are no longer available as substitutes and must be rebuilt.

## Command Line

The publisher can also publish an existing local file cache:

```bash
nix copy --to 'file:///tmp/publish-cache?compression=zstd' /nix/store/...

GITHUB_TOKEN=... static-nix-cache publish \
  --cache-dir /tmp/publish-cache \
  --owner example \
  --repo project \
  --output-dir ./site \
  --authoritative
```

| Option            | Environment          | Default            | Description                                     |
| ----------------- | -------------------- | ------------------ | ----------------------------------------------- |
| `--cache-dir`     |                      | required           | Cache produced by `nix copy --to file://...`    |
| `--output-dir`    |                      |                    | Static metadata output directory                |
| `--owner`         | `GITHUB_OWNER`       | repository context | Destination owner                               |
| `--repo`          | `GITHUB_REPO`        | repository context | Destination repository                          |
| `--token`         | `GITHUB_TOKEN`       | required           | Token with `contents: write`                    |
| `--release-tag`   | `GITHUB_RELEASE_TAG` | `nix-cache`        | Base tag for cache release shards               |
| `--store-dir`     | `STORE_DIR`          | `/nix/store`       | Store directory advertised to Nix               |
| `--priority`      | `CACHE_PRIORITY`     | `30`               | Binary cache priority                           |
| `--authoritative` |                      | `false`            | Generate metadata only for the supplied closure |

`GITHUB_REPOSITORY=owner/repo` supplies the owner and repository when they are
not passed explicitly. Compression is selected when creating the local cache;
the publisher preserves Nix's NAR bytes and metadata rather than recompressing
them.

For manual authoritative deployments, publish with `--authoritative`, deploy
the generated site, and only then remove stale release assets:

```bash
GITHUB_TOKEN=... static-nix-cache prune \
  --cache-dir /tmp/publish-cache \
  --owner example \
  --repo project
```

The deploy action performs this ordering automatically. When `deploy-pages` is
false, it generates authoritative metadata but leaves remote assets intact;
run `prune` yourself after the external static host has deployed the new index.

## Storage Design And Limits

The cache stores compressed NARs and raw `.narinfo` files in release shards.
The first shard uses the base tag `nix-cache`; additional shards use tags such
as `nix-cache-0001` and carry versioned static-cache metadata in their release
body. The Pages copy combines metadata from all applicable shards and rewrites
each NAR URL to the absolute URL for its owning release. This is transparent to
Nix clients and does not invalidate signatures.

GitHub limits a release to 1,000 assets and each asset to 2 GiB. This design
therefore stores roughly 500 conventional store paths per shard and creates
more shards as needed. Release downloads need to be publicly accessible to
ordinary Nix clients.

After an authoritative Pages deployment succeeds, pruning compares all managed
assets against the new local closure. It removes stale `.narinfo` files and NAR
files not referenced by any retained path. Empty releases remain available for
future shard reuse. If publication or Pages deployment fails, pruning does not
run; if pruning itself fails, the active index remains valid and only stale
storage is retained.

## License

[GNU General Public License v2.0 only](LICENSE)
