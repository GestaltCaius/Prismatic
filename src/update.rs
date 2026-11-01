use std::collections::HashSet;
use std::io::Cursor;
use std::path::{PathBuf};
use anyhow::{anyhow, bail, Result};
use async_recursion::async_recursion;
use bytes::Bytes;
#[cfg(feature = "nexus")]
use scraper::{Html, Selector};
use serde_json::json;
use tokio::fs;
use tokio::sync::OnceCell;
use tokio::task::JoinSet;
use zip::ZipArchive;
use crate::mod_locator::{Mod, UpdateKey, UpdateKeys};

#[cfg(feature = "nexus")]
static NEXUS_SESSION: OnceCell<String> = OnceCell::const_new();
#[cfg(feature = "nexus")]
static NEXUS_SEMAPHORE: tokio::sync::Semaphore = tokio::sync::Semaphore::const_new(2);
static REQWEST: OnceCell<wreq::Client> = OnceCell::const_new();

pub async fn reqwest_client() -> Result<&'static wreq::Client> {
    REQWEST.get_or_try_init(|| async {
        // Emulate Chrome TLS/HTTP2 fingerprint so Cloudflare doesn't block us
        Result::<_, anyhow::Error>::Ok(wreq::Client::builder()
            .emulation(wreq_util::emulate::Emulation::Chrome136)
            .build()?)
    }).await
}

pub async fn update(mods: &Vec<Mod>, mods_dir: &PathBuf, force: bool) -> Result<()> {
    let mut updated = 0;
    let mut failed = 0;
    let mut skipped = 0;
    let mut set: JoinSet<(Mod, UpdateResult)> = JoinSet::new();
    for mod_ in mods {
        let name = mod_.manifest.name.clone().unwrap_or(mod_.path.strip_prefix(mods_dir)?.display().to_string());
        if let Some(source) = mod_.manifest.update_keys.preferred() {
            if source.subkey.is_some() {
                failed += 1;
                eprintln!("{name}: Subkeys are not yet supported");
                continue;
            }
            let (m, source) = (mod_.clone(), source.clone());
            match &*source.source.to_lowercase() {
                #[cfg(feature = "github")] "github" => {set.spawn(async move { github(m.clone(), source, force).await.unwrap_or_else(|e| (m, UpdateResult::Failure(e.to_string()))) });}
                #[cfg(feature = "nexus")] "nexus" => {set.spawn(async move { nexus(m.clone(), source, force).await.unwrap_or_else(|e| (m, UpdateResult::Failure(e.to_string()))) });}
                _ => unreachable!()
            }
        } else {
            failed += 1;
            eprintln!("{name}: Unknown or unsupported source");
        }
    }
    while let Some(res) = set.join_next().await {
        let (finished, result) = res?;
        let name = finished.manifest.name.unwrap_or(finished.path.strip_prefix(mods_dir)?.display().to_string());
        match result {
            UpdateResult::Success { new_version } => {
                updated += 1;
                if let Some(old_version) = finished.manifest.version {
                    println!("{name}: {old_version} -> {new_version}");
                } else {
                    println!("{name}: {new_version}");
                }
            }
            UpdateResult::AlreadyUpToDate => {
                skipped += 1;
                eprintln!("{name}: Already up to date");
            }
            UpdateResult::Failure(message) => {
                failed += 1;
                eprintln!("{name}: {message}");
            }
        }
    }
    if force {
        eprintln!("{updated} updated, {failed} failed");
    } else {
        eprintln!("{updated} updated, {failed} failed, {skipped} already up to date");
    }
    Ok(())
}

enum UpdateResult {
    Success {
        new_version: String,
    },
    AlreadyUpToDate,
    Failure(String),
}

async fn check_for_updates(mod_: &Mod) -> Result<bool> {
    let Some(key) = mod_.manifest.update_keys.preferred() else {bail!("No preferred update key")};
    Ok(reqwest_client().await?
        .post("https://smapi.io/api/v3.0/mods")
        .json(&json!(
            {
                "mods": [
                    {
                        "id": mod_.manifest.unique_id.clone().unwrap_or_else(|| format!("FAKE.{}.{}", key.source, key.id)),
                        "updateKeys": [ format!("{}:{}", key.source, key.id) ],
                        "installedVersion": mod_.manifest.version.clone().unwrap_or_default(),
                    },
                ],
                "apiVersion": "4.0.7",  // TODO: Use actual version
            }
        ))
        .send().await?
        .error_for_status()?
        .json::<serde_json::Value>().await?
        .as_array().ok_or_else(|| anyhow!("Expected array"))?[0]
        .as_object().ok_or_else(|| anyhow!("Expected object"))?
        .contains_key("suggestedUpdate")
    )
}

#[cfg(feature = "github")]
async fn github(mod_: Mod, update_key: UpdateKey, force: bool) -> Result<(Mod, UpdateResult)> {
    if !force && !check_for_updates(&mod_).await? {
        return Ok((mod_, UpdateResult::AlreadyUpToDate));
    }
    let (owner, repo) = update_key.id.split_once('/').ok_or_else(|| anyhow!("Invalid GitHub repo"))?;
    let release = octocrab::instance()
        .repos(owner, repo)
        .releases()
        .get_latest().await?;
    let version = release.tag_name.trim_start_matches(['v', 'V']);
    if !force && mod_.manifest.version == Some(version.into()) {
        return Ok((mod_, UpdateResult::AlreadyUpToDate));
    }
    let assets: Vec<_> = release.assets.iter()
        .filter(|asset| asset.name.to_lowercase().ends_with(".zip"))
        .collect();
    if assets.is_empty() {
        return Ok((mod_, UpdateResult::Failure("No valid release assets found".into())));
    }
    if assets.len() > 1 {
        return Ok((mod_, UpdateResult::Failure(format!("Multiple valid assets found: {assets:?}"))));
    }
    let asset = reqwest_client().await?
        .get(assets[0].browser_download_url.as_str())
        .send().await?
        .error_for_status()?
        .bytes().await?;
    install(&mod_, asset).await?;
    Ok((mod_, UpdateResult::Success {new_version: version.into()}))
}

#[cfg(feature = "nexus")]
async fn nexus(mod_: Mod, update_key: UpdateKey, force: bool) -> Result<(Mod, UpdateResult)> {
    if !force && !check_for_updates(&mod_).await? {
        return Ok((mod_, UpdateResult::AlreadyUpToDate));
    }
    let file = nexus_download(&update_key.id).await?;
    install(&mod_, file).await?;
    Ok((mod_, UpdateResult::Success {new_version: "latest".into()}))  // TODO: Determine actual new version
}

/// Downloads the main file of a Stardew Valley Nexus mod.
#[cfg(feature = "nexus")]
pub async fn nexus_download(mod_id: &str) -> Result<Bytes> {
    // nexusmods_session: NEXUS_COOKIE env var (bare value or full cookie string) or browser via rookie
    let session = NEXUS_SESSION.get_or_try_init(|| async {
        if let Ok(val) = std::env::var("NEXUS_COOKIE") {
            return Ok(val.split(';')
                .find_map(|p| p.trim().strip_prefix("nexusmods_session=").map(str::to_string))
                .unwrap_or(val));
        }
        Result::<_, anyhow::Error>::Ok(
            rookie::load(Some(vec!["nexusmods.com".to_string()])).map_err(|e| anyhow!("{e}"))?.iter()
                .find(|c| c.name == "nexusmods_session")
                .ok_or_else(|| anyhow!("No nexusmods_session found. Set the NEXUS_COOKIE environment variable or sign in to Nexus in your browser."))?
                .value.clone()
        )
    }).await?;
    // Limit concurrency to avoid Nexus rate-limiting
    let _permit = NEXUS_SEMAPHORE.acquire().await?;
    // wreq emulates Chrome TLS/HTTP2 fingerprint — Cloudflare accepts without cf_clearance
    let client = reqwest_client().await?;
    let cookie = format!("nexusmods_session={session}");
    let files_url = format!("https://www.nexusmods.com/stardewvalley/mods/{mod_id}?tab=files");
    let file_id = {
        let html = client.get(&files_url)
            .header("Cookie", &cookie)
            .header("Referer", "https://www.nexusmods.com/stardewvalley/mods/")
            .send().await?.error_for_status()?.text().await?;
        let document = Html::parse_document(&html);
        let main_files = Selector::parse("#file-container-main-files .file-expander-header").map_err(|e| anyhow!("{e}"))?;
        let update_files = Selector::parse("#file-container-update-files .file-expander-header").map_err(|e| anyhow!("{e}"))?;
        document.select(&main_files)
            .next()
            .or_else(|| document.select(&update_files).next())
            .ok_or_else(|| anyhow!("HTML parsing failed: couldn't locate main file element"))?
            .attr("data-id").ok_or_else(|| anyhow!("HTML parsing failed: couldn't find data-id attribute"))?
            .to_string()
    };
    let dl_resp = client
        .post("https://www.nexusmods.com/Core/Libs/Common/Managers/Downloads?GenerateDownloadUrl")
        .form(&json!({"fid": file_id, "game_id": 1303, "collection_id": 0}))
        .header("Cookie", &cookie)
        .header("Referer", &files_url)
        .header("X-Requested-With", "XMLHttpRequest")
        .header("Origin", "https://www.nexusmods.com")
        .send().await?
        .error_for_status()?
        .json::<serde_json::Value>().await?;
    let download_url = dl_resp
        .get("url").ok_or_else(|| anyhow!("GenerateDownloadUrl unexpected response: {dl_resp}"))?
        .as_str().ok_or_else(|| anyhow!("GenerateDownloadUrl: url is not a string"))?
        .to_string();
    Ok(client.get(&download_url).header("Cookie", &cookie).send().await?.error_for_status()?.bytes().await?)
}

async fn install(mod_: &Mod, archive: Bytes) -> Result<()> {
    let old_dir = mod_.path.join("../.old/");
    if !old_dir.is_dir() {
        fs::create_dir(&old_dir).await?;
    }
    let old_dir = old_dir.canonicalize()?;
    let dir_name = mod_.path.file_name().ok_or_else(|| anyhow!("Mod path has no directory name"))?;
    let old_subdir = old_dir.join(format!("{} - {}", dir_name.to_string_lossy(), mod_.manifest.version.clone().unwrap_or_else(|| "unknown".into())));
    if old_subdir.is_dir() {
        bail!("File exists: {}", old_subdir.display());
    }
    let mut zip = ZipArchive::new(Cursor::new(archive))?;
    if zip.is_empty() {
        bail!("Zip archive is empty");
    }
    let base: HashSet<_> = zip.file_names().filter_map(|name| Some(name.split_once('/')?.0)).collect();
    let mut base = base.into_iter();
    let (Some(base), None) = (base.next(), base.next()) else {
        bail!("Zip archive must contain exactly one item in the root");
    };
    let new_dir = mod_.path.with_file_name(base);
    fs::rename(&mod_.path, &old_subdir).await?;
    if new_dir.exists() {
        bail!("File exists: {}", base);
    }
    zip.extract(mod_.path.parent().ok_or_else(|| anyhow!("Mod path has no parent directory"))?)?;
    for file in old_subdir
        .read_dir()?
        .flatten()
        .filter(|file| ["config.json", "config"].contains(&&*file.file_name().to_string_lossy()))
    {
        let new_path = new_dir.join(file.path().strip_prefix(&old_subdir)?);
        if new_path.exists() {
            fs::rename(&new_path, new_path.with_file_name(format!("{}.new", file.file_name().to_string_lossy()))).await?;
        }
        copy(&file.path(), &new_path).await?;
    }
    Ok(())
}

#[async_recursion]
async fn copy(from: &PathBuf, to: &PathBuf) -> Result<()> {
    if from.is_file() {
        fs::copy(from, to).await?;
    } else if from.is_dir() {
        fs::create_dir(&to).await?;
        for file in from.read_dir()?.flatten() {
            copy(&file.path(), &to.join(file.file_name())).await?;
        }
    } else {
        bail!("{} is neither a file nor a directory", from.display());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mod_locator::locate_mods;
    use tokio::fs;

    /// Downloads Pony Weight Loss Program (11K) and `AutomaticGates` (33K) into `$TMPDIR`.
    /// Run with: cargo test -- --ignored
    /// Requires `NEXUS_COOKIE` env var (`nexusmods_session` value or full cookie string).
    #[tokio::test]
    #[ignore = "integration: hits real Nexus, requires NEXUS_COOKIE"]
    async fn nexus_downloads_small_mods() {
        assert!(std::env::var("NEXUS_COOKIE").is_ok(), "NEXUS_COOKIE not set");

        let test_dir = std::env::temp_dir().join("prismatic-test");
        if test_dir.exists() { fs::remove_dir_all(&test_dir).await.unwrap(); }
        fs::create_dir_all(&test_dir).await.unwrap();

        for (name, nexus_id) in [("Pony Weight Loss Program", "1232"), ("AutomaticGates", "3109")] {
            let mod_dir = test_dir.join(name);
            fs::create_dir(&mod_dir).await.unwrap();
            // version 0.0.1 ensures SMAPI reports an update; force=true skips the check anyway
            fs::write(mod_dir.join("manifest.json"), format!(
                r#"{{"Name":"{name}","Author":"test","Version":"0.0.1","UniqueID":"test.{nexus_id}","UpdateKeys":["Nexus:{nexus_id}"]}}"#
            )).await.unwrap();
        }

        let mods = locate_mods(&test_dir).await.expect("locate_mods failed");
        assert_eq!(mods.len(), 2, "expected 2 mods in test dir");

        update(&mods, &test_dir, true).await.expect("update returned Err");

        let old_dir = test_dir.join(".old");
        assert!(old_dir.is_dir(), ".old/ missing — no mods were installed");
        let backed_up = old_dir.read_dir().unwrap().flatten().count();
        assert_eq!(backed_up, 2, "expected both mods backed up in .old/, got {backed_up}");
    }
}
