use std::collections::HashSet;
use std::fmt;
use std::io::IsTerminal;
use std::path::Path;
use anyhow::{anyhow, bail, Result};
use inquire::MultiSelect;
use serde::{Deserialize, Serialize};
use tokio::fs;
use crate::mod_locator::{locate_mods, Mod, UpdateKey, UpdateKeys};

pub const DEFAULT_FILE: &str = "prismatic-mods.json";
const FORMAT: u32 = 1;
/// inquire wraps rows wider than the terminal and corrupts the redraw, so rows are capped well below 80 columns
const LABEL_WIDTH: usize = 60;

#[derive(Debug, Serialize, Deserialize)]
struct ModList {
    format: u32,
    mods: Vec<Entry>,
}

/// `name` and `version` are informational, only `key` is used to install
#[derive(Debug, Clone, Serialize, Deserialize)]
struct Entry {
    key: UpdateKey,
    name: Option<String>,
    version: Option<String>,
}

impl fmt::Display for Entry {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let label = format!("{} {} ({})", self.name.as_deref().unwrap_or("Unnamed mod"), self.version.as_deref().unwrap_or(""), self.key);
        match label.char_indices().nth(LABEL_WIDTH) {
            Some((end, _)) => write!(f, "{}...", &label[..end]),
            None => f.write_str(&label),
        }
    }
}

/// One archive can hold several mods sharing a key, so entries are deduplicated by key.
/// Returns the entries and the names of mods that have no supported key.
fn entries_from(mods: &[Mod]) -> (Vec<Entry>, Vec<String>) {
    let mut seen = HashSet::new();
    let (mut entries, mut unexportable) = (Vec::new(), Vec::new());
    for mod_ in mods {
        match mod_.manifest.update_keys.preferred() {
            Some(key) => if seen.insert(key.to_string().to_lowercase()) {
                entries.push(Entry {
                    key: key.clone(),
                    name: mod_.manifest.name.clone(),
                    version: mod_.manifest.version.clone(),
                });
            },
            None => unexportable.push(mod_.manifest.name.clone().unwrap_or_else(|| mod_.path.display().to_string())),
        }
    }
    (entries, unexportable)
}

/// Everything starts selected, so a subset is made by deselecting.
/// Without a terminal (or with `all`) there is nobody to ask and every entry is kept.
fn pick(entries: Vec<Entry>, prompt: &str, all: bool) -> Result<Vec<Entry>> {
    if all || entries.is_empty() || !(std::io::stdin().is_terminal() && std::io::stderr().is_terminal()) {
        return Ok(entries);
    }
    let defaults: Vec<usize> = (0..entries.len()).collect();
    let picked = MultiSelect::new(prompt, entries)
        .with_default(&defaults)
        .with_page_size(15)
        .raw_prompt()
        .map_err(|e| anyhow!("Cancelled: {e}"))?;
    Ok(picked.into_iter().map(|option| option.value).collect())
}

pub async fn export(mods: &[Mod], file: &Path, all: bool) -> Result<()> {
    if file.exists() {
        bail!("File exists: {}", file.display());
    }
    let (entries, unexportable) = entries_from(mods);
    for name in &unexportable {
        eprintln!("{name}: No update key, can't be exported");
    }
    let entries = pick(entries, "Pick mods to export (space: toggle, right: all, left: none, enter: confirm)", all)?;
    if entries.is_empty() {
        bail!("No mods selected");
    }
    let count = entries.len();
    fs::write(file, serde_json::to_string_pretty(&ModList { format: FORMAT, mods: entries })?).await?;
    println!("Exported {count} mods to {}", file.display());
    Ok(())
}

fn is_installed(mods: &[Mod], key: &UpdateKey) -> bool {
    mods.iter().any(|m| m.manifest.update_keys.iter().any(|k| k.source.eq_ignore_ascii_case(&key.source) && k.id == key.id))
}

#[cfg_attr(not(feature = "nexus"), allow(unused_variables, clippy::unused_async))]
async fn install(mods: &[Mod], mods_dir: &Path, key: &UpdateKey) -> Result<()> {
    match &*key.source.to_lowercase() {
        #[cfg(feature = "nexus")] "nexus" => crate::add::add_nexus(mods, mods_dir, &key.id).await,
        #[cfg(not(feature = "nexus"))] "nexus" => bail!("Prismatic was compiled without Nexus support"),
        "github" => bail!("GitHub installs are not yet implemented"),
        other => bail!("Unsupported source: {other}"),
    }
}

pub async fn import(mods: &[Mod], mods_dir: &Path, file: &Path, all: bool) -> Result<()> {
    let list: ModList = serde_json::from_str(&fs::read_to_string(file).await?)?;
    if list.format != FORMAT {
        bail!("Unsupported mod list format {}, expected {FORMAT}", list.format);
    }
    let entries = pick(list.mods, "Pick mods to install (space: toggle, right: all, left: none, enter: confirm)", all)?;
    let mut mods = mods.to_vec();
    let (mut installed, mut skipped, mut failed) = (0, 0, 0);
    // Sequential: Nexus rate-limits, and a pack can satisfy later entries
    for entry in entries {
        if is_installed(&mods, &entry.key) {
            skipped += 1;
            eprintln!("{entry}: Already installed");
            continue;
        }
        if entry.key.subkey.is_some() {
            eprintln!("{entry}: Subkeys are not yet supported, installing the main file");
        }
        match install(&mods, mods_dir, &entry.key).await {
            Ok(()) => {
                installed += 1;
                mods = locate_mods(mods_dir).await?;
            }
            Err(e) => {
                failed += 1;
                eprintln!("{entry}: {e}");
            }
        }
    }
    eprintln!("{installed} installed, {failed} failed, {skipped} already installed");
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;
    use super::{entries_from, Entry, ModList};
    use crate::mod_locator::{Manifest, Mod, UpdateKey};

    fn key(source: &str, id: &str, subkey: Option<&str>) -> UpdateKey {
        UpdateKey { source: source.into(), id: id.into(), subkey: subkey.map(Into::into) }
    }

    fn mod_(name: &str, keys: Vec<UpdateKey>) -> Mod {
        Mod {
            path: PathBuf::from(name),
            manifest: Manifest {
                name: Some(name.into()),
                author: None,
                version: Some("1.0.0".into()),
                description: None,
                unique_id: None,
                update_keys: keys,
            },
        }
    }

    #[test]
    fn dedupes_packs_and_reports_mods_without_a_key() {
        let mods = [
            mod_("A", vec![key("Nexus", "1", None)]),
            mod_("A (variant)", vec![key("nexus", "1", None)]),
            mod_("B", vec![]),
            mod_("C", vec![key("Nexus", "2", None)]),
        ];
        let (entries, unexportable) = entries_from(&mods);
        assert_eq!(entries.iter().map(|e| e.name.as_deref().unwrap()).collect::<Vec<_>>(), ["A", "C"]);
        assert_eq!(unexportable, ["B"]);
    }

    #[test]
    fn keys_round_trip_as_smapi_strings() {
        let list = ModList { format: 1, mods: vec![Entry { key: key("Nexus", "123", Some("@main")), name: None, version: None }] };
        let json = serde_json::to_string(&list).unwrap();
        assert!(json.contains(r#""key":"Nexus:123@main""#));
        let back: ModList = serde_json::from_str(&json).unwrap();
        assert_eq!(back.mods[0].key.to_string(), "Nexus:123@main");
    }
}
