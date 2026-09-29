use std::collections::{BTreeMap, HashMap};
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};

use crate::github::{Asset, GitHub};
use crate::narinfo::{nar_asset_name_from_content, rewrite_url, scan_local_cache, LocalNarInfo};

#[derive(Debug)]
pub struct PublishOptions {
    pub cache_dir: PathBuf,
    pub output_dir: Option<PathBuf>,
    pub owner: String,
    pub repo: String,
    pub token: String,
    pub release_tag: String,
    pub store_dir: String,
    pub priority: u32,
    pub write_redirects_file: bool,
}

pub fn publish(options: PublishOptions) -> Result<()> {
    let local_narinfos = scan_local_cache(&options.cache_dir)?;
    if local_narinfos.is_empty() {
        eprintln!(
            "warning: no .narinfo files found in {}; publishing metadata only",
            options.cache_dir.display()
        );
    }

    let github = GitHub::new(
        options.token.clone(),
        options.owner.clone(),
        options.repo.clone(),
    )?;
    let release = github.get_or_create_release(&options.release_tag)?;
    eprintln!("using GitHub release id={}", release.id);

    let mut assets = index_assets(github.list_assets(release.id)?);

    let local_nars = collect_unique_nars(&local_narinfos)?;
    for (asset_name, path) in &local_nars {
        sync_nar(&github, release.id, &mut assets, asset_name, path)?;
    }

    // Keep raw, Nix-generated narinfo in the release. This is the durable metadata
    // database across runs/matrix jobs. URL rewriting is only for the static site.
    for narinfo in &local_narinfos {
        sync_narinfo(&github, release.id, &mut assets, narinfo)?;
    }

    if let Some(output_dir) = &options.output_dir {
        // Refresh after uploads so static generation sees metadata from this job
        // plus previous/matrix jobs.
        let final_assets = github.list_assets(release.id)?;
        let final_index = index_assets(final_assets.clone());

        generate_static_site(
            &github,
            &options,
            output_dir,
            &local_narinfos,
            &final_assets,
            &final_index,
        )?;

        eprintln!(
            "published {} local narinfo file(s), {} unique NAR(s); static site: {}",
            local_narinfos.len(),
            local_nars.len(),
            output_dir.display()
        );
    } else {
        eprintln!(
            "published {} local narinfo file(s), {} unique NAR(s)",
            local_narinfos.len(),
            local_nars.len()
        );
    }

    Ok(())
}

fn collect_unique_nars(local_narinfos: &[LocalNarInfo]) -> Result<BTreeMap<String, PathBuf>> {
    let mut nars = BTreeMap::new();

    for narinfo in local_narinfos {
        if let Some(existing) = nars.get(&narinfo.nar_asset_name) {
            if existing != &narinfo.nar_path {
                let a = fs::metadata(existing)?.len();
                let b = fs::metadata(&narinfo.nar_path)?.len();
                if a != b {
                    bail!(
                        "two local files map to release asset {} with different sizes",
                        narinfo.nar_asset_name
                    );
                }
            }
            continue;
        }

        nars.insert(narinfo.nar_asset_name.clone(), narinfo.nar_path.clone());
    }

    Ok(nars)
}

fn sync_nar(
    github: &GitHub,
    release_id: u64,
    assets: &mut HashMap<String, Asset>,
    name: &str,
    path: &Path,
) -> Result<()> {
    let local_size = fs::metadata(path)
        .with_context(|| format!("stat NAR {}", path.display()))?
        .len();

    if let Some(existing) = assets.get(name).cloned() {
        // Nix names local-cache NARs by the compressed file hash. Matching name
        // and size is enough to treat the immutable blob as already present.
        if existing.size == local_size {
            eprintln!("NAR already present: {name}");
            return Ok(());
        }

        eprintln!(
            "NAR asset {name} exists with size {}, local size is {}; replacing it",
            existing.size, local_size
        );
        github.delete_asset(existing.id)?;
        assets.remove(name);
    }

    eprintln!("uploading NAR: {name} ({local_size} bytes)");
    let uploaded = github.upload_file(release_id, name, path, "application/x-nix-nar")?;
    assets.insert(name.to_owned(), uploaded);
    Ok(())
}

fn sync_narinfo(
    github: &GitHub,
    release_id: u64,
    assets: &mut HashMap<String, Asset>,
    narinfo: &LocalNarInfo,
) -> Result<()> {
    if let Some(existing) = assets.get(&narinfo.filename).cloned() {
        // Avoid needless delete/upload churn when metadata is byte-for-byte equal.
        let remote = github.download_asset(&existing)?;
        if remote.as_slice() == narinfo.raw.as_bytes() {
            eprintln!("narinfo already present: {}", narinfo.filename);
            return Ok(());
        }

        eprintln!("replacing narinfo: {}", narinfo.filename);
        github.delete_asset(existing.id)?;
        assets.remove(&narinfo.filename);
    } else {
        eprintln!("uploading narinfo: {}", narinfo.filename);
    }

    let uploaded = github.upload_bytes(
        release_id,
        &narinfo.filename,
        narinfo.raw.as_bytes().to_vec(),
        "text/x-nix-narinfo",
    )?;
    assets.insert(narinfo.filename.clone(), uploaded);
    Ok(())
}

fn generate_static_site(
    github: &GitHub,
    options: &PublishOptions,
    output_dir: &Path,
    local_narinfos: &[LocalNarInfo],
    all_assets: &[Asset],
    asset_index: &HashMap<String, Asset>,
) -> Result<()> {
    fs::create_dir_all(output_dir)
        .with_context(|| format!("creating output directory {}", output_dir.display()))?;

    let cache_info = format!(
        "StoreDir: {}\nWantMassQuery: 1\nPriority: {}\n",
        options.store_dir, options.priority
    );
    fs::write(output_dir.join("nix-cache-info"), cache_info).context("writing nix-cache-info")?;

    // Disable Jekyll processing on GitHub Pages. The cache is already a complete
    // static tree and should be served byte-for-byte.
    fs::write(output_dir.join(".nojekyll"), b"").context("writing .nojekyll")?;

    let local_by_name: HashMap<&str, &LocalNarInfo> = local_narinfos
        .iter()
        .map(|entry| (entry.filename.as_str(), entry))
        .collect();

    let mut narinfo_assets: Vec<&Asset> = all_assets
        .iter()
        .filter(|asset| asset.name.ends_with(".narinfo"))
        .collect();
    narinfo_assets.sort_by(|a, b| a.name.cmp(&b.name));

    let mut generated = 0_usize;
    for asset in narinfo_assets {
        let raw = if let Some(local) = local_by_name.get(asset.name.as_str()) {
            local.raw.clone()
        } else {
            let bytes = github.download_asset(asset)?;
            String::from_utf8(bytes)
                .with_context(|| format!("narinfo asset {} is not UTF-8", asset.name))?
        };

        let nar_asset_name = nar_asset_name_from_content(&raw)
            .with_context(|| format!("parsing URL in {}", asset.name))?;
        if !asset_index.contains_key(&nar_asset_name) {
            eprintln!(
                "warning: {} references NAR asset {}, but it is not present on the release",
                asset.name, nar_asset_name
            );
        }

        let absolute_nar_url =
            github.release_download_url(&options.release_tag, &nar_asset_name)?;
        let output = rewrite_url(&raw, &absolute_nar_url)?;
        fs::write(output_dir.join(&asset.name), output)
            .with_context(|| format!("writing static narinfo {}", asset.name))?;
        generated += 1;
    }

    if options.write_redirects_file {
        // Retained for compatibility with Cloudflare Pages. The rewritten absolute
        // URL fields mean Nix does not depend on this file, including on GitHub Pages.
        let marker = "__STATIC_NIX_CACHE_ASSET__";
        let example = github.release_download_url(&options.release_tag, marker)?;
        let base = example
            .strip_suffix(marker)
            .context("constructed GitHub release URL does not end in marker")?;
        let redirects = format!("/nar/:filename {base}:filename 302\n");
        fs::write(output_dir.join("_redirects"), redirects).context("writing _redirects")?;
    }

    eprintln!("generated {generated} static narinfo file(s)");
    Ok(())
}

fn index_assets(assets: Vec<Asset>) -> HashMap<String, Asset> {
    assets
        .into_iter()
        .map(|asset| (asset.name.clone(), asset))
        .collect()
}
