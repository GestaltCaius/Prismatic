use std::collections::HashMap;
use std::fmt::Write;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use anyhow::{anyhow, bail, Result};
use axum::extract::{Request, State};
use axum::http::{header, HeaderValue, StatusCode};
use axum::middleware::{self, Next};
use axum::response::{Html, IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use tokio::net::TcpListener;
use crate::mod_list::{collect_entries, install, installable, is_installed, list_json, parse_list};
use crate::mod_locator::{locate_mods, Mod, UpdateKeys};
use crate::progress::{Event, Progress};
use crate::smapi;
use crate::update::{update_mod, UpdateResult};

pub const DEFAULT_PORT: u16 = 47821;
const PAGE: &str = include_str!("ui.html");
const LOGO: &[u8] = include_bytes!("assets/logo.png");
const DUCK: &[u8] = include_bytes!("assets/duck.png");
const DOG: &[u8] = include_bytes!("assets/dog.gif");
const TOKEN_HEADER: &str = "x-prismatic-token";
/// How many finished jobs the page keeps showing
const JOBS_KEPT: usize = 20;

struct AppState {
    mods_dir: PathBuf,
    token: String,
    port: u16,
    jobs: Mutex<Vec<Job>>,
    /// Jobs touch the mods folder, so only one runs at a time
    work: tokio::sync::Mutex<()>,
}

type Shared = Arc<AppState>;

#[derive(Serialize, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
enum Status {
    Queued,
    Running,
    Done,
    Failed,
    Skipped,
}

#[derive(Serialize, Clone)]
struct Item {
    label: String,
    status: Status,
    message: String,
    stage: String,
    downloaded: u64,
    total: Option<u64>,
}

#[derive(Serialize, Clone)]
struct Job {
    id: usize,
    title: String,
    items: Vec<Item>,
    finished: bool,
}

#[derive(Serialize)]
struct ModInfo {
    path: String,
    name: String,
    version: Option<String>,
    author: Option<String>,
    description: Option<String>,
    unique_id: Option<String>,
    keys: Vec<String>,
    /// Whether `update` knows how to update this mod
    updatable: bool,
}

#[derive(Serialize)]
struct StateResponse {
    mods_dir: String,
    mods: Vec<ModInfo>,
    jobs: Vec<Job>,
    /// `pasted`, `environment`, or nothing (the browser's cookie store is tried on demand)
    cookie: Option<&'static str>,
}

type ApiResult<T> = std::result::Result<T, (StatusCode, String)>;

fn bad_request(message: &str) -> (StatusCode, String) {
    (StatusCode::BAD_REQUEST, message.to_string())
}

fn server_error(e: &anyhow::Error) -> (StatusCode, String) {
    (StatusCode::INTERNAL_SERVER_ERROR, e.to_string())
}

fn relative(mods_dir: &Path, path: &Path) -> String {
    path.strip_prefix(mods_dir).unwrap_or(path).display().to_string()
}

fn constant_time_eq(a: &str, b: &str) -> bool {
    a.len() == b.len() && a.bytes().zip(b.bytes()).fold(0, |acc, (x, y)| acc | (x ^ y)) == 0
}

/// Only the loopback names with our own port, anything else is a DNS rebinding attempt
fn allowed_host(host: &str, port: u16) -> bool {
    let host = host.to_ascii_lowercase();
    host == format!("127.0.0.1:{port}") || host == format!("localhost:{port}")
}

fn token_from_query(query: &str) -> Option<&str> {
    query.split('&').find_map(|pair| pair.strip_prefix("token="))
}

async fn guard(State(state): State<Shared>, request: Request, next: Next) -> Response {
    let host_ok = request.headers().get(header::HOST)
        .and_then(|host| host.to_str().ok())
        .is_some_and(|host| allowed_host(host, state.port));
    let supplied = if request.uri().path() == "/" {
        request.uri().query().and_then(token_from_query)
    } else {
        request.headers().get(TOKEN_HEADER).and_then(|token| token.to_str().ok())
    };
    // Images can't send the token header, and they hold nothing private
    let public = request.uri().path().starts_with("/assets/");
    if !host_ok || !(public || supplied.is_some_and(|token| constant_time_eq(token, &state.token))) {
        return (StatusCode::FORBIDDEN, "Forbidden").into_response();
    }
    next.run(request).await
}

async fn page() -> Response {
    let mut response = Html(PAGE).into_response();
    let headers = response.headers_mut();
    headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    headers.insert(header::REFERRER_POLICY, HeaderValue::from_static("no-referrer"));
    headers.insert(header::X_CONTENT_TYPE_OPTIONS, HeaderValue::from_static("nosniff"));
    headers.insert(header::CONTENT_SECURITY_POLICY, HeaderValue::from_static(
        "default-src 'none'; script-src 'unsafe-inline'; style-src 'unsafe-inline'; img-src 'self'; connect-src 'self'; base-uri 'none'; form-action 'none'",
    ));
    response
}

#[cfg_attr(not(feature = "nexus"), allow(clippy::missing_const_for_fn))]
fn cookie_source() -> Option<&'static str> {
    #[cfg(feature = "nexus")]
    { crate::update::nexus_cookie_source() }
    #[cfg(not(feature = "nexus"))]
    { None }
}

async fn api_state(State(state): State<Shared>) -> ApiResult<Json<StateResponse>> {
    let mods = locate_mods(&state.mods_dir).await.map_err(|e| server_error(&e))?;
    let mut mods: Vec<ModInfo> = mods.iter().map(|m| ModInfo {
        path: relative(&state.mods_dir, &m.path),
        name: m.manifest.name.clone().unwrap_or_else(|| relative(&state.mods_dir, &m.path)),
        version: m.manifest.version.clone(),
        author: m.manifest.author.clone(),
        description: m.manifest.description.clone(),
        unique_id: m.manifest.unique_id.clone(),
        keys: m.manifest.update_keys.iter().map(ToString::to_string).collect(),
        updatable: m.manifest.update_keys.preferred().is_some_and(|key| key.subkey.is_none()),
    }).collect();
    mods.sort_by_key(|m| m.name.to_lowercase());
    let jobs = state.jobs.lock().map(|jobs| jobs.clone()).unwrap_or_default();
    Ok(Json(StateResponse { mods_dir: state.mods_dir.display().to_string(), mods, jobs, cookie: cookie_source() }))
}

#[derive(Deserialize)]
struct CookieRequest {
    value: String,
}

async fn api_cookie(Json(request): Json<CookieRequest>) -> ApiResult<StatusCode> {
    let value = request.value.trim();
    if value.is_empty() || value.len() > 4096 || value.chars().any(char::is_control) {
        return Err(bad_request("That doesn't look like a cookie value"));
    }
    #[cfg(feature = "nexus")]
    {
        crate::update::set_nexus_cookie(value);
        Ok(StatusCode::NO_CONTENT)
    }
    #[cfg(not(feature = "nexus"))]
    { Err(bad_request("Prismatic was compiled without Nexus support")) }
}

#[derive(Serialize)]
struct CookieValue {
    value: String,
}

/// Hands the cookie from the environment to the page so it can fill the field, the page is token protected and local
async fn api_cookie_from_environment() -> ApiResult<Json<CookieValue>> {
    #[cfg(feature = "nexus")]
    { crate::update::env_nexus_cookie().map(|value| Json(CookieValue { value })).ok_or_else(|| bad_request("The NEXUS_COOKIE variable is not set")) }
    #[cfg(not(feature = "nexus"))]
    { Err(bad_request("Prismatic was compiled without Nexus support")) }
}

async fn api_cookie_from_browser() -> ApiResult<Json<CookieValue>> {
    #[cfg(feature = "nexus")]
    {
        let value = tokio::task::spawn_blocking(crate::update::browser_nexus_cookie).await
            .map_err(|e| server_error(&anyhow!("{e}")))?
            .map_err(|e| bad_request(&e.to_string()))?;
        Ok(Json(CookieValue { value }))
    }
    #[cfg(not(feature = "nexus"))]
    { Err(bad_request("Prismatic was compiled without Nexus support")) }
}

async fn api_forget_cookie() -> StatusCode {
    #[cfg(feature = "nexus")]
    crate::update::clear_nexus_cookie();
    StatusCode::NO_CONTENT
}

/// Mods whose path is in `paths`, or all of them when `paths` is `None`
async fn select_mods(state: &AppState, paths: Option<&[String]>) -> ApiResult<Vec<Mod>> {
    let mods = locate_mods(&state.mods_dir).await.map_err(|e| server_error(&e))?;
    Ok(mods.into_iter()
        .filter(|m| paths.is_none_or(|paths| paths.contains(&relative(&state.mods_dir, &m.path))))
        .collect())
}

#[derive(Deserialize)]
struct PathsRequest {
    paths: Option<Vec<String>>,
}

/// Newest SMAPI-suggested version per mod path, only for mods that have one
async fn api_check(State(state): State<Shared>) -> ApiResult<Json<HashMap<String, String>>> {
    let mods = select_mods(&state, None).await?;
    let versions = smapi::suggested_versions(&mods).await.map_err(|e| server_error(&e))?;
    Ok(Json(mods.iter().zip(versions)
        .filter_map(|(m, version)| Some((relative(&state.mods_dir, &m.path), version?)))
        .collect()))
}

fn new_job(state: &AppState, title: String, labels: Vec<String>) -> usize {
    let mut jobs = state.jobs.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    let id = jobs.last().map_or(0, |job| job.id + 1);
    jobs.push(Job {
        id,
        title,
        items: labels.into_iter().map(|label| Item {
            label,
            status: Status::Queued,
            message: String::new(),
            stage: String::new(),
            downloaded: 0,
            total: None,
        }).collect(),
        finished: false,
    });
    while jobs.len() > JOBS_KEPT {
        jobs.remove(0);
    }
    drop(jobs);
    id
}

fn with_item(state: &AppState, job: usize, index: usize, change: impl FnOnce(&mut Item)) {
    let mut jobs = state.jobs.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    if let Some(item) = jobs.iter_mut().find(|j| j.id == job).and_then(|j| j.items.get_mut(index)) {
        change(item);
    }
}

fn finish(state: &AppState, job: usize, index: usize, status: Status, message: String) {
    with_item(state, job, index, |item| {
        item.status = status;
        item.message = message;
        item.stage.clear();
        item.total = None;
    });
}

fn finish_job(state: &AppState, job: usize) {
    let mut jobs = state.jobs.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    if let Some(job) = jobs.iter_mut().find(|j| j.id == job) {
        job.finished = true;
    }
}

fn item_progress(state: &Shared, job: usize, index: usize) -> Progress {
    let state = state.clone();
    Progress::new(move |event| with_item(&state, job, index, |item| match event {
        Event::Stage(label) => {
            label.clone_into(&mut item.stage);
            item.downloaded = 0;
            item.total = None;
        }
        Event::Download { done, total } => {
            "Downloading".clone_into(&mut item.stage);
            item.downloaded = done;
            item.total = total;
        }
        Event::Done => item.stage.clear(),
    }))
}

fn mod_label(state: &AppState, m: &Mod) -> String {
    m.manifest.name.clone().unwrap_or_else(|| relative(&state.mods_dir, &m.path))
}

async fn api_update(State(state): State<Shared>, Json(request): Json<PathsRequest>) -> ApiResult<Json<usize>> {
    let mods = select_mods(&state, request.paths.as_deref()).await?;
    if mods.is_empty() {
        return Err(bad_request("No mods selected"));
    }
    let job = new_job(&state, "Update mods".into(), mods.iter().map(|m| mod_label(&state, m)).collect());
    tokio::spawn(async move {
        let _turn = state.work.lock().await;
        for (index, m) in mods.iter().enumerate() {
            with_item(&state, job, index, |item| item.status = Status::Running);
            let progress = item_progress(&state, job, index);
            match update_mod(m, false, &progress).await {
                UpdateResult::Success { new_version } => {
                    let message = m.manifest.version.as_ref().map_or_else(|| new_version.clone(), |old| format!("{old} -> {new_version}"));
                    finish(&state, job, index, Status::Done, message);
                }
                UpdateResult::AlreadyUpToDate => finish(&state, job, index, Status::Skipped, "Already up to date".into()),
                UpdateResult::Failure(message) => finish(&state, job, index, Status::Failed, message),
            }
        }
        finish_job(&state, job);
    });
    Ok(Json(job))
}

#[derive(Deserialize)]
#[cfg_attr(not(feature = "nexus"), allow(dead_code))]
struct AddRequest {
    url: String,
}

async fn api_add(State(state): State<Shared>, Json(request): Json<AddRequest>) -> ApiResult<Json<usize>> {
    #[cfg(feature = "nexus")]
    {
        let id = crate::add::nexus_id(request.url.trim())
            .ok_or_else(|| bad_request("Expected a URL like https://www.nexusmods.com/stardewvalley/mods/<id>"))?
            .to_string();
        let job = new_job(&state, "Add a mod".into(), vec![format!("Nexus:{id}")]);
        tokio::spawn(async move {
            let _turn = state.work.lock().await;
            with_item(&state, job, 0, |item| item.status = Status::Running);
            let progress = item_progress(&state, job, 0);
            let result = match locate_mods(&state.mods_dir).await {
                Ok(mods) => crate::add::add_nexus(&mods, &state.mods_dir, &id, &progress).await,
                Err(e) => Err(e),
            };
            progress.emit(Event::Done);
            match result.and_then(|added| Ok((added.lines(&state.mods_dir)?, added.warnings))) {
                Ok((lines, warnings)) => finish(&state, job, 0, Status::Done, lines.into_iter().chain(warnings).collect::<Vec<_>>().join("\n")),
                Err(e) => finish(&state, job, 0, Status::Failed, e.to_string()),
            }
            finish_job(&state, job);
        });
        Ok(Json(job))
    }
    #[cfg(not(feature = "nexus"))]
    {
        let _ = (state, request);
        Err(bad_request("Prismatic was compiled without Nexus support"))
    }
}

#[derive(Serialize)]
struct ExportResponse {
    content: String,
    count: usize,
    skipped: Vec<String>,
}

async fn api_export(State(state): State<Shared>, Json(request): Json<PathsRequest>) -> ApiResult<Json<ExportResponse>> {
    let mods = select_mods(&state, request.paths.as_deref()).await?;
    let (entries, skipped) = collect_entries(&mods).await;
    if entries.is_empty() {
        return Err(bad_request("None of the selected mods can be exported"));
    }
    let count = entries.len();
    Ok(Json(ExportResponse { content: list_json(entries).map_err(|e| server_error(&e))?, count, skipped }))
}

#[derive(Deserialize)]
struct ImportPreviewRequest {
    content: String,
}

#[derive(Serialize)]
struct PreviewEntry {
    index: usize,
    name: String,
    version: Option<String>,
    key: String,
    /// `installed`, `installable` or `unsupported`
    state: &'static str,
}

async fn api_import_preview(State(state): State<Shared>, Json(request): Json<ImportPreviewRequest>) -> ApiResult<Json<Vec<PreviewEntry>>> {
    let entries = parse_list(&request.content).map_err(|e| bad_request(&format!("Not a Prismatic mod list: {e}")))?;
    let mods = locate_mods(&state.mods_dir).await.map_err(|e| server_error(&e))?;
    Ok(Json(entries.iter().enumerate().map(|(index, entry)| PreviewEntry {
        index,
        name: entry.name.clone().unwrap_or_else(|| "Unnamed mod".into()),
        version: entry.version.clone(),
        key: entry.keys.first().map_or_else(|| "no key".into(), ToString::to_string),
        state: if is_installed(&mods, entry) {
            "installed"
        } else if installable(&entry.keys).is_some() {
            "installable"
        } else {
            "unsupported"
        },
    }).collect()))
}

#[derive(Deserialize)]
struct ImportRequest {
    content: String,
    indexes: Vec<usize>,
}

async fn api_import(State(state): State<Shared>, Json(request): Json<ImportRequest>) -> ApiResult<Json<usize>> {
    let entries = parse_list(&request.content).map_err(|e| bad_request(&format!("Not a Prismatic mod list: {e}")))?;
    let chosen: Vec<_> = request.indexes.iter().filter_map(|index| entries.get(*index).cloned()).collect();
    if chosen.is_empty() {
        return Err(bad_request("No mods selected"));
    }
    let job = new_job(&state, "Import mods".into(), chosen.iter().map(|entry| entry.name.clone().unwrap_or_else(|| "Unnamed mod".into())).collect());
    tokio::spawn(async move {
        let _turn = state.work.lock().await;
        for (index, entry) in chosen.iter().enumerate() {
            with_item(&state, job, index, |item| item.status = Status::Running);
            let mods = match locate_mods(&state.mods_dir).await {
                Ok(mods) => mods,
                Err(e) => {
                    finish(&state, job, index, Status::Failed, e.to_string());
                    continue;
                }
            };
            if is_installed(&mods, entry) {
                finish(&state, job, index, Status::Skipped, "Already installed".into());
                continue;
            }
            let progress = item_progress(&state, job, index);
            let result = install(&mods, &state.mods_dir, entry, &progress).await;
            progress.emit(Event::Done);
            match result {
                Ok(done) => finish(&state, job, index, Status::Done, done.lines.into_iter().chain(done.warnings).collect::<Vec<_>>().join("\n")),
                Err(e) => finish(&state, job, index, Status::Failed, e.to_string()),
            }
        }
        finish_job(&state, job);
    });
    Ok(Json(job))
}

fn image(content_type: &'static str, bytes: &'static [u8]) -> Response {
    ([(header::CONTENT_TYPE, content_type), (header::CACHE_CONTROL, "max-age=3600")], bytes).into_response()
}

fn router(state: Shared) -> Router {
    Router::new()
        .route("/", get(page))
        .route("/assets/logo.png", get(|| async { image("image/png", LOGO) }))
        .route("/assets/duck.png", get(|| async { image("image/png", DUCK) }))
        .route("/assets/dog.gif", get(|| async { image("image/gif", DOG) }))
        .route("/api/state", get(api_state))
        .route("/api/cookie", post(api_cookie).delete(api_forget_cookie))
        .route("/api/cookie/environment", post(api_cookie_from_environment))
        .route("/api/cookie/browser", post(api_cookie_from_browser))
        .route("/api/check", post(api_check))
        .route("/api/update", post(api_update))
        .route("/api/add", post(api_add))
        .route("/api/export", post(api_export))
        .route("/api/import/preview", post(api_import_preview))
        .route("/api/import", post(api_import))
        .layer(middleware::from_fn_with_state(state.clone(), guard))
        .with_state(state)
}

fn random_token() -> Result<String> {
    let mut bytes = [0u8; 32];
    getrandom::fill(&mut bytes).map_err(|e| anyhow!("Couldn't generate a token: {e}"))?;
    Ok(bytes.iter().fold(String::new(), |mut token, byte| {
        let _ = write!(token, "{byte:02x}");
        token
    }))
}

fn open_browser(url: &str) {
    let result = if cfg!(target_os = "macos") {
        std::process::Command::new("open").arg(url).spawn()
    } else if cfg!(windows) {
        std::process::Command::new("cmd").args(["/C", "start", "", url]).spawn()
    } else {
        std::process::Command::new("xdg-open").arg(url).spawn()
    };
    if result.is_err() {
        eprintln!("Couldn't open a browser, open the address above yourself");
    }
}

/// Binds `port` on the loopback interface only, or any free port when that one is taken
async fn bind(port: u16) -> Result<TcpListener> {
    match TcpListener::bind(("127.0.0.1", port)).await {
        Ok(listener) => Ok(listener),
        Err(_) => Ok(TcpListener::bind(("127.0.0.1", 0)).await?),
    }
}

pub async fn run(mods_dir: PathBuf, port: u16, open: bool) -> Result<()> {
    let listener = bind(port).await?;
    let port = listener.local_addr()?.port();
    let token = random_token()?;
    let url = format!("http://127.0.0.1:{port}/?token={token}");
    let state = Arc::new(AppState { mods_dir, token, port, jobs: Mutex::new(Vec::new()), work: tokio::sync::Mutex::new(()) });
    println!("Prismatic is running at {url}\nPress Ctrl+C to stop.");
    if open {
        open_browser(&url);
    }
    if let Err(e) = axum::serve(listener, router(state)).await {
        bail!("Server stopped: {e}");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_loopback_hosts_with_our_port_are_allowed() {
        assert!(allowed_host("127.0.0.1:47821", 47821));
        assert!(allowed_host("LocalHost:47821", 47821));
        assert!(!allowed_host("127.0.0.1:80", 47821));
        assert!(!allowed_host("evil.example:47821", 47821));
        assert!(!allowed_host("127.0.0.1.evil.example:47821", 47821));
    }

    #[test]
    fn finds_the_token_in_a_query() {
        assert_eq!(token_from_query("token=abc"), Some("abc"));
        assert_eq!(token_from_query("x=1&token=abc"), Some("abc"));
        assert_eq!(token_from_query("x=1"), None);
    }

    #[test]
    fn compares_tokens_exactly() {
        assert!(constant_time_eq("abc", "abc"));
        assert!(!constant_time_eq("abc", "abd"));
        assert!(!constant_time_eq("abc", "abcd"));
    }

    #[test]
    fn tokens_are_long_and_different() {
        let (a, b) = (random_token().unwrap(), random_token().unwrap());
        assert_eq!(a.len(), 64);
        assert_ne!(a, b);
    }

    fn color(name: &str) -> [f64; 3] {
        let key = format!("--{name}: #");
        let start = PAGE.find(&key).unwrap() + key.len();
        let channel = |i: usize| f64::from(u8::from_str_radix(&PAGE[start + i..start + i + 2], 16).unwrap());
        [channel(0), channel(2), channel(4)]
    }

    /// WCAG 2.x contrast ratio between two palette colors
    fn contrast(a: &str, b: &str) -> f64 {
        let luminance = |name: &str| {
            let [r, g, b] = color(name).map(|c| {
                let c = c / 255.0;
                if c <= 0.03928 { c / 12.92 } else { ((c + 0.055) / 1.055).powf(2.4) }
            });
            0.0722f64.mul_add(b, 0.7152f64.mul_add(g, 0.2126 * r))
        };
        let (la, lb) = (luminance(a), luminance(b));
        (la.max(lb) + 0.05) / (la.min(lb) + 0.05)
    }

    fn failing<'a>(pairs: &[(&'a str, &'a str)], minimum: f64) -> Vec<String> {
        pairs
            .iter()
            .map(|&(a, b)| (a, b, contrast(a, b)))
            .filter(|&(_, _, ratio)| ratio < minimum)
            .map(|(a, b, ratio)| format!("{a} on {b} is {ratio:.2}:1, needs {minimum}:1"))
            .collect()
    }

    #[test]
    fn text_colors_meet_wcag_aa() {
        // Pairs mirror where each color is used: panels sit on `surface`, inputs and the error banner on `bg`
        let pairs = [
            ("text", "surface"), ("text", "bg"), ("muted", "surface"), ("muted", "bg"),
            ("accent-contrast", "accent"), ("ok", "surface"), ("warn", "surface"),
            ("danger", "surface"), ("danger", "bg"),
        ];
        let failures = failing(&pairs, 4.5);
        assert!(failures.is_empty(), "{failures:#?}");
    }

    #[test]
    fn control_boundaries_and_focus_ring_meet_wcag_non_text_contrast() {
        let pairs = [("border", "surface"), ("accent", "surface"), ("text", "surface")];
        let failures = failing(&pairs, 3.0);
        assert!(failures.is_empty(), "{failures:#?}");
    }

    #[test]
    fn contrast_math_matches_the_wcag_reference() {
        assert!((contrast("text", "text") - 1.0).abs() < f64::EPSILON);
        assert!(contrast("text", "surface") > contrast("muted", "surface"));
    }

    #[test]
    fn page_has_the_accessibility_basics() {
        assert!(PAGE.contains("<html lang=\"en\">"));
        assert!(PAGE.contains("name=\"viewport\""));
        assert!(PAGE.contains(":focus-visible"));
        assert!(PAGE.contains("role=\"alert\""));
        assert!(PAGE.contains("id=\"cookieStatus\" aria-live=\"polite\""));
        assert!(!PAGE.contains("outline: none") && !PAGE.contains("outline: 0"));
        for tag in PAGE.split("<img").skip(1) {
            let tag = &tag[..tag.find('>').unwrap()];
            assert!(tag.contains(" alt="), "image without alt text: <img{tag}>");
        }
        for tag in PAGE.split("<input").skip(1) {
            let tag = &tag[..tag.find('>').unwrap()];
            assert!(tag.contains("aria-label="), "input without a label: <input{tag}>");
        }
    }

    /// Starts the real server on a free port with an empty mods folder
    async fn serve() -> (u16, String, PathBuf) {
        let dir = std::env::temp_dir().join(format!("prismatic-ui-test-{}", random_token().unwrap()));
        std::fs::create_dir_all(dir.join("Fake")).unwrap();
        std::fs::write(dir.join("Fake/manifest.json"), r#"{"Name":"Fake","Version":"1.0","UniqueID":"x.fake","UpdateKeys":["Nexus:1"]}"#).unwrap();
        let listener = bind(0).await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let token = random_token().unwrap();
        let state = Arc::new(AppState { mods_dir: dir.clone(), token: token.clone(), port, jobs: Mutex::new(Vec::new()), work: tokio::sync::Mutex::new(()) });
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
        let client = wreq::Client::new();
        for _ in 0..50 {
            let state: serde_json::Value = client.get(format!("{base}/api/state")).header(TOKEN_HEADER, &token).send().await.unwrap().json().await.unwrap();
            if state["jobs"][0]["finished"] == true {
                assert_eq!(state["jobs"][0]["items"][0]["status"], "skipped");
                assert_eq!(state["jobs"][0]["items"][0]["message"], "Already installed");
                return;
            }
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        }
        panic!("import job did not finish");
    }

    #[tokio::test]
    async fn validates_cookie_and_add_requests() {
        let (port, token, _dir) = serve().await;
        let base = format!("http://127.0.0.1:{port}");

        assert_eq!(post(&base, &token, "/api/cookie", serde_json::json!({"value": "  "})).await.0, 400);
        assert_eq!(post(&base, &token, "/api/cookie", serde_json::json!({"value": "bad\ncookie"})).await.0, 400);
        assert_eq!(post(&base, &token, "/api/add", serde_json::json!({"url": "https://example.com/"})).await.0, 400);
        assert_eq!(post(&base, &token, "/api/update", serde_json::json!({"paths": ["No such mod"]})).await.0, 400);

        #[cfg(feature = "nexus")]
        {
            assert_eq!(post(&base, &token, "/api/cookie", serde_json::json!({"value": "test-value"})).await.0, 204);
            let state: serde_json::Value = wreq::Client::new().get(format!("{base}/api/state")).header(TOKEN_HEADER, &token).send().await.unwrap().json().await.unwrap();
            assert_eq!(state["cookie"], "pasted");

            let forgotten = wreq::Client::new().delete(format!("{base}/api/cookie")).header(TOKEN_HEADER, &token).send().await.unwrap();
            assert_eq!(forgotten.status(), 204);
            let state: serde_json::Value = wreq::Client::new().get(format!("{base}/api/state")).header(TOKEN_HEADER, &token).send().await.unwrap().json().await.unwrap();
            assert_ne!(state["cookie"], "pasted");

            std::env::set_var("NEXUS_COOKIE", "other=1; nexusmods_session=from-env");
            let (status, body) = post(&base, &token, "/api/cookie/environment", serde_json::json!({})).await;
            assert_eq!((status, body.as_str()), (200, r#"{"value":"from-env"}"#));
            std::env::remove_var("NEXUS_COOKIE");
            assert_eq!(post(&base, &token, "/api/cookie/environment", serde_json::json!({})).await.0, 400);
        }
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
        let client = wreq::Client::new();

        let checked: serde_json::Value = client.post(format!("{base}/api/check")).header(TOKEN_HEADER, &token).send().await.unwrap().json().await.unwrap();
        assert!(checked["AutomaticGates"].as_str().is_some());

        let (status, _) = post(&base, &token, "/api/update", serde_json::json!({"paths": ["AutomaticGates"]})).await;
        assert_eq!(status, 200);
        for _ in 0..300 {
            let state: serde_json::Value = client.get(format!("{base}/api/state")).header(TOKEN_HEADER, &token).send().await.unwrap().json().await.unwrap();
            if state["jobs"][0]["finished"] == true {
                let item = &state["jobs"][0]["items"][0];
                assert_eq!(item["status"], "done", "{item}");
                assert!(item["message"].as_str().unwrap().starts_with("0.0.1 -> "));
                return;
            }
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        }
        panic!("update job did not finish");
    }
}
