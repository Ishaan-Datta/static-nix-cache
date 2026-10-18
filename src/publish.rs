use std::collections::{BTreeMap, HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{bail, Context, Result};

use crate::github::{Asset, GitHub, Release};
use crate::narinfo::{nar_asset_name_from_content, rewrite_url, scan_local_cache, LocalNarInfo};

const MAX_RELEASE_ASSETS: usize = 1_000;
const CACHE_FORMAT: &str = "static-nix-cache:v1";
const LEGACY_RELEASE_BODY: &str = "Nix binary cache NAR files managed by static-nix-cache.";
const STAGED_NARINFO_SUFFIX: &str = ".static-nix-cache-upload";

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
    pub authoritative: bool,
}

#[derive(Debug)]
pub struct PruneOptions {
    pub cache_dir: PathBuf,
    pub owner: String,
    pub repo: String,
    pub token: String,
    pub release_tag: String,
}

#[derive(Debug)]
struct CacheShard {
    number: u32,
    release: Release,
    assets: HashMap<String, Asset>,
}

pub fn publish(options: PublishOptions) -> Result<()> {
    let local_narinfos = scan_local_cache(&options.cache_dir)?;
    if let Some(output_dir) = &options.output_dir {
        ensure_distinct_directories(&options.cache_dir, output_dir)?;
    }
    if options.authoritative && local_narinfos.is_empty() {
        bail!(
            "refusing to publish an empty authoritative cache: {}",
            options.cache_dir.display()
        );
    }
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
    let mut shards = load_cache_shards(&github, &options.release_tag)?;
    if shards.is_empty() {
        shards.push(create_cache_shard(&github, &options.release_tag, 0)?);
    }
    eprintln!("using {} GitHub cache release shard(s)", shards.len());

    let mut owners = index_asset_owners(&shards)?;
    let local_nars = collect_unique_nars(&local_narinfos)?;
    for (asset_name, path) in &local_nars {
        sync_nar(
            &github,
            &options.release_tag,
            &mut shards,
            &mut owners,
            asset_name,
            path,
        )?;
    }

    // Keep raw, Nix-generated narinfo in the releases. URL rewriting is only
    // for the static site served to Nix clients.
    for narinfo in &local_narinfos {
        sync_narinfo(
            &github,
            &options.release_tag,
            &mut shards,
            &mut owners,
            narinfo,
        )?;
    }

    if let Some(output_dir) = &options.output_dir {
        generate_static_site(
            &github,
            &options,
            output_dir,
            &local_narinfos,
            &shards,
            &owners,
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

pub fn prune(options: PruneOptions) -> Result<()> {
    let local_narinfos = scan_local_cache(&options.cache_dir)?;
    if local_narinfos.is_empty() {
        bail!(
            "refusing to prune from an empty local cache: {}",
            options.cache_dir.display()
        );
    }

    let github = GitHub::new(options.token, options.owner, options.repo)?;
    let shards = load_cache_shards(&github, &options.release_tag)?;
    if shards.is_empty() {
        eprintln!("no cache release shards found; nothing to prune");
        return Ok(());
    }

    let desired_narinfos: HashSet<&str> = local_narinfos
        .iter()
        .map(|narinfo| narinfo.filename.as_str())
        .collect();
    let desired_nars: HashSet<&str> = local_narinfos
        .iter()
        .map(|narinfo| narinfo.nar_asset_name.as_str())
        .collect();
    let owners = index_asset_owners(&shards)?;

    for narinfo in &local_narinfos {
        if !owners.contains_key(&narinfo.filename) {
            bail!(
                "refusing to prune because desired asset {} is not present in a cache release",
                narinfo.filename
            );
        }
        let nar_shard = owners.get(&narinfo.nar_asset_name).with_context(|| {
            format!(
                "refusing to prune because desired asset {} is not present in a cache release",
                narinfo.nar_asset_name
            )
        })?;
        let remote_nar = shards[*nar_shard]
            .assets
            .get(&narinfo.nar_asset_name)
            .context("indexed NAR asset is missing from its shard")?;
        let local_size = fs::metadata(&narinfo.nar_path)
            .with_context(|| format!("stat NAR {}", narinfo.nar_path.display()))?
            .len();
        if remote_nar.size != local_size {
            bail!(
                "refusing to prune because desired NAR {} has remote size {} and local size {}",
                narinfo.nar_asset_name,
                remote_nar.size,
                local_size
            );
        }
    }

    let mut stale = Vec::new();
    for shard in &shards {
        for asset in shard.assets.values() {
            let desired = if asset.name.ends_with(".narinfo") {
                desired_narinfos.contains(asset.name.as_str())
            } else if is_nar_asset_name(&asset.name) {
                desired_nars.contains(asset.name.as_str())
            } else {
                true
            };

            if !desired {
                stale.push((shard.release.tag_name.clone(), asset.clone()));
            }
        }
    }
    stale.sort_by(|a, b| a.1.name.cmp(&b.1.name));

    for (tag, asset) in &stale {
        eprintln!("deleting stale asset from {tag}: {}", asset.name);
        github.delete_asset(asset.id)?;
    }

    eprintln!(
        "pruned {} stale asset(s) across {} cache release shard(s)",
        stale.len(),
        shards.len()
    );
    Ok(())
}

fn load_cache_shards(github: &GitHub, base_tag: &str) -> Result<Vec<CacheShard>> {
    let mut matching = Vec::new();
    for release in github.list_releases()? {
        let Some(number) = cache_shard_number(base_tag, &release) else {
            continue;
        };
        if release.draft {
            bail!("cache release {} must not be a draft", release.tag_name);
        }
        matching.push((number, release));
    }
    matching.sort_by_key(|(number, _)| *number);

    for pair in matching.windows(2) {
        if pair[0].0 == pair[1].0 {
            bail!("multiple cache releases claim shard {}", pair[0].0);
        }
    }

    let mut shards = Vec::with_capacity(matching.len());
    for (number, release) in matching {
        let assets = index_assets(github.list_assets(release.id)?)?;
        shards.push(CacheShard {
            number,
            release,
            assets,
        });
    }
    recover_staged_narinfos(github, &mut shards)?;
    Ok(shards)
}

fn recover_staged_narinfos(github: &GitHub, shards: &mut [CacheShard]) -> Result<()> {
    let mut final_names: HashSet<String> = shards
        .iter()
        .flat_map(|shard| shard.assets.keys())
        .filter(|name| !name.ends_with(STAGED_NARINFO_SUFFIX))
        .cloned()
        .collect();
    let mut staged_assets = Vec::new();
    for (shard_index, shard) in shards.iter().enumerate() {
        for (name, asset) in &shard.assets {
            if name.ends_with(STAGED_NARINFO_SUFFIX) {
                staged_assets.push((shard_index, name.clone(), asset.clone()));
            }
        }
    }
    staged_assets.sort_by(|a, b| a.1.cmp(&b.1));

    for (shard_index, staged_name, staged) in staged_assets {
        let final_name = staged_name
            .strip_suffix(STAGED_NARINFO_SUFFIX)
            .context("staged narinfo name lost its suffix")?;

        if final_names.contains(final_name) {
            eprintln!(
                "removing superseded staged narinfo from {}: {staged_name}",
                shards[shard_index].release.tag_name
            );
            github.delete_asset(staged.id)?;
            shards[shard_index].assets.remove(&staged_name);
        } else {
            eprintln!(
                "recovering staged narinfo on {}: {final_name}",
                shards[shard_index].release.tag_name
            );
            let recovered = github.rename_asset(staged.id, final_name)?;
            shards[shard_index].assets.remove(&staged_name);
            shards[shard_index]
                .assets
                .insert(final_name.to_owned(), recovered);
            final_names.insert(final_name.to_owned());
        }
    }
    Ok(())
}

fn create_cache_shard(github: &GitHub, base_tag: &str, number: u32) -> Result<CacheShard> {
    let tag = cache_shard_tag(base_tag, number);
    let body = cache_shard_body(base_tag, number);
    let release = github.get_or_create_release(&tag, &body)?;
    if release.draft {
        bail!("cache release {} must not be a draft", release.tag_name);
    }
    let valid_body = release.body.as_deref() == Some(body.as_str())
        || (number == 0 && release.body.as_deref() == Some(LEGACY_RELEASE_BODY));
    if !valid_body {
        bail!(
            "release {} already exists but is not a static-nix-cache shard",
            release.tag_name
        );
    }
    let assets = index_assets(github.list_assets(release.id)?)?;
    eprintln!("created cache release shard {tag}");
    let mut shard = CacheShard {
        number,
        release,
        assets,
    };
    recover_staged_narinfos(github, std::slice::from_mut(&mut shard))?;
    Ok(shard)
}

fn cache_shard_number(base_tag: &str, release: &Release) -> Option<u32> {
    if release.tag_name == base_tag {
        let body = release.body.as_deref();
        return (body == Some(cache_shard_body(base_tag, 0).as_str())
            || body == Some(LEGACY_RELEASE_BODY))
        .then_some(0);
    }

    let suffix = release.tag_name.strip_prefix(&format!("{base_tag}-"))?;
    let number = suffix.parse().ok()?;
    (release.body.as_deref() == Some(cache_shard_body(base_tag, number).as_str())).then_some(number)
}

fn cache_shard_tag(base_tag: &str, number: u32) -> String {
    if number == 0 {
        base_tag.to_owned()
    } else {
        format!("{base_tag}-{number:04}")
    }
}

fn cache_shard_body(base_tag: &str, number: u32) -> String {
    format!("{CACHE_FORMAT}\ncache: {base_tag}\nshard: {number}\n")
}

fn index_asset_owners(shards: &[CacheShard]) -> Result<HashMap<String, usize>> {
    let mut owners = HashMap::new();
    for (index, shard) in shards.iter().enumerate() {
        for name in shard.assets.keys() {
            if let Some(previous) = owners.insert(name.clone(), index) {
                bail!(
                    "asset {name} is present in both {} and {}",
                    shards[previous].release.tag_name,
                    shard.release.tag_name
                );
            }
        }
    }
    Ok(owners)
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
    base_tag: &str,
    shards: &mut Vec<CacheShard>,
    owners: &mut HashMap<String, usize>,
    name: &str,
    path: &Path,
) -> Result<()> {
    let local_size = fs::metadata(path)
        .with_context(|| format!("stat NAR {}", path.display()))?
        .len();

    if let Some(&shard_index) = owners.get(name) {
        let existing = shards[shard_index]
            .assets
            .get(name)
            .context("indexed NAR asset is missing from its shard")?
            .clone();
        if existing.size == local_size {
            eprintln!("NAR already present: {name}");
            return Ok(());
        }

        bail!(
            "NAR asset {name} exists with size {}, but the local content-addressed file has size {local_size}",
            existing.size
        );
    }

    let shard_index = upload_shard(github, base_tag, shards)?;
    eprintln!(
        "uploading NAR to {}: {name} ({local_size} bytes)",
        shards[shard_index].release.tag_name
    );
    let uploaded = github.upload_file(
        shards[shard_index].release.id,
        name,
        path,
        "application/x-nix-nar",
    )?;
    shards[shard_index].assets.insert(name.to_owned(), uploaded);
    owners.insert(name.to_owned(), shard_index);
    Ok(())
}

fn sync_narinfo(
    github: &GitHub,
    base_tag: &str,
    shards: &mut Vec<CacheShard>,
    owners: &mut HashMap<String, usize>,
    narinfo: &LocalNarInfo,
) -> Result<()> {
    if let Some(&shard_index) = owners.get(&narinfo.filename) {
        let existing = shards[shard_index]
            .assets
            .get(&narinfo.filename)
            .context("indexed narinfo asset is missing from its shard")?
            .clone();
        let remote = github.download_asset(&existing)?;
        if remote.as_slice() == narinfo.raw.as_bytes() {
            eprintln!("narinfo already present: {}", narinfo.filename);
            return Ok(());
        }

        eprintln!("replacing narinfo: {}", narinfo.filename);
        let staged_name = format!("{}{}", narinfo.filename, STAGED_NARINFO_SUFFIX);
        let staged_shard = upload_shard(github, base_tag, shards)?;
        let staged = github.upload_bytes(
            shards[staged_shard].release.id,
            &staged_name,
            narinfo.raw.as_bytes().to_vec(),
            "text/x-nix-narinfo",
        )?;
        shards[staged_shard]
            .assets
            .insert(staged_name.clone(), staged.clone());
        owners.insert(staged_name.clone(), staged_shard);

        if let Err(error) = github.delete_asset(existing.id) {
            return Err(error).with_context(|| {
                format!(
                    "deleting old narinfo after staging {staged_name}; the staged asset will be recovered on the next run"
                )
            });
        }
        shards[shard_index].assets.remove(&narinfo.filename);
        owners.remove(&narinfo.filename);

        let uploaded = github
            .rename_asset(staged.id, &narinfo.filename)
            .with_context(|| {
                format!(
                    "promoting staged narinfo {staged_name}; it will be recovered on the next run"
                )
            })?;
        shards[staged_shard].assets.remove(&staged_name);
        owners.remove(&staged_name);
        shards[staged_shard]
            .assets
            .insert(narinfo.filename.clone(), uploaded);
        owners.insert(narinfo.filename.clone(), staged_shard);
        return Ok(());
    }

    let shard_index = upload_shard(github, base_tag, shards)?;
    eprintln!(
        "uploading narinfo to {}: {}",
        shards[shard_index].release.tag_name, narinfo.filename
    );
    let uploaded = github.upload_bytes(
        shards[shard_index].release.id,
        &narinfo.filename,
        narinfo.raw.as_bytes().to_vec(),
        "text/x-nix-narinfo",
    )?;
    shards[shard_index]
        .assets
        .insert(narinfo.filename.clone(), uploaded);
    owners.insert(narinfo.filename.clone(), shard_index);
    Ok(())
}

fn upload_shard(github: &GitHub, base_tag: &str, shards: &mut Vec<CacheShard>) -> Result<usize> {
    if let Some(index) = available_upload_shard(shards) {
        return Ok(index);
    }

    let number = shards
        .iter()
        .map(|shard| shard.number)
        .max()
        .unwrap_or(0)
        .checked_add(1)
        .context("cache shard number overflow")?;
    shards.push(create_cache_shard(github, base_tag, number)?);
    Ok(shards.len() - 1)
}

fn available_upload_shard(shards: &[CacheShard]) -> Option<usize> {
    shards
        .iter()
        .rposition(|shard| shard.assets.len() < MAX_RELEASE_ASSETS)
}

fn generate_static_site(
    github: &GitHub,
    options: &PublishOptions,
    output_dir: &Path,
    local_narinfos: &[LocalNarInfo],
    shards: &[CacheShard],
    owners: &HashMap<String, usize>,
) -> Result<()> {
    let cache_info = format!(
        "StoreDir: {}\nWantMassQuery: 1\nPriority: {}\n",
        options.store_dir, options.priority
    );

    let local_by_name: HashMap<&str, &LocalNarInfo> = local_narinfos
        .iter()
        .map(|entry| (entry.filename.as_str(), entry))
        .collect();
    let mut narinfo_names: Vec<String> = if options.authoritative {
        local_narinfos
            .iter()
            .map(|entry| entry.filename.clone())
            .collect()
    } else {
        owners
            .keys()
            .filter(|name| name.ends_with(".narinfo"))
            .cloned()
            .collect()
    };
    narinfo_names.sort();

    let mut generated = Vec::with_capacity(narinfo_names.len());
    for name in &narinfo_names {
        let raw = if let Some(local) = local_by_name.get(name.as_str()) {
            local.raw.clone()
        } else {
            let shard_index = *owners
                .get(name)
                .with_context(|| format!("narinfo asset {name} has no owning shard"))?;
            let asset = shards[shard_index]
                .assets
                .get(name)
                .with_context(|| format!("narinfo asset {name} is missing from its shard"))?;
            let bytes = github.download_asset(asset)?;
            String::from_utf8(bytes)
                .with_context(|| format!("narinfo asset {name} is not UTF-8"))?
        };

        let nar_asset_name =
            nar_asset_name_from_content(&raw).with_context(|| format!("parsing URL in {name}"))?;
        let nar_shard = *owners.get(&nar_asset_name).with_context(|| {
            format!(
                "{name} references NAR asset {nar_asset_name}, but it is not present in a cache release"
            )
        })?;

        let absolute_nar_url =
            github.release_download_url(&shards[nar_shard].release.tag_name, &nar_asset_name)?;
        let output = rewrite_url(&raw, &absolute_nar_url)?;
        generated.push((name.clone(), output));
    }

    replace_static_site(output_dir, &cache_info, &generated)?;

    eprintln!("generated {} static narinfo file(s)", narinfo_names.len());
    Ok(())
}

fn replace_static_site(
    output_dir: &Path,
    cache_info: &str,
    narinfos: &[(String, String)],
) -> Result<()> {
    if output_dir.exists() && !output_dir.is_dir() {
        bail!(
            "static output path is not a directory: {}",
            output_dir.display()
        );
    }
    let parent = output_dir
        .parent()
        .filter(|path| !path.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    fs::create_dir_all(parent)
        .with_context(|| format!("creating static output parent {}", parent.display()))?;

    let staging = temporary_sibling(output_dir, "staging")?;
    fs::create_dir(&staging)
        .with_context(|| format!("creating staged static site {}", staging.display()))?;
    let write_result = (|| -> Result<()> {
        if output_dir.exists() {
            copy_unmanaged_entries(output_dir, &staging)?;
        }
        fs::write(staging.join("nix-cache-info"), cache_info)
            .context("writing staged nix-cache-info")?;
        for (name, output) in narinfos {
            fs::write(staging.join(name), output)
                .with_context(|| format!("writing staged static narinfo {name}"))?;
        }
        Ok(())
    })();
    if let Err(error) = write_result {
        let _ = fs::remove_dir_all(&staging);
        return Err(error);
    }

    if !output_dir.exists() {
        return fs::rename(&staging, output_dir)
            .with_context(|| format!("installing static site at {}", output_dir.display()));
    }

    let backup = temporary_sibling(output_dir, "backup")?;
    fs::rename(output_dir, &backup).with_context(|| {
        format!(
            "moving existing static site {} to {}",
            output_dir.display(),
            backup.display()
        )
    })?;
    if let Err(error) = fs::rename(&staging, output_dir) {
        let rollback = fs::rename(&backup, output_dir);
        let _ = fs::remove_dir_all(&staging);
        if let Err(rollback_error) = rollback {
            bail!(
                "installing static site failed: {error}; restoring {} also failed: {rollback_error}",
                output_dir.display()
            );
        }
        return Err(error)
            .with_context(|| format!("installing static site at {}", output_dir.display()));
    }
    if let Err(error) = fs::remove_dir_all(&backup) {
        eprintln!(
            "warning: could not remove old static site {}: {error}",
            backup.display()
        );
    }
    Ok(())
}

fn copy_unmanaged_entries(source: &Path, destination: &Path) -> Result<()> {
    for entry in fs::read_dir(source)
        .with_context(|| format!("reading existing static site {}", source.display()))?
    {
        let entry = entry?;
        let name = entry.file_name();
        let managed = name
            .to_str()
            .is_some_and(|name| name == "nix-cache-info" || name.ends_with(".narinfo"));
        if managed {
            continue;
        }
        copy_entry(&entry.path(), &destination.join(name))?;
    }
    Ok(())
}

fn copy_entry(source: &Path, destination: &Path) -> Result<()> {
    let metadata = fs::symlink_metadata(source)
        .with_context(|| format!("reading metadata for {}", source.display()))?;
    if metadata.file_type().is_symlink() {
        let target = fs::read_link(source)
            .with_context(|| format!("reading symlink {}", source.display()))?;
        create_symlink(&target, destination, source)?;
    } else if metadata.is_dir() {
        fs::create_dir(destination)
            .with_context(|| format!("creating directory {}", destination.display()))?;
        for entry in fs::read_dir(source)
            .with_context(|| format!("reading directory {}", source.display()))?
        {
            let entry = entry?;
            copy_entry(&entry.path(), &destination.join(entry.file_name()))?;
        }
    } else if metadata.is_file() {
        fs::copy(source, destination).with_context(|| {
            format!(
                "copying static site entry {} to {}",
                source.display(),
                destination.display()
            )
        })?;
    } else {
        bail!("unsupported entry in static output: {}", source.display());
    }
    Ok(())
}

#[cfg(unix)]
fn create_symlink(target: &Path, destination: &Path, _source: &Path) -> Result<()> {
    std::os::unix::fs::symlink(target, destination)
        .with_context(|| format!("copying symlink to {}", destination.display()))
}

#[cfg(windows)]
fn create_symlink(target: &Path, destination: &Path, source: &Path) -> Result<()> {
    let target_metadata = fs::metadata(source)
        .with_context(|| format!("reading symlink target metadata for {}", source.display()))?;
    if target_metadata.is_dir() {
        std::os::windows::fs::symlink_dir(target, destination)
    } else {
        std::os::windows::fs::symlink_file(target, destination)
    }
    .with_context(|| format!("copying symlink to {}", destination.display()))
}

fn temporary_sibling(path: &Path, purpose: &str) -> Result<PathBuf> {
    let parent = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .context("static output directory must have a UTF-8 name")?;
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .context("system clock is before the Unix epoch")?
        .as_nanos();
    Ok(parent.join(format!(
        ".{name}.static-nix-cache-{purpose}-{}-{nonce}",
        std::process::id()
    )))
}

fn ensure_distinct_directories(cache_dir: &Path, output_dir: &Path) -> Result<()> {
    let cache_dir = intended_canonical_path(cache_dir)?;
    let output_dir = intended_canonical_path(output_dir)?;
    if cache_dir.starts_with(&output_dir) || output_dir.starts_with(&cache_dir) {
        bail!(
            "cache directory {} and static output directory {} must not overlap",
            cache_dir.display(),
            output_dir.display()
        );
    }
    Ok(())
}

fn intended_canonical_path(path: &Path) -> Result<PathBuf> {
    if path.exists() {
        return fs::canonicalize(path)
            .with_context(|| format!("canonicalizing path {}", path.display()));
    }

    let parent = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    let name = path.file_name().context("path must name a directory")?;
    Ok(intended_canonical_path(parent)?.join(name))
}

fn is_nar_asset_name(name: &str) -> bool {
    name.ends_with(".nar") || name.contains(".nar.")
}

fn index_assets(assets: Vec<Asset>) -> Result<HashMap<String, Asset>> {
    let mut index = HashMap::new();
    for asset in assets {
        let name = asset.name.clone();
        if index.insert(name.clone(), asset).is_some() {
            bail!("release contains duplicate asset name {name}");
        }
    }
    Ok(index)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    fn release(tag: &str, body: Option<String>) -> Release {
        Release {
            id: 1,
            tag_name: tag.to_owned(),
            body,
            draft: false,
        }
    }

    fn asset(id: u64, name: String) -> Asset {
        Asset {
            id,
            name,
            size: 1,
            url: "https://api.github.test/asset".to_owned(),
        }
    }

    fn shard(number: u32, asset_count: usize) -> CacheShard {
        let tag = cache_shard_tag("nix-cache", number);
        CacheShard {
            number,
            release: release(&tag, Some(cache_shard_body("nix-cache", number))),
            assets: (0..asset_count)
                .map(|index| {
                    let name = format!("asset-{number}-{index}");
                    (name.clone(), asset(index as u64, name))
                })
                .collect(),
        }
    }

    #[test]
    fn recognizes_legacy_base_and_marked_shards() {
        assert_eq!(
            cache_shard_number(
                "nix-cache",
                &release("nix-cache", Some(LEGACY_RELEASE_BODY.to_owned()))
            ),
            Some(0)
        );
        assert_eq!(
            cache_shard_number(
                "nix-cache",
                &release("nix-cache-0002", Some(cache_shard_body("nix-cache", 2)))
            ),
            Some(2)
        );
    }

    #[test]
    fn ignores_unmarked_prefixed_releases() {
        assert_eq!(
            cache_shard_number("nix-cache", &release("nix-cache", None)),
            None
        );
        assert_eq!(
            cache_shard_number("nix-cache", &release("nix-cache-0002", None)),
            None
        );
        assert_eq!(
            cache_shard_number(
                "nix-cache",
                &release("nix-cache-0002", Some(cache_shard_body("other", 2)))
            ),
            None
        );
    }

    #[test]
    fn recognizes_managed_nar_assets() {
        assert!(is_nar_asset_name("hash.nar"));
        assert!(is_nar_asset_name("hash.nar.xz"));
        assert!(is_nar_asset_name("hash.nar.zst"));
        assert!(!is_nar_asset_name("hash.narinfo"));
        assert!(!is_nar_asset_name("notes.txt"));
    }

    #[test]
    fn allocates_to_the_latest_shard_with_capacity() {
        let shards = vec![
            shard(0, MAX_RELEASE_ASSETS - 1),
            shard(1, MAX_RELEASE_ASSETS),
        ];
        assert_eq!(available_upload_shard(&shards), Some(0));

        let shards = vec![shard(0, MAX_RELEASE_ASSETS), shard(1, 0)];
        assert_eq!(available_upload_shard(&shards), Some(1));

        let shards = vec![shard(0, MAX_RELEASE_ASSETS), shard(1, MAX_RELEASE_ASSETS)];
        assert_eq!(available_upload_shard(&shards), None);
    }

    #[test]
    fn authoritative_site_uses_the_nar_owning_shard() {
        let cache = tempdir().unwrap();
        let output = tempdir().unwrap();
        fs::create_dir(cache.path().join("nar")).unwrap();
        fs::write(cache.path().join("nar/cache.nar.zst"), b"nar").unwrap();
        fs::write(
            cache.path().join("store.narinfo"),
            "StorePath: /nix/store/store-package\nURL: nar/cache.nar.zst\n",
        )
        .unwrap();
        fs::write(output.path().join("stale.narinfo"), "stale").unwrap();
        fs::write(output.path().join("CNAME"), "cache.example.test").unwrap();

        let local = scan_local_cache(cache.path()).unwrap();
        let mut nar_shard = shard(1, 0);
        nar_shard.assets.insert(
            "cache.nar.zst".to_owned(),
            asset(1, "cache.nar.zst".to_owned()),
        );
        let mut info_shard = shard(2, 0);
        info_shard.assets.insert(
            "store.narinfo".to_owned(),
            asset(2, "store.narinfo".to_owned()),
        );
        let shards = vec![nar_shard, info_shard];
        let owners = index_asset_owners(&shards).unwrap();
        let options = PublishOptions {
            cache_dir: cache.path().to_owned(),
            output_dir: Some(output.path().to_owned()),
            owner: "owner".to_owned(),
            repo: "repo".to_owned(),
            token: "token".to_owned(),
            release_tag: "nix-cache".to_owned(),
            store_dir: "/nix/store".to_owned(),
            priority: 30,
            authoritative: true,
        };
        let github =
            GitHub::new("token".to_owned(), "owner".to_owned(), "repo".to_owned()).unwrap();

        generate_static_site(&github, &options, output.path(), &local, &shards, &owners).unwrap();

        assert!(!output.path().join("stale.narinfo").exists());
        assert_eq!(
            fs::read_to_string(output.path().join("CNAME")).unwrap(),
            "cache.example.test"
        );
        let generated = fs::read_to_string(output.path().join("store.narinfo")).unwrap();
        assert!(generated.contains(
            "URL: https://github.com/owner/repo/releases/download/nix-cache-0001/cache.nar.zst"
        ));
    }

    #[test]
    fn failed_generation_preserves_the_existing_index() {
        let cache = tempdir().unwrap();
        let output = tempdir().unwrap();
        fs::create_dir(cache.path().join("nar")).unwrap();
        fs::write(cache.path().join("nar/missing.nar.zst"), b"nar").unwrap();
        fs::write(
            cache.path().join("store.narinfo"),
            "StorePath: /nix/store/store-package\nURL: nar/missing.nar.zst\n",
        )
        .unwrap();
        fs::write(output.path().join("existing.narinfo"), "existing").unwrap();

        let local = scan_local_cache(cache.path()).unwrap();
        let mut info_shard = shard(1, 0);
        info_shard.assets.insert(
            "store.narinfo".to_owned(),
            asset(1, "store.narinfo".to_owned()),
        );
        let shards = vec![info_shard];
        let owners = index_asset_owners(&shards).unwrap();
        let options = PublishOptions {
            cache_dir: cache.path().to_owned(),
            output_dir: Some(output.path().to_owned()),
            owner: "owner".to_owned(),
            repo: "repo".to_owned(),
            token: "token".to_owned(),
            release_tag: "nix-cache".to_owned(),
            store_dir: "/nix/store".to_owned(),
            priority: 30,
            authoritative: true,
        };
        let github =
            GitHub::new("token".to_owned(), "owner".to_owned(), "repo".to_owned()).unwrap();

        assert!(
            generate_static_site(&github, &options, output.path(), &local, &shards, &owners)
                .is_err()
        );
        assert_eq!(
            fs::read_to_string(output.path().join("existing.narinfo")).unwrap(),
            "existing"
        );
    }

    #[test]
    fn rejects_overlapping_cache_and_output_directories() {
        let root = tempdir().unwrap();
        let cache = root.path().join("cache");
        fs::create_dir(&cache).unwrap();

        assert!(ensure_distinct_directories(&cache, &cache.join("site")).is_err());
        assert!(ensure_distinct_directories(&cache, root.path()).is_err());
    }
}
