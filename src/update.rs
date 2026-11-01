use std::collections::HashSet;
use std::io::Cursor;
use std::path::{Path, PathBuf};
use anyhow::{anyhow, bail, Result};
use indicatif::MultiProgress;
use async_recursion::async_recursion;
use bytes::Bytes;
use futures_util::StreamExt;
#[cfg(feature = "nexus")]
use scraper::{Html, Selector};
use serde_json::json;
use tokio::fs;
use tokio::sync::OnceCell;
use tokio::task::JoinSet;
use zip::ZipArchive;
use crate::mod_locator::{locate_mods_at, Mod, UpdateKey, UpdateKeys};
use crate::progress::{self, Event, Progress};

#[cfg(feature = "nexus")]
static NEXUS_SESSION: OnceCell<String> = OnceCell::const_new();
#[cfg(feature = "nexus")]
static NEXUS_SEMAPHORE: tokio::sync::Semaphore = tokio::sync::Semaphore::const_new(2);
/// Cookie pasted in the web UI, it takes precedence over the environment and the browser
#[cfg(feature = "nexus")]
static NEXUS_COOKIE_OVERRIDE: std::sync::RwLock<Option<String>> = std::sync::RwLock::new(None);
static REQWEST: OnceCell<wreq::Client> = OnceCell::const_new();

/// `nexusmods_session` out of a bare value or a full cookie string
#[cfg(feature = "nexus")]
fn session_from(val: &str) -> String {
    val.split(';')
        .find_map(|p| p.trim().strip_prefix("nexusmods_session=").map(str::to_string))
        .unwrap_or_else(|| val.trim().to_string())
}

#[cfg(feature = "nexus")]
pub fn set_nexus_cookie(value: &str) {
    if let Ok(mut cookie) = NEXUS_COOKIE_OVERRIDE.write() {
        *cookie = Some(session_from(value));
    }
}

/// Drops the pasted cookie so the environment and the browser are used again
#[cfg(feature = "nexus")]
pub fn clear_nexus_cookie() {
    if let Ok(mut cookie) = NEXUS_COOKIE_OVERRIDE.write() {
        *cookie = None;
    }
}

/// Where the Nexus cookie will come from, without touching the browser
#[cfg(feature = "nexus")]
pub fn nexus_cookie_source() -> Option<&'static str> {
    if NEXUS_COOKIE_OVERRIDE.read().is_ok_and(|cookie| cookie.is_some()) {
        Some("pasted")
    } else if std::env::var("NEXUS_COOKIE").is_ok() {
        Some("environment")
    } else {
        None
    }
}

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
    let multi = MultiProgress::new();
    for mod_ in mods {
        let name = mod_.manifest.name.clone().unwrap_or(mod_.path.strip_prefix(mods_dir)?.display().to_string());
        if mod_.manifest.update_keys.preferred().is_some() {
            let (m, progress) = (mod_.clone(), progress::bar(&multi, &name));
            set.spawn(async move {
                let result = update_mod(&m, force, &progress).await;
                (m, result)
            });
        } else {
            failed += 1;
            multi.suspend(|| eprintln!("{name}: Unknown or unsupported source"));
        }
    }
    while let Some(res) = set.join_next().await {
        let (finished, result) = res?;
        let name = finished.manifest.name.unwrap_or(finished.path.strip_prefix(mods_dir)?.display().to_string());
        match result {
            UpdateResult::Success { new_version } => {
                updated += 1;
                if let Some(old_version) = finished.manifest.version {
                    multi.suspend(|| println!("{name}: {old_version} -> {new_version}"));
                } else {
                    multi.suspend(|| println!("{name}: {new_version}"));
                }
            }
            UpdateResult::AlreadyUpToDate => {
                skipped += 1;
                multi.suspend(|| eprintln!("{name}: Already up to date"));
            }
            UpdateResult::Failure(message) => {
                failed += 1;
                multi.suspend(|| eprintln!("{name}: {message}"));
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

pub enum UpdateResult {
    Success {
        new_version: String,
    },
    AlreadyUpToDate,
    Failure(String),
}

/// Updates one mod from its preferred update key
pub async fn update_mod(mod_: &Mod, force: bool, progress: &Progress) -> UpdateResult {
    let Some(key) = mod_.manifest.update_keys.preferred().cloned() else {
        return UpdateResult::Failure("Unknown or unsupported source".into());
    };
    if key.subkey.is_some() {
        return UpdateResult::Failure("Subkeys are not yet supported".into());
    }
    let result = match &*key.source.to_lowercase() {
        #[cfg(feature = "github")] "github" => github(mod_, key, force, progress).await,
        #[cfg(feature = "nexus")] "nexus" => nexus(mod_, key, force, progress).await,
        _ => Err(anyhow!("Unsupported source: {}", key.source)),
    };
    progress.emit(Event::Done);
    result.unwrap_or_else(|e| UpdateResult::Failure(e.to_string()))
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
async fn github(mod_: &Mod, update_key: UpdateKey, force: bool, progress: &Progress) -> Result<UpdateResult> {
    if !force && !check_for_updates(mod_).await? {
        return Ok(UpdateResult::AlreadyUpToDate);
    }
    let (owner, repo) = update_key.id.split_once('/').ok_or_else(|| anyhow!("Invalid GitHub repo"))?;
    let release = octocrab::instance()
        .repos(owner, repo)
        .releases()
        .get_latest().await?;
    let version = release.tag_name.trim_start_matches(['v', 'V']);
    if !force && mod_.manifest.version == Some(version.into()) {
        return Ok(UpdateResult::AlreadyUpToDate);
    }
    let assets: Vec<_> = release.assets.iter()
        .filter(|asset| asset.name.to_lowercase().ends_with(".zip"))
        .collect();
    if assets.is_empty() {
        return Ok(UpdateResult::Failure("No valid release assets found".into()));
    }
    if assets.len() > 1 {
        return Ok(UpdateResult::Failure(format!("Multiple valid assets found: {assets:?}")));
    }
    progress.emit(Event::Stage("Starting download"));
    let response = reqwest_client().await?
        .get(assets[0].browser_download_url.as_str())
        .send().await?
        .error_for_status()?;
    let asset = read_body(response, progress).await?;
    let new_dir = install(mod_, asset, progress).await?;
    let new_version = installed_version(&new_dir, mod_.manifest.unique_id.as_deref()).await.unwrap_or_else(|| version.into());
    Ok(UpdateResult::Success {new_version})
}

#[cfg(feature = "nexus")]
async fn nexus(mod_: &Mod, update_key: UpdateKey, force: bool, progress: &Progress) -> Result<UpdateResult> {
    if !force && !check_for_updates(mod_).await? {
        return Ok(UpdateResult::AlreadyUpToDate);
    }
    let file = nexus_download(&update_key.id, progress).await?;
    let new_dir = install(mod_, file, progress).await?;
    let new_version = installed_version(&new_dir, mod_.manifest.unique_id.as_deref()).await.unwrap_or_else(|| "unknown".into());
    Ok(UpdateResult::Success {new_version})
}

/// Version in the manifest of a freshly installed mod, picking the one matching `unique_id` out of a pack
async fn installed_version(dir: &Path, unique_id: Option<&str>) -> Option<String> {
    let mods = locate_mods_at(dir).await.ok()?;
    mods.iter()
        .find(|m| m.manifest.unique_id.as_deref().zip(unique_id).is_some_and(|(a, b)| a.eq_ignore_ascii_case(b)))
        .or_else(|| mods.first())?
        .manifest.version.clone()
}

/// Reads a response body, reporting progress as it arrives
async fn read_body(response: wreq::Response, progress: &Progress) -> Result<Bytes> {
    let total = response.content_length();
    let mut body = Vec::new();
    let mut stream = response.bytes_stream();
    while let Some(chunk) = stream.next().await {
        body.extend_from_slice(&chunk?);
        progress.emit(Event::Download { done: body.len() as u64, total });
    }
    Ok(body.into())
}

/// Downloads the main file of a Stardew Valley Nexus mod.
#[cfg(feature = "nexus")]
pub async fn nexus_download(mod_id: &str, progress: &Progress) -> Result<Bytes> {
    // nexusmods_session: NEXUS_COOKIE env var (bare value or full cookie string) or browser via rookie
    let pasted = NEXUS_COOKIE_OVERRIDE.read().ok().and_then(|cookie| cookie.clone());
    let session = if let Some(pasted) = pasted {
        pasted
    } else {
        NEXUS_SESSION.get_or_try_init(|| async {
            if let Ok(val) = std::env::var("NEXUS_COOKIE") {
                return Ok(session_from(&val));
            }
            Result::<_, anyhow::Error>::Ok(
                rookie::load(Some(vec!["nexusmods.com".to_string()])).map_err(|e| anyhow!("{e}"))?.iter()
                    .find(|c| c.name == "nexusmods_session")
                    .ok_or_else(|| anyhow!("No nexusmods_session found. Set the NEXUS_COOKIE environment variable or sign in to Nexus in your browser."))?
                    .value.clone()
            )
        }).await?.clone()
    };
    // Limit concurrency to avoid Nexus rate-limiting
    progress.emit(Event::Stage("Waiting for a download slot"));
    let _permit = NEXUS_SEMAPHORE.acquire().await?;
    // wreq emulates Chrome TLS/HTTP2 fingerprint — Cloudflare accepts without cf_clearance
    let client = reqwest_client().await?;
    let cookie = format!("nexusmods_session={session}");
    let files_url = format!("https://www.nexusmods.com/stardewvalley/mods/{mod_id}?tab=files");
    progress.emit(Event::Stage("Finding the file"));
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
    progress.emit(Event::Stage("Requesting the download link"));
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
    let response = client.get(&download_url).header("Cookie", &cookie).send().await?.error_for_status()?;
    read_body(response, progress).await
}

async fn install(mod_: &Mod, archive: Bytes, progress: &Progress) -> Result<PathBuf> {
    progress.emit(Event::Stage("Extracting"));
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
    Ok(new_dir)
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
