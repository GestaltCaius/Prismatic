use std::collections::HashSet;
use std::io::Cursor;
use std::path::Path;
use anyhow::{anyhow, bail, Result};
use indicatif::MultiProgress;
use zip::ZipArchive;
use crate::mod_locator::{is_nexus_id, locate_mods, Mod};
use crate::progress::{self, Event, Progress};
use crate::update::nexus_download;

/// Extracts the mod id from a Stardew Valley Nexus URL.
pub fn nexus_id(url: &str) -> Option<&str> {
    let id = url.split("nexusmods.com/stardewvalley/mods/").nth(1)?
        .split(['?', '#', '/']).next()?;
    is_nexus_id(id).then_some(id)
}

pub async fn add(mods: &[Mod], mods_dir: &Path, source: &str) -> Result<()> {
    let id = nexus_id(source).ok_or_else(|| anyhow!("Expected a URL like https://www.nexusmods.com/stardewvalley/mods/<id>"))?;
    let progress = progress::bar(&MultiProgress::new(), &format!("Nexus:{id}"));
    let result = add_nexus(mods, mods_dir, id, &progress).await;
    progress.emit(Event::Done);
    result?.print(mods_dir)
}

pub async fn add_nexus(mods: &[Mod], mods_dir: &Path, id: &str, progress: &Progress) -> Result<Added> {
    if !is_nexus_id(id) {
        bail!("Invalid Nexus mod id: {id}");
    }
    if let Some(installed) = mods.iter().find(|m| m.manifest.update_keys.iter().any(|k| k.source.eq_ignore_ascii_case("nexus") && k.id == id)) {
        bail!("Already installed: {}", installed.path.strip_prefix(mods_dir)?.display());
    }
    let mut zip = ZipArchive::new(Cursor::new(nexus_download(id, progress).await?))?;
    let roots: HashSet<String> = zip.file_names().filter_map(|name| Some(name.split_once('/')?.0.to_string())).collect();
    if roots.is_empty() {
        bail!("Zip archive has no top-level folder");
    }
    if let Some(root) = roots.iter().find(|root| mods_dir.join(root).exists()) {
        bail!("File exists: {root}");
    }
    progress.emit(Event::Stage("Extracting"));
    zip.extract(mods_dir)?;
    progress.emit(Event::Done);

    // One archive can hold several mods (packs) or alternate variants, all are installed
    let new_mods: Vec<_> = locate_mods(mods_dir).await?.into_iter()
        .filter(|m| roots.iter().any(|root| m.path.starts_with(mods_dir.join(root))))
        .collect();
    let mut warnings = Vec::new();
    if new_mods.is_empty() {
        warnings.push(format!("extracted {roots:?} but found no manifest.json"));
    }
    if new_mods.len() > 1 {
        warnings.push(format!("{} mods installed from one archive, delete the ones you don't want", new_mods.len()));
    }
    if !new_mods.is_empty() && !new_mods.iter().any(|m| m.manifest.update_keys.iter().any(|k| k.source.eq_ignore_ascii_case("nexus") && k.id == id)) {
        warnings.push(format!("no Nexus:{id} update key in the manifest(s), `update` will not be able to update this mod"));
    }
    Ok(Added { mods: new_mods, warnings })
}

/// What an install put on disk
pub struct Added {
    pub mods: Vec<Mod>,
    pub warnings: Vec<String>,
}

impl Added {
    /// One line per installed mod: name, version and folder
    pub fn lines(&self, mods_dir: &Path) -> Result<Vec<String>> {
        self.mods.iter().map(|m| Ok(format!(
            "{} {} ({})",
            m.manifest.name.as_deref().unwrap_or("Unnamed mod"),
            m.manifest.version.as_deref().unwrap_or(""),
            m.path.strip_prefix(mods_dir)?.display(),
        ))).collect()
    }

    pub fn print(&self, mods_dir: &Path) -> Result<()> {
        for line in self.lines(mods_dir)? {
            println!("{line}");
        }
        for warning in &self.warnings {
            eprintln!("Warning: {warning}");
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::nexus_id;

    #[test]
    fn parses_nexus_urls() {
        assert_eq!(nexus_id("https://www.nexusmods.com/stardewvalley/mods/3109?tab=files"), Some("3109"));
        assert_eq!(nexus_id("nexusmods.com/stardewvalley/mods/12/"), Some("12"));
        assert_eq!(nexus_id("https://www.nexusmods.com/skyrim/mods/1"), None);
        assert_eq!(nexus_id("https://www.nexusmods.com/stardewvalley/mods/abc"), None);
    }
}
