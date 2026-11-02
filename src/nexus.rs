use std::sync::{PoisonError, RwLock};
use anyhow::{anyhow, Result};
use bytes::Bytes;
use scraper::{Html, Selector};
use serde_json::json;
use tokio::sync::{OnceCell, Semaphore};
use crate::http::read_body;
use crate::manifest::is_nexus_id;
use crate::progress::{Event, Progress};

const GAME_ID: u32 = 1303;
/// Downloads at once, more gets us rate-limited
const DOWNLOAD_SLOTS: usize = 2;

/// Nexus Mods access: the session cookie and the download flow.
/// Nexus has no API for basic accounts, so this scrapes the site like a browser would,
/// signed in with the `nexusmods_session` cookie.
pub struct Nexus {
    /// Cookie pasted in the web UI, it takes precedence over the environment and the browser
    pasted: RwLock<Option<String>>,
    /// The environment or browser cookie, looked up once (the browser may ask for a keychain password)
    found: OnceCell<String>,
    slots: Semaphore,
}

/// Left out on purpose: the cookie is a credential and must not end up in a log
impl std::fmt::Debug for Nexus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Nexus").finish_non_exhaustive()
    }
}

/// `nexusmods_session` out of a bare value or a full cookie string
fn session_from(value: &str) -> String {
    value.split(';')
        .find_map(|part| part.trim().strip_prefix("nexusmods_session="))
        .map_or_else(|| value.trim().to_string(), str::to_string)
}

/// Extracts the mod id from a Stardew Valley Nexus URL
pub fn mod_id_from_url(url: &str) -> Option<&str> {
    let id = url.split("nexusmods.com/stardewvalley/mods/").nth(1)?
        .split(['?', '#', '/']).next()?;
    is_nexus_id(id).then_some(id)
}

/// The `data-id` of the main file on a mod's files tab, or of the first update file when there is none
fn main_file_id(html: &str) -> Result<String> {
    let document = Html::parse_document(html);
    let header = |container: &str| Selector::parse(&format!("#{container} .file-expander-header")).map_err(|e| anyhow!("{e}"));
    let (main, updates) = (header("file-container-main-files")?, header("file-container-update-files")?);
    document.select(&main)
        .next()
        .or_else(|| document.select(&updates).next())
        .ok_or_else(|| anyhow!("HTML parsing failed: couldn't locate main file element"))?
        .attr("data-id")
        .map(str::to_string)
        .ok_or_else(|| anyhow!("HTML parsing failed: couldn't find data-id attribute"))
}

impl Nexus {
    pub fn new() -> Self {
        Self { pasted: RwLock::new(None), found: OnceCell::new(), slots: Semaphore::new(DOWNLOAD_SLOTS) }
    }

    fn pasted(&self) -> Option<String> {
        self.pasted.read().unwrap_or_else(PoisonError::into_inner).clone()
    }

    pub fn set_cookie(&self, value: &str) {
        *self.pasted.write().unwrap_or_else(PoisonError::into_inner) = Some(session_from(value));
    }

    /// Drops the pasted cookie so the environment and the browser are used again
    pub fn clear_cookie(&self) {
        *self.pasted.write().unwrap_or_else(PoisonError::into_inner) = None;
    }

    /// Where the cookie will come from, without touching the browser: `pasted`, `environment` or neither
    pub fn cookie_source(&self) -> Option<&'static str> {
        if self.pasted().is_some() {
            Some("pasted")
        } else if std::env::var("NEXUS_COOKIE").is_ok() {
            Some("environment")
        } else {
            None
        }
    }

    /// The `NEXUS_COOKIE` variable, as a bare session value
    pub fn env_cookie() -> Option<String> {
        std::env::var("NEXUS_COOKIE").ok().map(|value| session_from(&value))
    }

    /// Reads the session from the browsers' cookie stores, which may ask for a keychain password
    pub fn browser_cookie() -> Result<String> {
        rookie::load(Some(vec!["nexusmods.com".to_string()])).map_err(|e| anyhow!("{e}"))?.iter()
            .find(|cookie| cookie.name == "nexusmods_session")
            .map(|cookie| cookie.value.clone())
            .ok_or_else(|| anyhow!("No nexusmods_session found. Set the NEXUS_COOKIE environment variable or sign in to Nexus in your browser."))
    }

    async fn session(&self) -> Result<String> {
        if let Some(pasted) = self.pasted() {
            return Ok(pasted);
        }
        let found = self.found.get_or_try_init(|| async {
            match Self::env_cookie() {
                Some(cookie) => Ok(cookie),
                None => tokio::task::spawn_blocking(Self::browser_cookie).await?,
            }
        }).await?;
        Ok(found.clone())
    }

    /// Downloads the main file of a Stardew Valley mod
    pub async fn download(&self, http: &wreq::Client, mod_id: &str, progress: &Progress) -> Result<Bytes> {
        let session = self.session().await?;
        progress.emit(Event::Stage("Waiting for a download slot"));
        let _slot = self.slots.acquire().await?;
        let cookie = format!("nexusmods_session={session}");
        let files_url = format!("https://www.nexusmods.com/stardewvalley/mods/{mod_id}?tab=files");
        progress.emit(Event::Stage("Finding the file"));
        let html = http.get(&files_url)
            .header("Cookie", &cookie)
            .header("Referer", "https://www.nexusmods.com/stardewvalley/mods/")
            .send().await?.error_for_status()?.text().await?;
        let file_id = main_file_id(&html)?;
        progress.emit(Event::Stage("Requesting the download link"));
        let link = http
            .post("https://www.nexusmods.com/Core/Libs/Common/Managers/Downloads?GenerateDownloadUrl")
            .form(&json!({"fid": file_id, "game_id": GAME_ID, "collection_id": 0}))
            .header("Cookie", &cookie)
            .header("Referer", &files_url)
            .header("X-Requested-With", "XMLHttpRequest")
            .header("Origin", "https://www.nexusmods.com")
            .send().await?
            .error_for_status()?
            .json::<serde_json::Value>().await?;
        let url = link.get("url").ok_or_else(|| anyhow!("GenerateDownloadUrl unexpected response: {link}"))?
            .as_str().ok_or_else(|| anyhow!("GenerateDownloadUrl: url is not a string"))?;
        let response = http.get(url).header("Cookie", &cookie).send().await?.error_for_status()?;
        read_body(response, progress).await
    }
}

impl Default for Nexus {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod properties {
    use proptest::prelude::*;
    use super::{is_nexus_id, mod_id_from_url};

    proptest! {
        #[test]
        fn an_id_taken_from_a_url_is_a_real_id_that_was_in_it(url in any::<String>()) {
            if let Some(id) = mod_id_from_url(&url) {
                prop_assert!(is_nexus_id(id) && url.contains(id));
            }
        }

        #[test]
        fn pages_of_a_mod_give_its_id(id in 1u32..10_000_000, tail in "([?#/][a-z=&0-9]{0,12})?") {
            let url = format!("https://www.nexusmods.com/stardewvalley/mods/{id}{tail}");
            let expected = id.to_string();
            prop_assert_eq!(mod_id_from_url(&url), Some(expected.as_str()));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{main_file_id, mod_id_from_url, session_from, Nexus};

    #[test]
    fn parses_nexus_urls() {
        assert_eq!(mod_id_from_url("https://www.nexusmods.com/stardewvalley/mods/3109?tab=files"), Some("3109"));
        assert_eq!(mod_id_from_url("nexusmods.com/stardewvalley/mods/12/"), Some("12"));
        assert_eq!(mod_id_from_url("https://www.nexusmods.com/skyrim/mods/1"), None);
        assert_eq!(mod_id_from_url("https://www.nexusmods.com/stardewvalley/mods/abc"), None);
    }

    #[test]
    fn extracts_the_session_from_a_bare_value_or_a_cookie_string() {
        assert_eq!(session_from("  abc "), "abc");
        assert_eq!(session_from("other=1; nexusmods_session=abc; x=2"), "abc");
    }

    #[test]
    fn a_pasted_cookie_wins_and_can_be_forgotten() {
        let nexus = Nexus::new();
        nexus.set_cookie("nexusmods_session=pasted");
        assert_eq!(nexus.cookie_source(), Some("pasted"));
        assert_eq!(nexus.pasted().as_deref(), Some("pasted"));
        nexus.clear_cookie();
        assert_ne!(nexus.cookie_source(), Some("pasted"));
    }

    #[test]
    fn finds_the_main_file_then_falls_back_to_update_files() {
        let page = |main: &str, update: &str| format!(
            r#"<div id="file-container-main-files">{main}</div><div id="file-container-update-files">{update}</div>"#);
        let header = |id: &str| format!(r#"<dt class="file-expander-header" data-id="{id}"></dt>"#);
        assert_eq!(main_file_id(&page(&header("11"), &header("22"))).unwrap(), "11");
        assert_eq!(main_file_id(&page("", &header("22"))).unwrap(), "22");
    }

    #[test]
    fn reports_a_page_without_files_or_ids() {
        let none = main_file_id("<html></html>").unwrap_err().to_string();
        assert!(none.contains("couldn't locate main file element"), "{none}");
        let no_id = main_file_id(r#"<div id="file-container-main-files"><dt class="file-expander-header"></dt></div>"#).unwrap_err().to_string();
        assert!(no_id.contains("data-id"), "{no_id}");
    }
}
