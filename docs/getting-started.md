# Getting Started

The simplest integration snapshots the Nix store before a build and publishes
the new paths afterward:

```yaml
permissions:
  contents: write
  pages: write
  id-token: write

jobs:
  cache:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v6
      - uses: DeterminateSystems/nix-installer-action@main
      - uses: Ishaan-Datta/static-nix-cache/setup@v0.1.0
      - run: nix build
      - id: cache
        uses: Ishaan-Datta/static-nix-cache/deploy@v0.1.0
        with:
          compression: zstd
      - uses: actions/upload-pages-artifact@v4
        with:
          path: ${{ steps.cache.outputs.static-path }}
      - uses: actions/deploy-pages@v4
```

The action stores NARs in the current repository's `nix-cache` GitHub Release
and always generates static metadata. GitHub Pages serves only that cache
metadata; documentation remains in this repository's Markdown files.

Configure Nix to use the Pages root as a substituter:

```nix
{
  nixConfig.extra-substituters = [ "https://<owner>.github.io/<repo>/" ];
}
```

If the cache is signed, also add its public key to
`extra-trusted-public-keys`.
