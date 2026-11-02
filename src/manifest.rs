use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use anyhow::{anyhow, Result};
use json_comments::StripComments;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use serde::de::Error;

#[derive(Debug, Clone)]
pub struct Mod {
    pub path: PathBuf,
    pub manifest: Manifest,
}

impl Mod {
    /// Folder relative to the mods directory, as the front ends show and match it
    pub fn relative(&self, mods_dir: &Path) -> String {
        self.path.strip_prefix(mods_dir).unwrap_or(&self.path).display().to_string()
    }

    /// The manifest name, or the folder when it has none
    pub fn display_name(&self, mods_dir: &Path) -> String {
        self.manifest.name.clone().unwrap_or_else(|| self.relative(mods_dir))
    }

    pub fn has_update_key(&self, source: &str, id: &str) -> bool {
        self.manifest.update_keys.iter().any(|key| key.is_source(source) && key.id == id)
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct Manifest {
    pub name: Option<String>,
    pub author: Option<String>,
    pub version: Option<String>,
    pub description: Option<String>,
    /// `PascalCase` would give `UniqueId`, manifests write `UniqueID`
    #[serde(rename = "UniqueID", alias = "UniqueId")]
    pub unique_id: Option<String>,
    #[serde(default)]
    pub update_keys: Vec<UpdateKey>,
}

#[derive(Debug, Clone)]
pub struct UpdateKey {
    pub source: String,
    pub id: String,
    pub subkey: Option<String>,
}

impl UpdateKey {
    pub fn is_source(&self, source: &str) -> bool {
        self.source.eq_ignore_ascii_case(source)
    }
}

impl std::fmt::Display for UpdateKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}:{}{}", self.source, self.id, self.subkey.as_deref().unwrap_or_default())
    }
}

impl Serialize for UpdateKey {
    fn serialize<S>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error> where S: Serializer {
        serializer.collect_str(self)
    }
}

/// Some manifests ship a placeholder like `Nexus:???` instead of a real id
pub fn is_nexus_id(id: &str) -> bool {
    !id.is_empty() && id.bytes().all(|b| b.is_ascii_digit())
}

/// `owner/name`, as a `GitHub:` update key writes it
fn is_github_repo(id: &str) -> bool {
    id.split_once('/').is_some_and(|(owner, name)| !owner.is_empty() && !name.is_empty() && !name.contains('/'))
}

pub trait UpdateKeys {
    /// The key to update from: GitHub first, then Nexus. Other sources are not supported,
    /// and neither are placeholder ids like `Nexus:???`, so a later real key wins over them.
    fn preferred(&self) -> Option<&UpdateKey>;
}

impl UpdateKeys for [UpdateKey] {
    fn preferred(&self) -> Option<&UpdateKey> {
        self.iter().find(|key| key.is_source("github") && is_github_repo(&key.id))
            .or_else(|| self.iter().find(|key| key.is_source("nexus") && is_nexus_id(&key.id)))
    }
}

impl<'de> Deserialize<'de> for UpdateKey {
    fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error> where D: Deserializer<'de> {
        let value = String::deserialize(deserializer)?;
        let (source, key) = value.split_once(':').ok_or_else(|| Error::custom("UpdateKey must contain ':'"))?;
        if let Some((id, subkey)) = key.split_once('@') {
            Ok(Self {
                source: source.into(),
                id: id.into(),
                subkey: Some(format!("@{subkey}")),
            })
        } else {
            Ok(Self {
                source: source.into(),
                id: key.into(),
                subkey: None,
            })
        }
    }
}

/// SMAPI accepts comments and trailing commas in manifests, `serde_json` accepts neither.
fn parse_manifest(text: &str) -> Result<Manifest> {
    let mut stripped = String::new();
    StripComments::new(text.trim_start_matches('\u{feff}').as_bytes()).read_to_string(&mut stripped)?;
    let chars: Vec<char> = stripped.chars().collect();
    let mut json = String::with_capacity(stripped.len());
    let (mut in_string, mut escaped) = (false, false);
    for (i, &c) in chars.iter().enumerate() {
        if in_string {
            in_string = escaped || c != '"';
            escaped = !escaped && c == '\\';
        } else if c == '"' {
            in_string = true;
        } else if c == ',' && matches!(chars[i + 1..].iter().find(|c| !c.is_whitespace()), Some('}' | ']')) {
            continue
        }
        json.push(c);
    }
    Ok(serde_json::from_str(&json)?)
}

fn read_mod(dir: PathBuf, manifest: &Path) -> Result<Mod> {
    let read = || parse_manifest(&fs::read_to_string(manifest)?);
    // One message: callers print errors with plain `Display`, which hides anyhow context layers
    Ok(Mod { path: dir, manifest: read().map_err(|e| anyhow!("{}: {e}", manifest.display()))? })
}

/// The mods found in a folder, and what could not be read. One broken manifest must not hide the rest.
#[derive(Debug, Default)]
pub struct Scan {
    pub mods: Vec<Mod>,
    /// One message per folder or manifest that was skipped, naming it
    pub errors: Vec<String>,
}

/// Mods in `dir`: `dir` itself if it holds a manifest, otherwise those below it
pub fn locate_mods_at(dir: &Path) -> Result<Scan> {
    let manifest = dir.join("manifest.json");
    if manifest.is_file() {
        return Ok(match read_mod(dir.to_path_buf(), &manifest) {
            Ok(mod_) => Scan { mods: vec![mod_], errors: Vec::new() },
            Err(e) => Scan { mods: Vec::new(), errors: vec![e.to_string()] },
        });
    }
    locate_mods(dir)
}

/// Every mod below `dir`, skipping folders that start with a dot (like the `.old` backups).
/// Fails only when `dir` itself can't be read.
pub fn locate_mods(dir: &Path) -> Result<Scan> {
    let mut scan = Scan::default();
    collect(dir.read_dir()?, &mut scan);
    Ok(scan)
}

fn collect(entries: fs::ReadDir, scan: &mut Scan) {
    for entry in entries.flatten() {
        if !entry.file_type().is_ok_and(|kind| kind.is_dir()) || entry.file_name().to_string_lossy().starts_with('.') {
            continue
        }
        let path = entry.path();
        let manifest = path.join("manifest.json");
        if manifest.is_file() {
            match read_mod(path, &manifest) {
                Ok(mod_) => scan.mods.push(mod_),
                Err(e) => scan.errors.push(e.to_string()),
            }
        } else {
            match path.read_dir() {
                Ok(entries) => collect(entries, scan),
                Err(e) => scan.errors.push(format!("{}: {e}", path.display())),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::fs;
    use super::{locate_mods, locate_mods_at, parse_manifest};
    use crate::testutil::scratch_dir;

    fn write_manifest(dir: &std::path::Path, text: &str) {
        fs::create_dir_all(dir).unwrap();
        fs::write(dir.join("manifest.json"), text).unwrap();
    }

    #[test]
    fn reads_unique_id() {
        let m = parse_manifest(r#"{"Name": "a", "UniqueID": "Author.Mod"}"#).unwrap();
        assert_eq!(m.unique_id.as_deref(), Some("Author.Mod"));
    }

    #[test]
    fn tolerates_trailing_commas_and_comments() {
        let m = parse_manifest("\u{feff}{\n  \"Name\": \"a,}\", // c\n  \"UpdateKeys\": [\"Nexus:1\",],\n}").unwrap();
        assert_eq!(m.name.as_deref(), Some("a,}"));
        assert_eq!(m.update_keys.len(), 1);
    }

    #[test]
    fn finds_mods_in_packs_and_skips_dot_folders() {
        let root = scratch_dir();
        write_manifest(&root.join("A"), r#"{"Name":"A"}"#);
        write_manifest(&root.join("Pack/B"), r#"{"Name":"B"}"#);
        write_manifest(&root.join("Pack/C"), r#"{"Name":"C"}"#);
        write_manifest(&root.join(".old/D - 1.0"), r#"{"Name":"D"}"#);
        let mut names: Vec<_> = locate_mods(&root).unwrap().mods.into_iter().filter_map(|m| m.manifest.name).collect();
        names.sort();
        assert_eq!(names, ["A", "B", "C"]);
    }

    #[test]
    fn locate_mods_at_accepts_the_mod_folder_itself() {
        let root = scratch_dir();
        write_manifest(&root.join("A"), r#"{"Name":"A"}"#);
        assert_eq!(locate_mods_at(&root.join("A")).unwrap().mods.len(), 1);
        assert_eq!(locate_mods_at(&root).unwrap().mods.len(), 1);
    }

    #[test]
    fn a_broken_manifest_is_reported_by_name_and_the_other_mods_are_still_found() {
        let root = scratch_dir();
        write_manifest(&root.join("Good"), r#"{"Name":"Good"}"#);
        write_manifest(&root.join("Pack/Broken"), "{nope");
        write_manifest(&root.join("Pack/Fine"), r#"{"Name":"Fine"}"#);

        let scan = locate_mods(&root).unwrap();

        assert_eq!(scan.mods.len(), 2);
        assert_eq!(scan.errors.len(), 1);
        assert!(scan.errors[0].contains("Broken") && scan.errors[0].contains("line 1"), "{}", scan.errors[0]);
    }

    #[test]
    fn an_unreadable_mods_folder_is_an_error() {
        assert!(locate_mods(&scratch_dir().join("missing")).is_err());
    }

    #[test]
    fn prefers_github_over_nexus_and_ignores_other_sources() {
        use super::{UpdateKey, UpdateKeys};
        let key = |source: &str, id: &str| UpdateKey { source: source.into(), id: id.into(), subkey: None };
        assert_eq!([key("Nexus", "1"), key("GitHub", "o/r")].preferred().unwrap().source, "GitHub");
        assert_eq!([key("ModDrop", "9"), key("nexus", "1")].preferred().unwrap().source, "nexus");
        assert!([key("ModDrop", "9")].preferred().is_none());
    }

    #[test]
    fn placeholder_ids_lose_to_a_real_key_and_alone_mean_no_source() {
        use super::{UpdateKey, UpdateKeys};
        let key = |source: &str, id: &str| UpdateKey { source: source.into(), id: id.into(), subkey: None };
        assert_eq!([key("Nexus", "???"), key("Nexus", "1915")].preferred().unwrap().id, "1915");
        assert_eq!([key("GitHub", "owner"), key("Nexus", "7")].preferred().unwrap().id, "7");
        assert!([key("Nexus", "???")].preferred().is_none());
        assert!([key("GitHub", "/repo"), key("GitHub", "a/b/c")].preferred().is_none());
    }
}
