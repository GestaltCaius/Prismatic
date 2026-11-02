use crate::manifest::{Mod, UpdateKeys};
use anyhow::{anyhow, Result};
use serde_json::{json, Value};
use std::collections::HashMap;

const MODS_API: &str = "https://smapi.io/api/v3.0/mods";
const API_VERSION: &str = "4.0.7"; // TODO: Use actual version
const TIMEOUT: std::time::Duration = std::time::Duration::from_secs(15);

async fn query(http: &wreq::Client, body: Value) -> Result<Vec<Value>> {
    let response = http
        .post(MODS_API)
        .json(&body)
        .timeout(TIMEOUT)
        .send()
        .await?
        .error_for_status()?
        .json::<Value>()
        .await?;
    match response {
        Value::Array(entries) => Ok(entries),
        _ => Err(anyhow!("Expected array")),
    }
}

/// Asks SMAPI's mod database for the Nexus id of each mod, by `UniqueID`.
/// Used when a manifest has no usable `Nexus:<id>` update key (e.g. `Nexus:???`).
/// Mods SMAPI doesn't know, or knows without a Nexus page, are left out of the result.
pub async fn nexus_ids(
    http: &wreq::Client,
    unique_ids: &[&str],
) -> Result<HashMap<String, String>> {
    if unique_ids.is_empty() {
        return Ok(HashMap::new());
    }
    let body = json!({
        "mods": unique_ids.iter().map(|id| json!({"id": id, "updateKeys": [], "installedVersion": ""})).collect::<Vec<_>>(),
        "apiVersion": API_VERSION,
        "includeExtendedMetadata": true,
    });
    Ok(parse_nexus_ids(&query(http, body).await?))
}

fn parse_nexus_ids(entries: &[Value]) -> HashMap<String, String> {
    entries
        .iter()
        .filter_map(|entry| {
            let id = entry.get("id")?.as_str()?;
            let nexus = entry.get("metadata")?.get("nexusID")?;
            let nexus = nexus
                .as_u64()
                .map(|n| n.to_string())
                .or_else(|| nexus.as_str().map(str::to_string))?;
            Some((id.to_string(), nexus))
        })
        .collect()
}

/// The id SMAPI knows a mod by: its `UniqueID`, or a made-up one from its update key.
/// `None` when the mod has no key to update from.
fn request_id(m: &Mod) -> Option<String> {
    let key = m.manifest.update_keys.preferred()?;
    Some(
        m.manifest
            .unique_id
            .clone()
            .unwrap_or_else(|| format!("FAKE.{}.{}", key.source, key.id)),
    )
}

/// Newest version SMAPI suggests for each mod, in the order of `mods`, in one request.
/// `None` means up to date, or no usable update key.
pub async fn suggested_versions(http: &wreq::Client, mods: &[Mod]) -> Result<Vec<Option<String>>> {
    let requests: Vec<Value> = mods
        .iter()
        .filter_map(|m| {
            let key = m.manifest.update_keys.preferred()?;
            Some(json!({
                "id": request_id(m)?,
                "updateKeys": [format!("{}:{}", key.source, key.id)],
                "installedVersion": m.manifest.version.clone().unwrap_or_default(),
            }))
        })
        .collect();
    if requests.is_empty() {
        return Ok(vec![None; mods.len()]);
    }
    let entries = query(http, json!({"mods": requests, "apiVersion": API_VERSION})).await?;
    let versions = parse_suggestions(&entries);
    Ok(mods
        .iter()
        .map(|m| versions.get(&request_id(m)?).cloned())
        .collect())
}

fn parse_suggestions(entries: &[Value]) -> HashMap<String, String> {
    entries
        .iter()
        .filter_map(|entry| {
            Some((
                entry.get("id")?.as_str()?.to_string(),
                entry
                    .get("suggestedUpdate")?
                    .get("version")?
                    .as_str()?
                    .to_string(),
            ))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::{nexus_ids, parse_nexus_ids, parse_suggestions, request_id, suggested_versions};
    use crate::manifest::{Manifest, Mod, UpdateKey};
    use serde_json::json;
    use std::path::PathBuf;

    fn mod_(unique_id: Option<&str>, version: &str, keys: Vec<UpdateKey>) -> Mod {
        Mod {
            path: PathBuf::from("m"),
            manifest: Manifest {
                version: Some(version.into()),
                unique_id: unique_id.map(Into::into),
                update_keys: keys,
                ..Manifest::default()
            },
        }
    }

    fn nexus_key() -> UpdateKey {
        UpdateKey {
            source: "Nexus".into(),
            id: "3109".into(),
            subkey: None,
        }
    }

    #[test]
    fn nexus_ids_accept_numbers_and_strings_and_skip_unknown_mods() {
        let entries = json!([
            {"id": "A", "metadata": {"nexusID": 1915}},
            {"id": "B", "metadata": {"nexusID": "3753"}},
            {"id": "C", "metadata": {}},
            {"id": "D"},
        ]);
        let ids = parse_nexus_ids(entries.as_array().unwrap());
        assert_eq!(ids.len(), 2);
        assert_eq!((ids["A"].as_str(), ids["B"].as_str()), ("1915", "3753"));
    }

    #[test]
    fn suggestions_only_list_mods_with_an_update() {
        let entries = json!([
            {"id": "A", "suggestedUpdate": {"version": "2.0", "url": "x"}},
            {"id": "B"},
        ]);
        let versions = parse_suggestions(entries.as_array().unwrap());
        assert_eq!(versions.len(), 1);
        assert_eq!(versions["A"], "2.0");
    }

    #[test]
    fn mods_without_a_unique_id_are_asked_about_by_a_made_up_one() {
        assert_eq!(
            request_id(&mod_(Some("x.old"), "1", vec![nexus_key()])).as_deref(),
            Some("x.old")
        );
        assert_eq!(
            request_id(&mod_(None, "1", vec![nexus_key()])).as_deref(),
            Some("FAKE.Nexus.3109")
        );
        assert_eq!(request_id(&mod_(Some("x.old"), "1", vec![])), None);
    }

    #[tokio::test]
    async fn nothing_to_ask_means_no_request() {
        let http = crate::http::client().unwrap();
        assert!(nexus_ids(&http, &[]).await.unwrap().is_empty());
        let versions = suggested_versions(&http, &[mod_(None, "1", vec![])])
            .await
            .unwrap();
        assert_eq!(versions, [None]);
    }

    /// Run with: cargo test smapi -- --ignored --nocapture
    #[tokio::test]
    #[ignore = "integration: hits the real SMAPI API"]
    async fn resolves_nexus_ids() {
        let http = crate::http::client().unwrap();
        let ids = nexus_ids(
            &http,
            &[
                "FlashShifter.SVECode",
                "Pathoschild.ContentPatcher",
                "No.Such.Mod",
            ],
        )
        .await
        .unwrap();
        println!("{ids:?}");
        assert_eq!(
            ids.get("Pathoschild.ContentPatcher").map(String::as_str),
            Some("1915")
        );
        assert!(!ids.contains_key("No.Such.Mod"));
    }

    /// Run with: cargo test smapi -- --ignored --nocapture
    #[tokio::test]
    #[ignore = "integration: hits the real SMAPI API"]
    async fn suggests_versions_for_outdated_mods_only() {
        let http = crate::http::client().unwrap();
        let mods = [
            mod_(Some("x.old"), "0.0.1", vec![nexus_key()]),
            mod_(Some("x.new"), "999.0.0", vec![nexus_key()]),
            mod_(None, "0.0.1", vec![nexus_key()]),
            mod_(None, "1", vec![]),
        ];
        let versions = suggested_versions(&http, &mods).await.unwrap();
        println!("{versions:?}");
        assert!(versions[0].is_some());
        assert!(versions[1].is_none());
        assert!(versions[2].is_some());
        assert!(versions[3].is_none());
    }
}
