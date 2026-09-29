# Static Site Generation

Pass `--output-dir` to the Rust publisher, or `static` to the deploy action, to
write a static binary-cache index:

```bash
GITHUB_TOKEN=... static-nix-cache publish \
  --cache-dir /tmp/publish-cache \
  --owner example \
  --repo project \
  --output-dir ./site
```

The directory contains `nix-cache-info`, `.nojekyll`, one `.narinfo` file per
published store path, and a Cloudflare-compatible `_redirects` file. Each
narinfo URL is rewritten to an absolute GitHub Release URL, so hosts do not
need redirect support.

Upload the directory byte-for-byte to GitHub Pages, Cloudflare Pages, or any
other static host. Use `--no-redirects-file` when the compatibility file is not
needed.
