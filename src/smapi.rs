use std::collections::HashMap;
use anyhow::{anyhow, Result};
use serde_json::{json, Value};
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
}
