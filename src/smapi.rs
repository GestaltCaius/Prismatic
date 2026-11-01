use std::collections::HashMap;
use anyhow::{anyhow, Result};
use serde_json::{json, Value};
use crate::mod_locator::{Mod, UpdateKeys};
use crate::update::reqwest_client;

/// Asks SMAPI's mod database for the Nexus id of each mod, by `UniqueID`.
/// Used when a manifest has no usable `Nexus:<id>` update key (e.g. `Nexus:???`).
/// Mods SMAPI doesn't know, or knows without a Nexus page, are left out of the result.
pub async fn nexus_ids(unique_ids: &[&str]) -> Result<HashMap<String, String>> {
    if unique_ids.is_empty() {
        return Ok(HashMap::new());
    }
    let body = json!({
        "mods": unique_ids.iter().map(|id| json!({"id": id, "updateKeys": [], "installedVersion": ""})).collect::<Vec<_>>(),
        "apiVersion": "4.0.7",  // TODO: Use actual version
        "includeExtendedMetadata": true,
    });
    let response = reqwest_client().await?
        .post("https://smapi.io/api/v3.0/mods")
        .json(&body)
        .send().await?
        .error_for_status()?
        .json::<Value>().await?;
    Ok(response.as_array().ok_or_else(|| anyhow!("Expected array"))?.iter()
        .filter_map(|entry| {
            let id = entry.get("id")?.as_str()?;
            let nexus = entry.get("metadata")?.get("nexusID")?;
            let nexus = nexus.as_u64().map(|n| n.to_string()).or_else(|| nexus.as_str().map(str::to_string))?;
            Some((id.to_string(), nexus))
        })
        .collect())
}

/// Newest version SMAPI suggests for each mod, in the order of `mods`. `None` means up to date,
/// or no usable update key.
pub async fn suggested_versions(mods: &[Mod]) -> Result<Vec<Option<String>>> {
    let request_id = |m: &Mod| m.manifest.update_keys.preferred().map(|key| {
        m.manifest.unique_id.clone().unwrap_or_else(|| format!("FAKE.{}.{}", key.source, key.id))
    });
    let requests: Vec<Value> = mods.iter().filter_map(|m| {
        let key = m.manifest.update_keys.preferred()?;
        Some(json!({
            "id": request_id(m)?,
            "updateKeys": [format!("{}:{}", key.source, key.id)],
            "installedVersion": m.manifest.version.clone().unwrap_or_default(),
        }))
    }).collect();
    if requests.is_empty() {
        return Ok(vec![None; mods.len()]);
    }
    let response = reqwest_client().await?
        .post("https://smapi.io/api/v3.0/mods")
        .json(&json!({"mods": requests, "apiVersion": "4.0.7"}))  // TODO: Use actual version
        .send().await?
        .error_for_status()?
        .json::<Value>().await?;
    let versions: HashMap<String, String> = response.as_array().ok_or_else(|| anyhow!("Expected array"))?.iter()
        .filter_map(|entry| Some((
            entry.get("id")?.as_str()?.to_string(),
            entry.get("suggestedUpdate")?.get("version")?.as_str()?.to_string(),
        )))
        .collect();
    Ok(mods.iter().map(|m| versions.get(&request_id(m)?).cloned()).collect())
}

#[cfg(test)]
mod tests {
    use super::nexus_ids;

    /// Run with: cargo test smapi -- --ignored --nocapture
    #[tokio::test]
    #[ignore = "integration: hits the real SMAPI API"]
    async fn resolves_nexus_ids() {
        let ids = nexus_ids(&["FlashShifter.SVECode", "Pathoschild.ContentPatcher", "No.Such.Mod"]).await.unwrap();
        println!("{ids:?}");
        assert_eq!(ids.get("Pathoschild.ContentPatcher").map(String::as_str), Some("1915"));
        assert!(!ids.contains_key("No.Such.Mod"));
    }

    /// Run with: cargo test smapi -- --ignored --nocapture
    #[tokio::test]
    #[ignore = "integration: hits the real SMAPI API"]
    async fn suggests_versions_for_outdated_mods_only() {
        use std::path::PathBuf;
        use crate::mod_locator::{Manifest, Mod, UpdateKey};
        use super::suggested_versions;

        let at = |version: &str, unique_id: Option<&str>| Mod {
            path: PathBuf::from("m"),
            manifest: Manifest {
                name: None,
                author: None,
                version: Some(version.into()),
                description: None,
                unique_id: unique_id.map(Into::into),
                update_keys: vec![UpdateKey { source: "Nexus".into(), id: "3109".into(), subkey: None }],
            },
        };
        let no_key = Mod { path: PathBuf::from("n"), manifest: Manifest { name: None, author: None, version: None, description: None, unique_id: None, update_keys: vec![] } };
        let mods = [at("0.0.1", Some("x.old")), at("999.0.0", Some("x.new")), at("0.0.1", None), no_key];
        let versions = suggested_versions(&mods).await.unwrap();
        println!("{versions:?}");
        assert!(versions[0].is_some());
        assert!(versions[1].is_none());
        assert!(versions[2].is_some());
        assert!(versions[3].is_none());
    }
}
