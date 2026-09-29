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

The action stores NARs in the `nix-cache` GitHub Release by default. Configure
Nix to use the deployed static directory as a substituter:

```nix
{
  nixConfig.extra-substituters = [ "https://<owner>.github.io/<repo>/" ];
}
```

If the cache is signed, also add its public key to
`extra-trusted-public-keys`.
