use tokio::fs;
use std::path::{Path, PathBuf};
use anyhow::anyhow;
use async_recursion::async_recursion;
use json_comments::StripComments;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use serde::de::Error;
use std::io::Read;

#[derive(Debug, Clone)]
pub struct Mod {
    pub path: PathBuf,
    pub manifest: Manifest,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct Manifest {
    pub name: Option<String>,
    pub author: Option<String>,
    pub version: Option<String>,
    pub description: Option<String>,
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

pub trait UpdateKeys {
    fn preferred(&self) -> Option<&UpdateKey>;
}

impl UpdateKeys for [UpdateKey] {
    fn preferred(&self) -> Option<&UpdateKey> {
        self.iter().find(|key| cfg!(feature = "github") && key.source.to_lowercase() == "github")
            .or_else(|| self.iter().find(|key| cfg!(feature = "nexus") && key.source.to_lowercase() == "nexus"))
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
fn parse_manifest(text: &str) -> anyhow::Result<Manifest> {
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

#[async_recursion]
pub async fn locate_mods(dir: &Path) -> anyhow::Result<Vec<Mod>> {
    let mut mods = Vec::with_capacity(dir.read_dir()?.count());
    for subdir in dir.read_dir()?.flatten() {
        if !subdir.file_type()?.is_dir() || subdir.file_name().to_str().ok_or_else(|| anyhow!("Invalid directory name"))?.starts_with('.') {
            continue
        }
        let manifest = subdir.path().join("manifest.json");
        if manifest.is_file() {
            mods.push(Mod {
                path: subdir.path(),
                manifest: parse_manifest(&fs::read_to_string(manifest).await?)?,
            });
        } else {
            mods.extend(locate_mods(&subdir.path()).await?);
        }
    }
    Ok(mods)
}

#[cfg(test)]
mod tests {
    use super::parse_manifest;

    #[test]
    fn tolerates_trailing_commas_and_comments() {
        let m = parse_manifest("\u{feff}{\n  \"Name\": \"a,}\", // c\n  \"UpdateKeys\": [\"Nexus:1\",],\n}").unwrap();
        assert_eq!(m.name.as_deref(), Some("a,}"));
        assert_eq!(m.update_keys.len(), 1);
    }
}
