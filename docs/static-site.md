# Static Site Generation

Pass `--output-dir` to the Rust publisher to write a static binary-cache index:

```bash
GITHUB_TOKEN=... static-nix-cache publish \
  --cache-dir /tmp/publish-cache \
  --owner example \
  --repo project \
  --output-dir ./site
```

The directory contains `nix-cache-info` and one `.narinfo` file per published
store path. Each narinfo URL is rewritten to an absolute GitHub Release URL.

Upload the directory byte-for-byte to GitHub Pages, Cloudflare Pages, or any
other static host. The deploy action always generates this directory and
exposes it through its `static-path` output.
