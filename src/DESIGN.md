# Design notes

## Data flow

```text
/nix/store (or restored file:// cache)
        |
        | nix copy --to file://...
        v
local Nix binary cache
  <store-hash>.narinfo
  nar/<file-hash>.nar[.xz|.zst]
        |
        | static-nix-cache publish
        v
GitHub Release
  <file-hash>.nar[.xz|.zst]
  <store-hash>.narinfo     (raw Nix-generated metadata; persistent index)
        |
        +---- NAR download URLs
        |
        v
static output / GitHub Pages
  nix-cache-info
  <store-hash>.narinfo     (URL rewritten to absolute Release URL)
  .nojekyll
```

## Why raw narinfo is stored on the release

A GitHub Pages deployment is an output artifact, not the authoritative metadata
store for future CI jobs. Persisting raw `.narinfo` alongside the NAR assets lets
the next run reconstruct a complete cache index, including entries produced by
older runs and other matrix jobs.

## Why URL is rewritten only in the static copy

The local file cache naturally contains `URL: nar/<filename>`. The release stores
NARs as flat asset names. The static copy replaces the URL with the absolute
GitHub Release download URL. Nix accepts an absolute URL here, and the URL is not
part of the binary-cache signature fingerprint, so existing signatures remain
valid.

## Signing

The Rust publisher does not implement Nix private-key signing. Sign the local
file cache with `nix store sign --store file://...` before publishing. This keeps
Nix itself responsible for its fingerprint and key format.

## Current GitHub Release limitation

GitHub permits at most 1,000 assets per release and each asset must be under
2 GiB. Because this compatibility design stores roughly one NAR plus one
`.narinfo` asset per store path, a single release will eventually become a
scaling limit. A larger implementation should shard NARs across multiple
release tags and/or consolidate narinfo metadata into an index asset.
