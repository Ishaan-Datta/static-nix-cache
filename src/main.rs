mod github;
mod narinfo;
mod publish;

use std::path::PathBuf;

use anyhow::{bail, Context, Result};
use clap::{Args, Parser, Subcommand};

#[derive(Debug, Parser)]
#[command(name = "static-nix-cache")]
#[command(about = "Publish a local Nix binary cache without running an HTTP upload server")]
#[command(version)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Publish a file:// Nix binary cache to GitHub Releases and generate static metadata.
    Publish(PublishArgs),
}

#[derive(Debug, Args)]
struct PublishArgs {
    /// Directory produced by `nix copy --to file:///...`.
    #[arg(long)]
    cache_dir: PathBuf,

    /// Directory to populate with nix-cache-info and .narinfo files.
    #[arg(long)]
    output_dir: Option<PathBuf>,

    /// GitHub repository owner. Defaults to the owner from GITHUB_REPOSITORY.
    #[arg(long, env = "GITHUB_OWNER")]
    owner: Option<String>,

    /// GitHub repository name. Defaults to the repo from GITHUB_REPOSITORY.
    #[arg(long, env = "GITHUB_REPO")]
    repo: Option<String>,

    /// GitHub token with contents:write permission.
    #[arg(long, env = "GITHUB_TOKEN", hide_env_values = true)]
    token: String,

    /// GitHub Release tag that stores NARs and persistent .narinfo metadata.
    #[arg(long, env = "GITHUB_RELEASE_TAG", default_value = "nix-cache")]
    release_tag: String,

    /// Logical Nix store directory advertised in nix-cache-info.
    #[arg(long, env = "STORE_DIR", default_value = "/nix/store")]
    store_dir: String,

    /// Binary cache priority; lower values have higher priority in Nix.
    #[arg(long, env = "CACHE_PRIORITY", default_value_t = 30)]
    priority: u32,
}

fn main() {
    if let Err(err) = real_main() {
        eprintln!("error: {err:#}");
        std::process::exit(1);
    }
}

fn real_main() -> Result<()> {
    let cli = Cli::parse();

    match cli.command {
        Command::Publish(args) => {
            let (owner, repo) = resolve_repository(args.owner, args.repo)?;
            let options = publish::PublishOptions {
                cache_dir: args.cache_dir,
                output_dir: args.output_dir,
                owner,
                repo,
                token: args.token,
                release_tag: args.release_tag,
                store_dir: args.store_dir,
                priority: args.priority,
            };
            publish::publish(options)
        }
    }
}

fn resolve_repository(owner: Option<String>, repo: Option<String>) -> Result<(String, String)> {
    match (owner, repo) {
        (Some(owner), Some(repo)) => return Ok((owner, repo)),
        (Some(_), None) | (None, Some(_)) => {
            bail!("--owner and --repo must be provided together")
        }
        (None, None) => {}
    }

    let repository = std::env::var("GITHUB_REPOSITORY")
        .context("set --owner/--repo or GITHUB_REPOSITORY=owner/repo")?;
    let (owner, repo) = repository
        .split_once('/')
        .context("GITHUB_REPOSITORY must have the form owner/repo")?;

    if owner.is_empty() || repo.is_empty() {
        bail!("GITHUB_REPOSITORY must have the form owner/repo");
    }

    Ok((owner.to_owned(), repo.to_owned()))
}
