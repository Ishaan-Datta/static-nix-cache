use std::fs::File;
use std::path::Path;

use anyhow::{bail, Context, Result};
use reqwest::blocking::{Body, Client, RequestBuilder};
use reqwest::header::{ACCEPT, CONTENT_LENGTH, CONTENT_TYPE, USER_AGENT};
use reqwest::StatusCode;
use serde::{Deserialize, Serialize};
use url::Url;

const API_VERSION: &str = "2022-11-28";

#[derive(Debug, Clone, Deserialize)]
pub struct Release {
    pub id: u64,
    pub tag_name: String,
    pub body: Option<String>,
    pub draft: bool,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Asset {
    pub id: u64,
    pub name: String,
    pub size: u64,
    pub url: String,
}

#[derive(Debug, Serialize)]
struct CreateRelease<'a> {
    tag_name: &'a str,
    name: String,
    body: &'a str,
    draft: bool,
    prerelease: bool,
}

#[derive(Debug, Serialize)]
struct RenameAsset<'a> {
    name: &'a str,
}

#[derive(Debug)]
pub struct GitHub {
    client: Client,
    token: String,
    owner: String,
    repo: String,
}

impl GitHub {
    pub fn new(token: String, owner: String, repo: String) -> Result<Self> {
        let client = Client::builder()
            .redirect(reqwest::redirect::Policy::limited(10))
            .build()
            .context("building HTTP client")?;
        Ok(Self {
            client,
            token,
            owner,
            repo,
        })
    }

    pub fn get_or_create_release(&self, tag: &str, body: &str) -> Result<Release> {
        let lookup = self.api_url(&["releases", "tags", tag])?;
        let response = self.send(self.get(lookup))?;

        if response.status().is_success() {
            return response
                .json()
                .context("parsing GitHub release lookup response");
        }

        if response.status() != StatusCode::NOT_FOUND {
            return response_error(response, "looking up GitHub release");
        }

        eprintln!("release {tag:?} not found; creating it");
        let create_url = self.api_url(&["releases"])?;
        let payload = CreateRelease {
            tag_name: tag,
            name: format!("Nix Binary Cache ({tag})"),
            body,
            draft: false,
            prerelease: false,
        };
        let response = self.send(self.post(create_url).json(&payload))?;

        if response.status().is_success() {
            return response.json().context("parsing created GitHub release");
        }

        // Matrix jobs can race to create the same release. GitHub reports that
        // as 422; retry the lookup and use the release created by the other job.
        if response.status() == StatusCode::UNPROCESSABLE_ENTITY {
            let lookup = self.api_url(&["releases", "tags", tag])?;
            let retry = self.send(self.get(lookup))?;
            if retry.status().is_success() {
                return retry
                    .json()
                    .context("parsing GitHub release after concurrent creation");
            }
            return response_error(retry, "looking up concurrently created GitHub release");
        }

        response_error(response, "creating GitHub release")
    }

    pub fn list_releases(&self) -> Result<Vec<Release>> {
        let mut all = Vec::new();
        let mut page = 1_u32;

        loop {
            let mut url = self.api_url(&["releases"])?;
            url.query_pairs_mut()
                .append_pair("per_page", "100")
                .append_pair("page", &page.to_string());

            let response = self.send(self.get(url))?;
            if !response.status().is_success() {
                return response_error(response, "listing GitHub releases");
            }

            let releases: Vec<Release> = response
                .json()
                .context("parsing GitHub releases response")?;
            let count = releases.len();
            all.extend(releases);

            if count < 100 {
                break;
            }
            page += 1;
        }

        Ok(all)
    }

    pub fn list_assets(&self, release_id: u64) -> Result<Vec<Asset>> {
        let mut all = Vec::new();
        let mut page = 1_u32;

        loop {
            let mut url = self.api_url(&["releases", &release_id.to_string(), "assets"])?;
            url.query_pairs_mut()
                .append_pair("per_page", "100")
                .append_pair("page", &page.to_string());

            let response = self.send(self.get(url))?;
            if !response.status().is_success() {
                return response_error(response, "listing GitHub release assets");
            }

            let assets: Vec<Asset> = response
                .json()
                .context("parsing GitHub release assets response")?;
            let count = assets.len();
            all.extend(assets);

            if count < 100 {
                break;
            }
            page += 1;
        }

        Ok(all)
    }

    pub fn delete_asset(&self, asset_id: u64) -> Result<()> {
        let url = self.api_url(&["releases", "assets", &asset_id.to_string()])?;
        let response = self.send(self.delete(url))?;
        if response.status().is_success() {
            return Ok(());
        }
        response_error(response, "deleting GitHub release asset")
    }

    pub fn rename_asset(&self, asset_id: u64, name: &str) -> Result<Asset> {
        let url = self.api_url(&["releases", "assets", &asset_id.to_string()])?;
        let response = self.send(self.patch(url).json(&RenameAsset { name }))?;
        if response.status().is_success() {
            return response.json().context("parsing renamed GitHub asset");
        }
        response_error(
            response,
            &format!("renaming GitHub release asset to {name}"),
        )
    }

    pub fn upload_file(
        &self,
        release_id: u64,
        name: &str,
        path: &Path,
        content_type: &str,
    ) -> Result<Asset> {
        let file = File::open(path)
            .with_context(|| format!("opening release asset {}", path.display()))?;
        let size = file
            .metadata()
            .with_context(|| format!("stat release asset {}", path.display()))?
            .len();

        let url = self.upload_url(release_id, name)?;
        let response = self.send(
            self.post(url)
                .header(CONTENT_TYPE, content_type)
                .header(CONTENT_LENGTH, size.to_string())
                .body(Body::new(file)),
        )?;

        if response.status().is_success() {
            return response.json().context("parsing uploaded GitHub asset");
        }

        response_error(response, &format!("uploading release asset {name}"))
    }

    pub fn upload_bytes(
        &self,
        release_id: u64,
        name: &str,
        bytes: Vec<u8>,
        content_type: &str,
    ) -> Result<Asset> {
        let size = bytes.len() as u64;
        let url = self.upload_url(release_id, name)?;
        let response = self.send(
            self.post(url)
                .header(CONTENT_TYPE, content_type)
                .header(CONTENT_LENGTH, size.to_string())
                .body(bytes),
        )?;

        if response.status().is_success() {
            return response.json().context("parsing uploaded GitHub asset");
        }

        response_error(response, &format!("uploading release asset {name}"))
    }

    pub fn download_asset(&self, asset: &Asset) -> Result<Vec<u8>> {
        let url = Url::parse(&asset.url).context("invalid asset API URL from GitHub")?;
        let response = self.send(self.get_binary(url))?;
        if !response.status().is_success() {
            return response_error(
                response,
                &format!("downloading release asset {}", asset.name),
            );
        }
        Ok(response
            .bytes()
            .context("reading GitHub release asset")?
            .to_vec())
    }

    pub fn release_download_url(&self, tag: &str, asset_name: &str) -> Result<String> {
        let mut url = Url::parse("https://github.com/")?;
        {
            let mut segments = url
                .path_segments_mut()
                .map_err(|_| anyhow::anyhow!("github.com URL cannot be a base URL"))?;
            segments
                .push(&self.owner)
                .push(&self.repo)
                .push("releases")
                .push("download")
                .push(tag)
                .push(asset_name);
        }
        Ok(url.to_string())
    }

    fn api_url(&self, suffix: &[&str]) -> Result<Url> {
        let mut url = Url::parse("https://api.github.com/")?;
        {
            let mut segments = url
                .path_segments_mut()
                .map_err(|_| anyhow::anyhow!("GitHub API URL cannot be a base URL"))?;
            segments.push("repos").push(&self.owner).push(&self.repo);
            for segment in suffix {
                segments.push(segment);
            }
        }
        Ok(url)
    }

    fn upload_url(&self, release_id: u64, name: &str) -> Result<Url> {
        let mut url = Url::parse("https://uploads.github.com/")?;
        {
            let mut segments = url
                .path_segments_mut()
                .map_err(|_| anyhow::anyhow!("GitHub uploads URL cannot be a base URL"))?;
            segments
                .push("repos")
                .push(&self.owner)
                .push(&self.repo)
                .push("releases")
                .push(&release_id.to_string())
                .push("assets");
        }
        url.query_pairs_mut().append_pair("name", name);
        Ok(url)
    }

    fn get(&self, url: Url) -> RequestBuilder {
        self.client
            .get(url)
            .header(ACCEPT, "application/vnd.github+json")
    }

    fn get_binary(&self, url: Url) -> RequestBuilder {
        self.client
            .get(url)
            .header(ACCEPT, "application/octet-stream")
    }

    fn post(&self, url: Url) -> RequestBuilder {
        self.client
            .post(url)
            .header(ACCEPT, "application/vnd.github+json")
    }

    fn patch(&self, url: Url) -> RequestBuilder {
        self.client
            .patch(url)
            .header(ACCEPT, "application/vnd.github+json")
    }

    fn delete(&self, url: Url) -> RequestBuilder {
        self.client
            .delete(url)
            .header(ACCEPT, "application/vnd.github+json")
    }

    fn send(&self, request: RequestBuilder) -> Result<reqwest::blocking::Response> {
        request
            .bearer_auth(&self.token)
            .header(USER_AGENT, "static-nix-cache-rust")
            .header("X-GitHub-Api-Version", API_VERSION)
            .send()
            .context("calling GitHub API")
    }
}

fn response_error<T>(response: reqwest::blocking::Response, operation: &str) -> Result<T> {
    let status = response.status();
    let body = response
        .text()
        .unwrap_or_else(|_| "<unable to read response body>".to_owned());
    bail!("{operation} failed: HTTP {status}: {body}")
}
