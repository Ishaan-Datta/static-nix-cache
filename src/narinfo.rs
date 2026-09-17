use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use url::Url;

#[derive(Debug, Clone)]
pub struct LocalNarInfo {
    pub filename: String,
    pub raw: String,
    pub nar_asset_name: String,
    pub nar_path: PathBuf,
}

pub fn scan_local_cache(cache_dir: &Path) -> Result<Vec<LocalNarInfo>> {
    if !cache_dir.is_dir() {
        bail!("cache directory does not exist: {}", cache_dir.display());
    }

    let mut entries = Vec::new();

    for dir_entry in fs::read_dir(cache_dir)
        .with_context(|| format!("reading cache directory {}", cache_dir.display()))?
    {
        let dir_entry = dir_entry?;
        let path = dir_entry.path();
        if !path.is_file() {
            continue;
        }

        let Some(filename) = path.file_name().and_then(|v| v.to_str()) else {
            continue;
        };
        if !filename.ends_with(".narinfo") {
            continue;
        }

        let raw = fs::read_to_string(&path)
            .with_context(|| format!("reading narinfo {}", path.display()))?;
        let nar_url = extract_url(&raw)
            .with_context(|| format!("narinfo {filename} has no URL field"))?;
        let relative_nar_path = relative_nar_path(&nar_url)
            .with_context(|| format!("narinfo {filename} has unsupported URL: {nar_url}"))?;
        let nar_asset_name = relative_nar_path
            .file_name()
            .and_then(|v| v.to_str())
            .context("NAR URL does not contain a UTF-8 filename")?
            .to_owned();
        let nar_path = cache_dir.join(&relative_nar_path);

        if !nar_path.is_file() {
            bail!(
                "narinfo {} references {}, but {} does not exist",
                filename,
                nar_url,
                nar_path.display()
            );
        }

        entries.push(LocalNarInfo {
            filename: filename.to_owned(),
            raw,
            nar_asset_name,
            nar_path,
        });
    }

    entries.sort_by(|a, b| a.filename.cmp(&b.filename));
    Ok(entries)
}

pub fn extract_url(content: &str) -> Option<String> {
    content.lines().find_map(|line| {
        line.strip_prefix("URL:")
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(ToOwned::to_owned)
    })
}

/// Resolve the URL from a file:// binary cache to a relative filesystem path.
/// Nix-generated local cache entries normally use `nar/<filename>`.
fn relative_nar_path(value: &str) -> Result<PathBuf> {
    if Url::parse(value).is_ok() {
        bail!("expected a relative URL in the local file cache");
    }

    let path_part = value
        .split(|c| c == '?' || c == '#')
        .next()
        .unwrap_or(value);

    let path = Path::new(path_part);
    if path.is_absolute() {
        bail!("absolute filesystem paths are not supported");
    }
    if path.components().any(|component| matches!(component, std::path::Component::ParentDir)) {
        bail!("parent-directory components are not supported");
    }

    Ok(path.to_path_buf())
}

pub fn rewrite_url(content: &str, absolute_url: &str) -> Result<String> {
    let mut found = false;
    let mut output = String::with_capacity(content.len() + absolute_url.len());

    for line in content.lines() {
        if line.starts_with("URL:") {
            output.push_str("URL: ");
            output.push_str(absolute_url);
            output.push('\n');
            found = true;
        } else {
            output.push_str(line);
            output.push('\n');
        }
    }

    if !found {
        bail!("narinfo has no URL field");
    }

    Ok(output)
}

pub fn nar_asset_name_from_content(content: &str) -> Result<String> {
    let value = extract_url(content).context("narinfo has no URL field")?;

    if let Ok(url) = Url::parse(&value) {
        let segment = url
            .path_segments()
            .and_then(|segments| segments.filter(|s| !s.is_empty()).last())
            .context("absolute NAR URL has no filename")?;
        return Ok(segment.to_owned());
    }

    let path_part = value
        .split(|c| c == '?' || c == '#')
        .next()
        .unwrap_or(&value);
    let name = Path::new(path_part)
        .file_name()
        .and_then(|v| v.to_str())
        .context("relative NAR URL has no UTF-8 filename")?;
    Ok(name.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_relative_url() {
        let raw = "StorePath: /nix/store/abc-pkg\nURL: nar/foo.nar.zst\nNarSize: 1\n";
        assert_eq!(extract_url(raw).as_deref(), Some("nar/foo.nar.zst"));
        assert_eq!(nar_asset_name_from_content(raw).unwrap(), "foo.nar.zst");
    }

    #[test]
    fn extracts_absolute_url_filename() {
        let raw = "URL: https://github.com/o/r/releases/download/tag/foo.nar.xz\n";
        assert_eq!(nar_asset_name_from_content(raw).unwrap(), "foo.nar.xz");
    }

    #[test]
    fn rewrites_only_url_field() {
        let raw = "StorePath: /nix/store/abc-pkg\nURL: nar/foo.nar.zst\nSig: key:sig\n";
        let rewritten = rewrite_url(raw, "https://example.test/foo.nar.zst").unwrap();
        assert!(rewritten.contains("URL: https://example.test/foo.nar.zst\n"));
        assert!(rewritten.contains("Sig: key:sig\n"));
    }
}
