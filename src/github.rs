use anyhow::{anyhow, bail, Result};
use serde::Deserialize;

#[derive(Deserialize)]
struct Release {
    tag_name: String,
    assets: Vec<Asset>,
}

#[derive(Deserialize)]
struct Asset {
    name: String,
    browser_download_url: String,
}

/// The zip of a repository's latest release
#[derive(Debug)]
pub struct Download {
    /// Tag without its `v` prefix
    pub version: String,
    pub url: String,
}

/// `repo` is `owner/name`, as written in a `GitHub:` update key
pub async fn latest(http: &wreq::Client, repo: &str) -> Result<Download> {
    let (owner, name) = repo
        .split_once('/')
        .ok_or_else(|| anyhow!("Invalid GitHub repo"))?;
    let release = http
        .get(format!(
            "https://api.github.com/repos/{owner}/{name}/releases/latest"
        ))
        .header("Accept", "application/vnd.github+json")
        .send()
        .await?
        .error_for_status()?
        .json::<Release>()
        .await?;
    pick(release)
}

fn pick(release: Release) -> Result<Download> {
    let mut zips: Vec<Asset> = release
        .assets
        .into_iter()
        .filter(|asset| asset.name.to_lowercase().ends_with(".zip"))
        .collect();
    if zips.len() > 1 {
        bail!(
            "Multiple valid assets found: {}",
            zips.iter()
                .map(|asset| asset.name.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        );
    }
    let asset = zips
        .pop()
        .ok_or_else(|| anyhow!("No valid release assets found"))?;
    Ok(Download {
        version: release.tag_name.trim_start_matches(['v', 'V']).to_string(),
        url: asset.browser_download_url,
    })
}

#[cfg(test)]
mod tests {
    use super::{pick, Release};

    fn release(json: &str) -> Release {
        serde_json::from_str(json).unwrap()
    }

    #[test]
    fn picks_the_only_zip_and_strips_the_tag_prefix() {
        let download = pick(release(
            r#"{"tag_name":"v1.2.3","assets":[
            {"name":"notes.txt","browser_download_url":"https://x/notes.txt"},
            {"name":"Mod 1.2.3.ZIP","browser_download_url":"https://x/mod.zip"}]}"#,
        ))
        .unwrap();
        assert_eq!(
            (download.version.as_str(), download.url.as_str()),
            ("1.2.3", "https://x/mod.zip")
        );
    }

    #[test]
    fn rejects_zero_or_several_zips() {
        let none = pick(release(r#"{"tag_name":"1.0","assets":[]}"#)).unwrap_err();
        assert_eq!(none.to_string(), "No valid release assets found");
        let two = pick(release(r#"{"tag_name":"1.0","assets":[
            {"name":"a.zip","browser_download_url":"u"},{"name":"b.zip","browser_download_url":"u"}]}"#)).unwrap_err();
        assert_eq!(two.to_string(), "Multiple valid assets found: a.zip, b.zip");
    }

    /// Run with: cargo test github -- --ignored
    #[tokio::test]
    #[ignore = "integration: hits the real GitHub API"]
    async fn finds_a_real_release() {
        let http = crate::http::client().unwrap();
        // A repo with several zips still proves the request and parsing work, only picking fails
        match super::latest(&http, "Pathoschild/SMAPI").await {
            Ok(download) => println!("{} {}", download.version, download.url),
            Err(e) => assert!(e.to_string().starts_with("Multiple valid assets"), "{e}"),
        }
    }
}
