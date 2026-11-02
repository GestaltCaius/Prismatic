use std::path::PathBuf;
use super::guard::TOKEN_HEADER;
use super::{bind, new_state, random_token, router, Shared};

#[test]
fn tokens_are_long_and_different() {
    let (a, b) = (random_token().unwrap(), random_token().unwrap());
    assert_eq!(a.len(), 64);
    assert_ne!(a, b);
}

fn mods_folder(manifest: Option<(&str, &str)>) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("prismatic-ui-test-{}", random_token().unwrap()));
    std::fs::create_dir_all(&dir).unwrap();
    if let Some((folder, text)) = manifest {
        std::fs::create_dir_all(dir.join(folder)).unwrap();
        std::fs::write(dir.join(folder).join("manifest.json"), text).unwrap();
    }
    dir
}

const FAKE: (&str, &str) = ("Fake", r#"{"Name":"Fake","Version":"1.0","UniqueID":"x.fake","UpdateKeys":["Nexus:1"]}"#);

/// Starts the real server on a free port, `dir` is the mods folder it starts with, if any
async fn serve_in(dir: Option<PathBuf>, explicit: bool) -> (u16, String, Shared) {
    let listener = bind(0).await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let state = new_state(dir, explicit, port).unwrap();
    let (token, shared) = (state.token.clone(), state.clone());
    tokio::spawn(async move { axum::serve(listener, router(state)).await.unwrap() });
    (port, token, shared)
}

/// The real server with a mods folder holding one mod
async fn serve() -> (u16, String, PathBuf) {
    let dir = mods_folder(Some(FAKE));
    let (port, token, _state) = serve_in(Some(dir.clone()), false).await;
    (port, token, dir)
}

#[tokio::test]
async fn rejects_requests_without_the_token_and_serves_the_ones_with_it() {
    let (port, token, _dir) = serve().await;
    let client = wreq::Client::new();
    let base = format!("http://127.0.0.1:{port}");

    assert_eq!(client.get(format!("{base}/")).send().await.unwrap().status(), 403);
    assert_eq!(client.get(format!("{base}/?token=wrong")).send().await.unwrap().status(), 403);
    assert_eq!(client.get(format!("{base}/api/state")).send().await.unwrap().status(), 403);
    assert_eq!(client.get(format!("{base}/api/state")).header(TOKEN_HEADER, "wrong").send().await.unwrap().status(), 403);
    assert_eq!(client.post(format!("{base}/api/update")).json(&serde_json::json!({"paths": null})).send().await.unwrap().status(), 403);

    let page = client.get(format!("{base}/?token={token}")).send().await.unwrap();
    assert_eq!(page.status(), 200);
    assert!(page.headers().contains_key("content-security-policy"));

    let state: serde_json::Value = client.get(format!("{base}/api/state")).header(TOKEN_HEADER, &token).send().await.unwrap().json().await.unwrap();
    assert_eq!(state["mods"][0]["name"], "Fake");
    assert_eq!(state["mods"][0]["keys"][0], "Nexus:1");
}

#[tokio::test]
async fn state_lists_the_readable_mods_and_warns_about_the_broken_ones() {
    let (port, token, dir) = serve().await;
    std::fs::create_dir_all(dir.join("Broken")).unwrap();
    std::fs::write(dir.join("Broken/manifest.json"), "{nope").unwrap();

    let state = state(&format!("http://127.0.0.1:{port}"), &token).await;

    assert_eq!(state["mods"].as_array().unwrap().len(), 1);
    assert_eq!(state["warnings"].as_array().unwrap().len(), 1);
    assert!(state["warnings"][0].as_str().unwrap().contains("Broken"));
}

#[tokio::test]
async fn serves_the_images_without_a_token_but_only_to_loopback_hosts() {
    let (port, _token, _dir) = serve().await;
    let client = wreq::Client::new();
    for (name, kind) in [("logo.png", "image/png"), ("duck.png", "image/png"), ("dog.gif", "image/gif")] {
        let response = client.get(format!("http://127.0.0.1:{port}/assets/{name}")).send().await.unwrap();
        assert_eq!(response.status(), 200, "{name}");
        assert_eq!(response.headers()["content-type"], kind, "{name}");
    }
    let foreign = client.get(format!("http://127.0.0.1:{port}/assets/logo.png")).header("Host", "evil.example").send().await.unwrap();
    assert_eq!(foreign.status(), 403);
}

#[tokio::test]
async fn rejects_a_foreign_host_even_with_the_token() {
    let (port, token, _dir) = serve().await;
    let response = wreq::Client::new().get(format!("http://127.0.0.1:{port}/api/state"))
        .header("Host", "evil.example")
        .header(TOKEN_HEADER, &token)
        .send().await.unwrap();
    assert_eq!(response.status(), 403);
}

async fn post(base: &str, token: &str, path: &str, body: serde_json::Value) -> (u16, String) {
    let response = wreq::Client::new().post(format!("{base}{path}")).header(TOKEN_HEADER, token).json(&body).send().await.unwrap();
    (response.status().as_u16(), response.text().await.unwrap())
}

async fn state(base: &str, token: &str) -> serde_json::Value {
    wreq::Client::new().get(format!("{base}/api/state")).header(TOKEN_HEADER, token).send().await.unwrap().json().await.unwrap()
}

/// The first job once it has finished, polling for at most `seconds`
async fn finished_job(base: &str, token: &str, seconds: u64) -> serde_json::Value {
    for _ in 0..seconds * 10 {
        let state = state(base, token).await;
        if state["jobs"][0]["finished"] == true {
            return state["jobs"][0].clone();
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
    panic!("job did not finish");
}

#[tokio::test]
async fn exports_previews_and_imports_through_the_api() {
    let (port, token, _dir) = serve().await;
    let base = format!("http://127.0.0.1:{port}");

    let (status, body) = post(&base, &token, "/api/export", serde_json::json!({"paths": null})).await;
    assert_eq!(status, 200);
    let exported: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!(exported["count"], 1);
    let content = exported["content"].as_str().unwrap();
    assert!(content.contains("Nexus:1"));

    let (status, body) = post(&base, &token, "/api/import/preview", serde_json::json!({"content": content})).await;
    assert_eq!(status, 200);
    let preview: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!(preview[0]["state"], "installed");

    assert_eq!(post(&base, &token, "/api/import/preview", serde_json::json!({"content": "nope"})).await.0, 400);
    assert_eq!(post(&base, &token, "/api/import", serde_json::json!({"content": content, "indexes": []})).await.0, 400);

    let (status, _) = post(&base, &token, "/api/import", serde_json::json!({"content": content, "indexes": [0]})).await;
    assert_eq!(status, 200);
    let job = finished_job(&base, &token, 5).await;
    assert_eq!(job["items"][0]["status"], "skipped");
    assert_eq!(job["items"][0]["message"], "Already installed");
}

#[tokio::test]
async fn validates_cookie_and_add_requests() {
    let (port, token, _dir) = serve().await;
    let base = format!("http://127.0.0.1:{port}");

    assert_eq!(post(&base, &token, "/api/cookie", serde_json::json!({"value": "  "})).await.0, 400);
    assert_eq!(post(&base, &token, "/api/cookie", serde_json::json!({"value": "bad\ncookie"})).await.0, 400);
    assert_eq!(post(&base, &token, "/api/add", serde_json::json!({"url": "https://example.com/"})).await.0, 400);
    assert_eq!(post(&base, &token, "/api/update", serde_json::json!({"paths": ["No such mod"]})).await.0, 400);

    assert_eq!(post(&base, &token, "/api/cookie", serde_json::json!({"value": "test-value"})).await.0, 204);
    assert_eq!(state(&base, &token).await["cookie"], "pasted");

    let forgotten = wreq::Client::new().delete(format!("{base}/api/cookie")).header(TOKEN_HEADER, &token).send().await.unwrap();
    assert_eq!(forgotten.status(), 204);
    assert_ne!(state(&base, &token).await["cookie"], "pasted");

    std::env::set_var("NEXUS_COOKIE", "other=1; nexusmods_session=from-env");
    let (status, body) = post(&base, &token, "/api/cookie/environment", serde_json::json!({})).await;
    assert_eq!((status, body.as_str()), (200, r#"{"value":"from-env"}"#));
    std::env::remove_var("NEXUS_COOKIE");
    assert_eq!(post(&base, &token, "/api/cookie/environment", serde_json::json!({})).await.0, 400);
}

#[tokio::test]
async fn adding_an_installed_mod_fails_before_any_download() {
    let (port, token, _dir) = serve().await;
    let base = format!("http://127.0.0.1:{port}");

    let (status, _) = post(&base, &token, "/api/add", serde_json::json!({"url": "https://www.nexusmods.com/stardewvalley/mods/1"})).await;
    assert_eq!(status, 200);

    let job = finished_job(&base, &token, 5).await;
    assert_eq!(job["items"][0]["label"], "Nexus:1");
    assert_eq!(job["items"][0]["status"], "failed");
    assert_eq!(job["items"][0]["message"], "Already installed: Fake");
}

#[tokio::test]
async fn updating_a_mod_without_a_source_fails_with_the_reason() {
    let (port, token, dir) = serve().await;
    std::fs::create_dir_all(dir.join("Bare")).unwrap();
    std::fs::write(dir.join("Bare/manifest.json"), r#"{"Name":"Bare","Version":"1.0"}"#).unwrap();
    let base = format!("http://127.0.0.1:{port}");

    let (status, _) = post(&base, &token, "/api/update", serde_json::json!({"paths": ["Bare"]})).await;
    assert_eq!(status, 200);

    let job = finished_job(&base, &token, 5).await;
    assert_eq!(job["items"][0]["status"], "failed");
    assert_eq!(job["items"][0]["message"], "Unknown or unsupported source");
}

/// Run with: `cargo test updates_an_outdated_mod -- --ignored`
/// Requires `NEXUS_COOKIE`, downloads `AutomaticGates` (33K) into a temporary folder.
#[tokio::test]
#[ignore = "integration: hits real Nexus and SMAPI, requires NEXUS_COOKIE"]
async fn updates_an_outdated_mod_through_the_api() {
    let (port, token, dir) = serve().await;
    std::fs::create_dir_all(dir.join("AutomaticGates")).unwrap();
    std::fs::write(dir.join("AutomaticGates/manifest.json"), r#"{"Name":"AutomaticGates","Version":"0.0.1","UniqueID":"Pathoschild.AutomaticGates","UpdateKeys":["Nexus:3109"]}"#).unwrap();
    let base = format!("http://127.0.0.1:{port}");

    let checked: serde_json::Value = wreq::Client::new().post(format!("{base}/api/check")).header(TOKEN_HEADER, &token).send().await.unwrap().json().await.unwrap();
    assert!(checked["AutomaticGates"].as_str().is_some());

    let (status, _) = post(&base, &token, "/api/update", serde_json::json!({"paths": ["AutomaticGates"]})).await;
    assert_eq!(status, 200);
    let job = finished_job(&base, &token, 30).await;
    let item = &job["items"][0];
    assert_eq!(item["status"], "done", "{item}");
    assert!(item["message"].as_str().unwrap().starts_with("0.0.1 -> "));
}

async fn set_folder(base: &str, token: &str, path: serde_json::Value) -> (u16, String) {
    post(base, token, "/api/folder", serde_json::json!({ "path": path })).await
}

#[tokio::test]
async fn without_a_folder_the_page_is_asked_to_choose_one() {
    let (port, token, _state) = serve_in(None, false).await;
    let base = format!("http://127.0.0.1:{port}");

    let before = state(&base, &token).await;
    assert!(before["mods_dir"].is_null());
    assert_eq!(before["mods"].as_array().unwrap().len(), 0);
    assert!(before["candidates"].is_array());
    assert_eq!(post(&base, &token, "/api/check", serde_json::json!({})).await.0, 400);
    assert_eq!(post(&base, &token, "/api/add", serde_json::json!({"url": "https://www.nexusmods.com/stardewvalley/mods/1"})).await.0, 400);

    let dir = mods_folder(Some(FAKE));
    assert_eq!(set_folder(&base, &token, dir.to_str().unwrap().into()).await.0, 204);

    let after = state(&base, &token).await;
    assert_eq!(after["mods_dir"], dir.to_str().unwrap());
    assert_eq!(after["mods"][0]["name"], "Fake");
}

#[tokio::test]
async fn the_folder_can_be_switched_and_the_new_one_is_listed() {
    let (port, token, _dir) = serve().await;
    let base = format!("http://127.0.0.1:{port}");
    let other = mods_folder(Some(("Other", r#"{"Name":"Other"}"#)));

    assert_eq!(set_folder(&base, &token, other.to_str().unwrap().into()).await.0, 204);

    let mods = state(&base, &token).await["mods"].clone();
    assert_eq!((mods.as_array().unwrap().len(), mods[0]["name"].as_str()), (1, Some("Other")));
}

#[tokio::test]
async fn only_existing_folders_are_accepted() {
    let (port, token, dir) = serve().await;
    let base = format!("http://127.0.0.1:{port}");

    let missing = dir.join("nope");
    let (status, message) = set_folder(&base, &token, missing.to_str().unwrap().into()).await;
    assert_eq!(status, 400);
    assert!(message.contains("is not a folder"), "{message}");
    let file = dir.join("Fake/manifest.json");
    assert_eq!(set_folder(&base, &token, file.to_str().unwrap().into()).await.0, 400, "a file is not a folder");
    assert_eq!(set_folder(&base, &token, "bad\npath".into()).await.0, 400);
    assert_eq!(state(&base, &token).await["mods"][0]["name"], "Fake", "a refused folder changes nothing");
}

#[tokio::test]
async fn the_folder_is_not_changed_while_a_job_runs_and_the_cookie_survives_a_change() {
    let (port, token, state_) = serve_in(Some(mods_folder(Some(FAKE))), true).await;
    let base = format!("http://127.0.0.1:{port}");
    assert_eq!(post(&base, &token, "/api/cookie", serde_json::json!({"value": "pasted-value"})).await.0, 204);
    let other = mods_folder(None);

    let busy = state_.work.lock().await;
    assert_eq!(set_folder(&base, &token, other.to_str().unwrap().into()).await.0, 409);
    drop(busy);
    assert_eq!(set_folder(&base, &token, other.to_str().unwrap().into()).await.0, 204);

    let after = state(&base, &token).await;
    assert_eq!(after["cookie"], "pasted");
    assert_eq!(after["explicit"], true);
}

#[tokio::test]
async fn disabling_a_mod_hides_it_from_updates_and_warns_about_the_mods_that_need_it() {
    let (port, token, dir) = serve().await;
    let base = format!("http://127.0.0.1:{port}");
    std::fs::create_dir_all(dir.join("Pack")).unwrap();
    std::fs::write(dir.join("Pack/manifest.json"), r#"{"Name":"Pack","UniqueID":"x.pack","ContentPackFor":{"UniqueID":"X.Fake"}}"#).unwrap();

    let mods = state(&base, &token).await["mods"].clone();
    let fake = mods.as_array().unwrap().iter().find(|m| m["name"] == "Fake").unwrap();
    assert_eq!(fake["required_by"], serde_json::json!(["Pack"]));
    assert_eq!(fake["disabled"], false);

    assert_eq!(post(&base, &token, "/api/enable", serde_json::json!({"paths": ["Fake"], "enabled": false})).await.0, 204);
    assert!(dir.join(".Fake/manifest.json").is_file() && !dir.join("Fake").exists());
    assert!(dir.join(".Pack/manifest.json").is_file(), "the mod that needs it goes off too");
    let mods = state(&base, &token).await["mods"].clone();
    let fake = mods.as_array().unwrap().iter().find(|m| m["name"] == "Fake").unwrap();
    assert_eq!((fake["disabled"].clone(), fake["path"].clone()), (true.into(), ".Fake".into()));
    assert_eq!(post(&base, &token, "/api/update", serde_json::json!({"paths": [".Fake"]})).await.0, 400, "disabled mods are not updated");

    assert_eq!(post(&base, &token, "/api/enable", serde_json::json!({"paths": [".Pack"], "enabled": true})).await.0, 204);
    assert!(dir.join("Fake/manifest.json").is_file() && dir.join("Pack/manifest.json").is_file(), "the mod it needs comes on too");
}
