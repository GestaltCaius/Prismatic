use std::path::PathBuf;
use super::guard::TOKEN_HEADER;
use super::{bind, new_state, random_token, router};
use prismatic::Manager;
use std::sync::Arc;

#[test]
fn tokens_are_long_and_different() {
    let (a, b) = (random_token().unwrap(), random_token().unwrap());
    assert_eq!(a.len(), 64);
    assert_ne!(a, b);
}

/// Starts the real server on a free port with a mods folder holding one mod
async fn serve() -> (u16, String, PathBuf) {
    let dir = std::env::temp_dir().join(format!("prismatic-ui-test-{}", random_token().unwrap()));
    std::fs::create_dir_all(dir.join("Fake")).unwrap();
    std::fs::write(dir.join("Fake/manifest.json"), r#"{"Name":"Fake","Version":"1.0","UniqueID":"x.fake","UpdateKeys":["Nexus:1"]}"#).unwrap();
    let listener = bind(0).await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let state = new_state(Arc::new(Manager::new(dir.clone()).unwrap()), port).unwrap();
    let token = state.token.clone();
    tokio::spawn(async move { axum::serve(listener, router(state)).await.unwrap() });
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
