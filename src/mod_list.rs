use std::collections::{HashMap, HashSet};
use std::fmt;
use std::io::IsTerminal;
use std::path::Path;
use anyhow::{anyhow, bail, Result};
use indicatif::MultiProgress;
use inquire::MultiSelect;
use serde::{Deserialize, Serialize};
use tokio::fs;
use crate::mod_locator::{is_nexus_id, locate_mods, Mod, UpdateKey};
use crate::progress::{self, Event, Progress};
use crate::smapi;

pub const DEFAULT_FILE: &str = "prismatic-mods.json";
const FORMAT: u32 = 2;
/// inquire wraps rows wider than the terminal and corrupts the redraw, so rows are capped well below 80 columns
const LABEL_WIDTH: usize = 60;

#[derive(Debug, Serialize, Deserialize)]
struct ModList {
    format: u32,
    mods: Vec<Entry>,
}

/// `name` and `version` are informational. `keys` holds every update key the mod declares,
/// installable ones first, and import installs from the first one it supports.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct Entry {
    #[serde(default)]
    keys: Vec<UpdateKey>,
    /// Format 1 files had a single `key`, it is folded into `keys` when importing
    #[serde(default, skip_serializing)]
    key: Option<UpdateKey>,
    name: Option<String>,
    version: Option<String>,
    unique_id: Option<String>,
}

impl Entry {
    fn describe(&self) -> String {
        let key = self.keys.first().map_or_else(|| "no key".to_string(), ToString::to_string);
        format!("{} {} ({key})", self.name.as_deref().unwrap_or("Unnamed mod"), self.version.as_deref().unwrap_or(""))
    }
}

/// Truncated, for the picker only
impl fmt::Display for Entry {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let label = self.describe();
        match label.char_indices().nth(LABEL_WIDTH) {
            Some((end, _)) => write!(f, "{}...", &label[..end]),
            None => f.write_str(&label),
        }
    }
}

fn is_source(key: &UpdateKey, source: &str) -> bool {
    key.source.eq_ignore_ascii_case(source)
}

/// The only keys import can install from for now
fn installable(keys: &[UpdateKey]) -> Option<&UpdateKey> {
    keys.iter().find(|key| is_source(key, "nexus") && is_nexus_id(&key.id))
}

/// One archive can hold several mods sharing a key, so entries are deduplicated by their first key.
/// `resolved` maps `UniqueID`s to Nexus ids for mods whose manifests have none.
/// Returns the entries and the names of mods that have no usable key at all.
fn entries_from(mods: &[Mod], resolved: &HashMap<String, String>) -> (Vec<Entry>, Vec<String>) {
    let mut seen = HashSet::new();
    let (mut entries, mut unexportable) = (Vec::new(), Vec::new());
    for mod_ in mods {
        let mut keys: Vec<UpdateKey> = mod_.manifest.update_keys.iter()
            .filter(|key| !is_source(key, "nexus") || is_nexus_id(&key.id))
            .cloned()
            .collect();
        if installable(&keys).is_none() {
            if let Some(id) = mod_.manifest.unique_id.as_ref().and_then(|unique_id| resolved.get(unique_id)) {
                keys.push(UpdateKey { source: "Nexus".into(), id: id.clone(), subkey: None });
            }
        }
        keys.sort_by_key(|key| !is_source(key, "nexus"));
        let Some(first) = keys.first() else {
            unexportable.push(mod_.manifest.name.clone().unwrap_or_else(|| mod_.path.display().to_string()));
            continue;
        };
        if seen.insert(first.to_string().to_lowercase()) {
            entries.push(Entry {
                keys,
                key: None,
                name: mod_.manifest.name.clone(),
                version: mod_.manifest.version.clone(),
                unique_id: mod_.manifest.unique_id.clone(),
            });
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
    let unresolved: HashSet<&str> = mods.iter()
        .filter(|m| installable(&m.manifest.update_keys).is_none())
        .filter_map(|m| m.manifest.unique_id.as_deref())
        .collect();
    let resolved = match smapi::nexus_ids(&unresolved.into_iter().collect::<Vec<_>>()).await {
        Ok(resolved) => resolved,
        Err(e) => {
            eprintln!("Couldn't look up missing Nexus ids on SMAPI: {e}");
            HashMap::new()
        }
    };
    let (entries, unexportable) = entries_from(mods, &resolved);
    for name in &unexportable {
        eprintln!("{name}: No usable update key, can't be exported");
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

fn is_installed(mods: &[Mod], entry: &Entry) -> bool {
    mods.iter().any(|m| {
        let same_id = entry.unique_id.as_deref().zip(m.manifest.unique_id.as_deref())
            .is_some_and(|(a, b)| a.eq_ignore_ascii_case(b));
        same_id || m.manifest.update_keys.iter().any(|k| entry.keys.iter().any(|e| is_source(e, &k.source) && e.id == k.id))
    })
}

#[cfg_attr(not(feature = "nexus"), allow(unused_variables, clippy::unused_async))]
async fn install(mods: &[Mod], mods_dir: &Path, entry: &Entry, progress: &Progress) -> Result<()> {
    let Some(key) = installable(&entry.keys) else {
        if entry.keys.iter().any(|key| is_source(key, "github")) {
            bail!("GitHub installs are not yet implemented");
        }
        bail!("No supported source, keys: {}", entry.keys.iter().map(ToString::to_string).collect::<Vec<_>>().join(", "));
    };
    #[cfg(feature = "nexus")]
    { crate::add::add_nexus(mods, mods_dir, &key.id, progress).await }
    #[cfg(not(feature = "nexus"))]
    { bail!("Prismatic was compiled without Nexus support") }
}

pub async fn import(mods: &[Mod], mods_dir: &Path, file: &Path, all: bool) -> Result<()> {
    let mut list: ModList = serde_json::from_str(&fs::read_to_string(file).await?)?;
    if !(1..=FORMAT).contains(&list.format) {
        bail!("Unsupported mod list format {}, expected {FORMAT}", list.format);
    }
    for entry in &mut list.mods {
        if let Some(key) = entry.key.take() {
            entry.keys.insert(0, key);
        }
    }
    let entries = pick(list.mods, "Pick mods to install (space: toggle, right: all, left: none, enter: confirm)", all)?;
    let mut mods = mods.to_vec();
    let multi = MultiProgress::new();
    let (mut installed, mut skipped, mut failed) = (0, 0, 0);
    // Sequential: Nexus rate-limits, and a pack can satisfy later entries
    for entry in entries {
        if is_installed(&mods, &entry) {
            skipped += 1;
            eprintln!("{}: Already installed", entry.describe());
            continue;
        }
        if installable(&entry.keys).is_some_and(|key| key.subkey.is_some()) {
            eprintln!("{}: Subkeys are not yet supported, installing the main file", entry.describe());
        }
        let progress = progress::bar(&multi, entry.name.as_deref().unwrap_or("Unnamed mod"));
        let result = install(&mods, mods_dir, &entry, &progress).await;
        progress.emit(Event::Done);
        match result {
            Ok(()) => {
                installed += 1;
                mods = locate_mods(mods_dir).await?;
            }
            Err(e) => {
                failed += 1;
                eprintln!("{}: {e}", entry.describe());
            }
        }
    }
    eprintln!("{installed} installed, {failed} failed, {skipped} already installed");
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::path::PathBuf;
    use super::{entries_from, is_installed, Entry, ModList};
    use crate::mod_locator::{Manifest, Mod, UpdateKey};

    fn key(source: &str, id: &str, subkey: Option<&str>) -> UpdateKey {
        UpdateKey { source: source.into(), id: id.into(), subkey: subkey.map(Into::into) }
    }

    fn mod_(name: &str, unique_id: Option<&str>, keys: Vec<UpdateKey>) -> Mod {
        Mod {
            path: PathBuf::from(name),
            manifest: Manifest {
                name: Some(name.into()),
                author: None,
                version: Some("1.0.0".into()),
                description: None,
                unique_id: unique_id.map(Into::into),
                update_keys: keys,
            },
        }
    }

    fn names(entries: &[Entry]) -> Vec<&str> {
        entries.iter().map(|e| e.name.as_deref().unwrap()).collect()
    }

    #[test]
    fn dedupes_packs_and_reports_mods_without_a_key() {
        let mods = [
            mod_("A", None, vec![key("Nexus", "1", None)]),
            mod_("A (variant)", None, vec![key("nexus", "1", None)]),
            mod_("B", None, vec![]),
            mod_("C", None, vec![key("Nexus", "2", None)]),
            mod_("D", None, vec![key("Nexus", "???", None)]),
        ];
        let (entries, unexportable) = entries_from(&mods, &HashMap::new());
        assert_eq!(names(&entries), ["A", "C"]);
        assert_eq!(unexportable, ["B", "D"]);
    }

    #[test]
    fn keeps_every_key_with_nexus_first() {
        let mods = [mod_("FTM", None, vec![key("GitHub", "o/r", None), key("Nexus", "3231", None), key("ModDrop", "598755", None), key("Nexus", "???", None)])];
        let (entries, _) = entries_from(&mods, &HashMap::new());
        assert_eq!(entries[0].keys.iter().map(ToString::to_string).collect::<Vec<_>>(), ["Nexus:3231", "GitHub:o/r", "ModDrop:598755"]);
    }

    #[test]
    fn resolves_placeholder_keys_through_the_unique_id_and_merges_a_pack() {
        let mods = [
            mod_("SVE Code", Some("Flash.Code"), vec![key("Nexus", "???", None)]),
            mod_("SVE CP", Some("Flash.CP"), vec![key("Nexus", "???", None)]),
            mod_("Unknown", Some("No.Such"), vec![key("Nexus", "???", None)]),
        ];
        let resolved = HashMap::from([("Flash.Code".to_string(), "3753".to_string()), ("Flash.CP".to_string(), "3753".to_string())]);
        let (entries, unexportable) = entries_from(&mods, &resolved);
        assert_eq!(names(&entries), ["SVE Code"]);
        assert_eq!(entries[0].keys[0].to_string(), "Nexus:3753");
        assert_eq!(unexportable, ["Unknown"]);
    }

    #[test]
    fn recognises_installed_mods_by_key_or_unique_id() {
        let entry = |unique_id: Option<&str>, keys: Vec<UpdateKey>| Entry { keys, key: None, name: None, version: None, unique_id: unique_id.map(Into::into) };
        let installed = [mod_("A", Some("x.a"), vec![key("Nexus", "???", None)]), mod_("B", Some("x.b"), vec![key("Nexus", "7", None)])];
        assert!(is_installed(&installed, &entry(Some("X.A"), vec![key("Nexus", "5", None)])));
        assert!(is_installed(&installed, &entry(None, vec![key("nexus", "7", None)])));
        assert!(!is_installed(&installed, &entry(Some("x.c"), vec![key("Nexus", "8", None)])));
    }

    #[test]
    fn keys_round_trip_as_smapi_strings() {
        let list = ModList { format: 2, mods: vec![Entry { keys: vec![key("Nexus", "123", Some("@main"))], key: None, name: None, version: None, unique_id: None }] };
        let json = serde_json::to_string(&list).unwrap();
        assert!(json.contains(r#""keys":["Nexus:123@main"]"#));
        assert!(!json.contains(r#""key":"#));
        let back: ModList = serde_json::from_str(&json).unwrap();
        assert_eq!(back.mods[0].keys[0].to_string(), "Nexus:123@main");
    }

    #[test]
    fn reads_format_1_single_key_entries() {
        let list: ModList = serde_json::from_str(r#"{"format":1,"mods":[{"key":"Nexus:1915","name":"CP","version":"2.9.1"}]}"#).unwrap();
        assert_eq!(list.mods[0].key.as_ref().unwrap().to_string(), "Nexus:1915");
        assert!(list.mods[0].keys.is_empty());
    }
}
