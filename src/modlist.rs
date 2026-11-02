//! The mod list file that export writes and import reads, and the logic behind both.

use std::collections::{HashMap, HashSet};
use anyhow::{bail, Result};
use serde::{Deserialize, Serialize};
use crate::manifest::{is_nexus_id, Mod, UpdateKey};
use crate::smapi;

const FORMAT: u32 = 2;

#[derive(Debug, Serialize, Deserialize)]
struct ModList {
    format: u32,
    mods: Vec<Entry>,
}

/// `name` and `version` are informational. `keys` holds every update key the mod declares,
/// installable ones first, and import installs from the first one it supports.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Entry {
    #[serde(default)]
    pub keys: Vec<UpdateKey>,
    /// Format 1 files had a single `key`, it is folded into `keys` when importing
    #[serde(default, skip_serializing)]
    key: Option<UpdateKey>,
    pub name: Option<String>,
    pub version: Option<String>,
    pub unique_id: Option<String>,
}

impl Entry {
    pub fn describe(&self) -> String {
        let key = self.keys.first().map_or_else(|| "no key".to_string(), ToString::to_string);
        format!("{} {} ({key})", self.name.as_deref().unwrap_or("Unnamed mod"), self.version.as_deref().unwrap_or(""))
    }
}

/// The only keys import can install from for now
pub fn installable(keys: &[UpdateKey]) -> Option<&UpdateKey> {
    keys.iter().find(|key| key.is_source("nexus") && is_nexus_id(&key.id))
}

/// One archive can hold several mods sharing a key, so entries are deduplicated by their first key.
/// `resolved` maps `UniqueID`s to Nexus ids for mods whose manifests have none.
/// Returns the entries and the names of mods that have no usable key at all.
pub(crate) fn entries_from(mods: &[Mod], resolved: &HashMap<String, String>) -> (Vec<Entry>, Vec<String>) {
    let mut seen = HashSet::new();
    let (mut entries, mut unexportable) = (Vec::new(), Vec::new());
    for mod_ in mods {
        let mut keys: Vec<UpdateKey> = mod_.manifest.update_keys.iter()
            .filter(|key| !key.is_source("nexus") || is_nexus_id(&key.id))
            .cloned()
            .collect();
        if installable(&keys).is_none() {
            if let Some(id) = mod_.manifest.unique_id.as_ref().and_then(|unique_id| resolved.get(unique_id)) {
                keys.push(UpdateKey { source: "Nexus".into(), id: id.clone(), subkey: None });
            }
        }
        keys.sort_by_key(|key| !key.is_source("nexus"));
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

/// What export found out about the installed mods
pub struct Collected {
    pub entries: Vec<Entry>,
    /// Names of mods that have no usable key at all
    pub unexportable: Vec<String>,
    /// Set when SMAPI couldn't be asked for the Nexus ids that manifests lack
    pub lookup_error: Option<String>,
}

/// Entries for the installed mods, asking SMAPI for Nexus ids the manifests lack
pub(crate) async fn collect_entries(http: &wreq::Client, mods: &[Mod]) -> Collected {
    let unresolved: HashSet<&str> = mods.iter()
        .filter(|m| installable(&m.manifest.update_keys).is_none())
        .filter_map(|m| m.manifest.unique_id.as_deref())
        .collect();
    let (resolved, lookup_error) = match smapi::nexus_ids(http, &unresolved.into_iter().collect::<Vec<_>>()).await {
        Ok(resolved) => (resolved, None),
        Err(e) => (HashMap::new(), Some(e.to_string())),
    };
    let (entries, unexportable) = entries_from(mods, &resolved);
    Collected { entries, unexportable, lookup_error }
}

pub fn list_json(entries: Vec<Entry>) -> Result<String> {
    Ok(serde_json::to_string_pretty(&ModList { format: FORMAT, mods: entries })?)
}

pub fn parse_list(text: &str) -> Result<Vec<Entry>> {
    let mut list: ModList = serde_json::from_str(text)?;
    if !(1..=FORMAT).contains(&list.format) {
        bail!("Unsupported mod list format {}, expected {FORMAT}", list.format);
    }
    for entry in &mut list.mods {
        if let Some(key) = entry.key.take() {
            entry.keys.insert(0, key);
        }
    }
    Ok(list.mods)
}

pub fn is_installed(mods: &[Mod], entry: &Entry) -> bool {
    mods.iter().any(|m| {
        let same_id = entry.unique_id.as_deref().zip(m.manifest.unique_id.as_deref())
            .is_some_and(|(a, b)| a.eq_ignore_ascii_case(b));
        same_id || m.manifest.update_keys.iter().any(|k| entry.keys.iter().any(|e| e.is_source(&k.source) && e.id == k.id))
    })
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::path::PathBuf;
    use super::{entries_from, is_installed, Entry, ModList};
    use crate::manifest::{Manifest, Mod, UpdateKey};

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
