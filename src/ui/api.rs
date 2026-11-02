//! The JSON API the page talks to.

use std::collections::HashMap;
use std::path::Path;
use std::sync::{Arc, PoisonError};
use anyhow::anyhow;
use axum::extract::State;
use axum::http::StatusCode;
use axum::Json;
use prismatic::manager::{Added, UpdateResult, Verdict};
use prismatic::paths::{default_candidates, default_mods_dir, expand_home};
use prismatic::Manager;
use prismatic::manifest::{dependents, requirements, Mod, UpdateKeys};
use prismatic::modlist::{installable, is_installed, list_json, parse_list};
use prismatic::nexus::{mod_id_from_url, Nexus};
use prismatic::progress::Event;
use serde::{Deserialize, Serialize};
use super::jobs::{Job, Status};
use super::Shared;

/// How long `state` waits for SMAPI to name the sources of mods whose manifests have none
const SOURCE_LOOKUP: std::time::Duration = std::time::Duration::from_secs(3);

type ApiResult<T> = std::result::Result<T, (StatusCode, String)>;

fn bad_request(message: &str) -> (StatusCode, String) {
    (StatusCode::BAD_REQUEST, message.to_string())
}

fn server_error(e: &anyhow::Error) -> (StatusCode, String) {
    (StatusCode::INTERNAL_SERVER_ERROR, e.to_string())
}

/// The manager, or a message asking for a mods folder first
fn manager(state: &Shared) -> ApiResult<Arc<Manager>> {
    state.manager().ok_or_else(|| bad_request("Choose a mods folder first"))
}

fn has_control_chars(value: &str) -> bool {
    value.chars().any(char::is_control)
}

#[derive(Serialize)]
pub struct Candidate {
    path: String,
    found: bool,
}

#[derive(Serialize)]
pub struct ModInfo {
    path: String,
    name: String,
    version: Option<String>,
    author: Option<String>,
    description: Option<String>,
    unique_id: Option<String>,
    keys: Vec<String>,
    /// Whether `update` knows how to update this mod
    updatable: bool,
    /// SMAPI ignores it, its folder starts with a dot
    disabled: bool,
    /// Enabled mods that can't work without this one, so they go off with it
    required_by: Vec<String>,
    /// Disabled mods this one can't work without, so they come on with it
    needs: Vec<String>,
}

#[derive(Serialize)]
pub struct StateResponse {
    /// `None` until a folder is chosen
    mods_dir: Option<String>,
    /// The folder was given on the command line, so a folder the page remembers must not replace it
    explicit: bool,
    /// Usual places for the mods folder on this system
    candidates: Vec<Candidate>,
    mods: Vec<ModInfo>,
    jobs: Vec<Job>,
    /// Manifests that couldn't be read, their mods are missing from `mods`
    warnings: Vec<String>,
    /// `pasted`, `environment`, or nothing (the browser's cookie store is tried on demand)
    cookie: Option<&'static str>,
}

pub async fn state(State(state): State<Shared>) -> ApiResult<Json<StateResponse>> {
    let candidates = default_candidates().into_iter().map(|path| Candidate { found: path.is_dir(), path: path.display().to_string() }).collect();
    let Some(manager) = state.manager() else {
        return Ok(Json(StateResponse {
            mods_dir: None,
            explicit: state.explicit,
            candidates,
            mods: Vec::new(),
            jobs: state.jobs.snapshot(),
            warnings: Vec::new(),
            cookie: state.nexus.cookie_source(),
        }));
    };
    let mods_dir = manager.mods_dir();
    let scan = manager.scan().await.map_err(|e| server_error(&e))?;
    // The lookup is cached and runs on in the background when SMAPI is slow, so the page is never held up
    // for long and a later poll finds the answer
    let lookup = tokio::spawn({
        let (manager, mods) = (manager.clone(), scan.mods.clone());
        async move { manager.resolve_sources(mods).await }
    });
    let found = tokio::time::timeout(SOURCE_LOOKUP, lookup).await.ok().and_then(Result::ok);
    let all = found.as_ref().unwrap_or(&scan.mods);
    let names = |mods: Vec<&Mod>| -> Vec<String> {
        let mut names: Vec<_> = mods.into_iter().map(|m| m.display_name(mods_dir)).collect();
        names.sort_by_key(|name| name.to_lowercase());
        names
    };
    let mut mods: Vec<ModInfo> = all.iter().map(|m| ModInfo {
        path: m.relative(mods_dir),
        name: m.display_name(mods_dir),
        version: m.manifest.version.clone(),
        author: m.manifest.author.clone(),
        description: m.manifest.description.clone(),
        unique_id: m.manifest.unique_id.clone(),
        keys: m.manifest.update_keys.iter().map(ToString::to_string).collect(),
        updatable: m.manifest.update_keys.preferred().is_some_and(|key| key.subkey.is_none()),
        disabled: m.is_disabled(mods_dir),
        required_by: names(dependents(all, m, mods_dir)),
        needs: names(requirements(all, m, mods_dir)),
    }).collect();
    mods.sort_by_key(|m| m.name.to_lowercase());
    Ok(Json(StateResponse {
        mods_dir: Some(mods_dir.display().to_string()),
        explicit: state.explicit,
        candidates,
        mods,
        jobs: state.jobs.snapshot(),
        warnings: scan.errors,
        cookie: state.nexus.cookie_source(),
    }))
}

#[derive(Deserialize)]
pub struct FolderRequest {
    /// `None` or empty goes back to the usual place for this system
    path: Option<String>,
}

/// Points the app at another mods folder. Refused while a job runs, since jobs work in the current one.
pub async fn set_folder(State(state): State<Shared>, Json(request): Json<FolderRequest>) -> ApiResult<StatusCode> {
    let Ok(_idle) = state.work.try_lock() else {
        return Err((StatusCode::CONFLICT, "Wait for the running job to finish before changing the folder".to_string()));
    };
    let dir = match request.path.as_deref().map(str::trim).filter(|path| !path.is_empty()) {
        None => default_mods_dir().map_err(|e| bad_request(&e.to_string()))?,
        Some(path) if path.len() > 4096 || has_control_chars(path) => return Err(bad_request("That doesn't look like a folder path")),
        Some(path) => expand_home(path),
    };
    if !Path::new(&dir).is_dir() {
        return Err(bad_request(&format!("{} is not a folder", dir.display())));
    }
    let manager = Manager::with_nexus(dir, state.nexus.clone()).map_err(|e| bad_request(&e.to_string()))?;
    *state.manager.write().unwrap_or_else(PoisonError::into_inner) = Some(Arc::new(manager));
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Deserialize)]
pub struct CookieRequest {
    value: String,
}

pub async fn set_cookie(State(state): State<Shared>, Json(request): Json<CookieRequest>) -> ApiResult<StatusCode> {
    let value = request.value.trim();
    if value.is_empty() || value.len() > 4096 || has_control_chars(value) {
        return Err(bad_request("That doesn't look like a cookie value"));
    }
    state.nexus.set_cookie(value);
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Serialize)]
pub struct CookieValue {
    value: String,
}

/// Hands the cookie from the environment to the page so it can fill the field, the page is token protected and local
pub async fn cookie_from_environment() -> ApiResult<Json<CookieValue>> {
    Nexus::env_cookie().map(|value| Json(CookieValue { value })).ok_or_else(|| bad_request("The NEXUS_COOKIE variable is not set"))
}

pub async fn cookie_from_browser() -> ApiResult<Json<CookieValue>> {
    let value = tokio::task::spawn_blocking(Nexus::browser_cookie).await
        .map_err(|e| server_error(&anyhow!("{e}")))?
        .map_err(|e| bad_request(&e.to_string()))?;
    Ok(Json(CookieValue { value }))
}

pub async fn forget_cookie(State(state): State<Shared>) -> StatusCode {
    state.nexus.clear_cookie();
    StatusCode::NO_CONTENT
}

/// Enabled mods whose path is in `paths`, or all of them when `paths` is `None`. Disabled mods are left alone.
async fn select_mods(manager: &Manager, paths: Option<&[String]>) -> ApiResult<Vec<Mod>> {
    let mods = manager.mods().await.map_err(|e| server_error(&e))?;
    let mods = manager.resolve_sources(mods).await;
    Ok(mods.into_iter()
        .filter(|m| !m.is_disabled(manager.mods_dir()))
        .filter(|m| paths.is_none_or(|paths| paths.contains(&m.relative(manager.mods_dir()))))
        .collect())
}

#[derive(Deserialize)]
pub struct PathsRequest {
    paths: Option<Vec<String>>,
}

#[derive(Deserialize)]
pub struct EnableRequest {
    paths: Vec<String>,
    enabled: bool,
}

/// Enables or disables the mods at `paths`, with the mods that have to follow them (see `Manager::set_enabled`).
/// Refused while a job runs, since jobs work on the same folders.
pub async fn enable(State(state): State<Shared>, Json(request): Json<EnableRequest>) -> ApiResult<StatusCode> {
    let manager = manager(&state)?;
    let Ok(_idle) = state.work.try_lock() else {
        return Err(bad_request("Wait for the running job to finish"));
    };
    let mods = manager.mods().await.map_err(|e| server_error(&e))?;
    let targets: Vec<Mod> = mods.into_iter().filter(|m| request.paths.contains(&m.relative(manager.mods_dir()))).collect();
    manager.set_enabled(&targets, request.enabled).await.map_err(|e| server_error(&e))?;
    Ok(StatusCode::NO_CONTENT)
}

/// Newest SMAPI-suggested version per mod path, only for mods that have one
pub async fn check(State(state): State<Shared>) -> ApiResult<Json<HashMap<String, String>>> {
    let manager = manager(&state)?;
    let mods = select_mods(&manager, None).await?;
    let versions = manager.suggested_versions(&mods).await.map_err(|e| server_error(&e))?;
    Ok(Json(mods.iter().zip(versions)
        .filter_map(|(m, version)| Some((m.relative(manager.mods_dir()), version?)))
        .collect()))
}

/// Lines and warnings of an install, as one message for its item
fn describe(manager: &Manager, added: &Added) -> String {
    added.lines(manager.mods_dir()).into_iter().chain(added.warnings.iter().cloned()).collect::<Vec<_>>().join("\n")
}

async fn run_update(state: &Shared, manager: &Manager, job: usize, mods: &[Mod]) {
    let verdicts = match manager.verdicts(mods, false).await {
        Ok(verdicts) => verdicts,
        Err(e) => {
            for index in 0..mods.len() {
                state.jobs.finish(job, index, Status::Failed, e.to_string());
            }
            return;
        }
    };
    for (index, (m, verdict)) in mods.iter().zip(verdicts).enumerate() {
        state.jobs.running(job, index);
        match verdict {
            Verdict::Update => {
                let progress = state.jobs.progress(job, index);
                match manager.update_mod(m, false, &progress).await {
                    UpdateResult::Success { new_version } => {
                        let message = m.manifest.version.as_ref().map_or_else(|| new_version.clone(), |old| format!("{old} -> {new_version}"));
                        state.jobs.finish(job, index, Status::Done, message);
                    }
                    UpdateResult::AlreadyUpToDate => state.jobs.finish(job, index, Status::Skipped, "Already up to date".into()),
                    UpdateResult::Failure(message) => state.jobs.finish(job, index, Status::Failed, message),
                }
            }
            Verdict::UpToDate => state.jobs.finish(job, index, Status::Skipped, "Already up to date".into()),
            Verdict::Together(leader) => state.jobs.finish(job, index, Status::Skipped, format!("Same download as {leader}")),
            Verdict::Unsupported(reason) => state.jobs.finish(job, index, Status::Failed, reason.into()),
        }
    }
}

pub async fn update(State(state): State<Shared>, Json(request): Json<PathsRequest>) -> ApiResult<Json<usize>> {
    let manager = manager(&state)?;
    let mods = select_mods(&manager, request.paths.as_deref()).await?;
    if mods.is_empty() {
        return Err(bad_request("No mods selected"));
    }
    let job = state.jobs.start("Update mods", mods.iter().map(|m| m.display_name(manager.mods_dir())).collect());
    tokio::spawn(async move {
        let _turn = state.work.lock().await;
        run_update(&state, &manager, job, &mods).await;
        state.jobs.complete(job);
    });
    Ok(Json(job))
}

#[derive(Deserialize)]
pub struct AddRequest {
    url: String,
}

pub async fn add(State(state): State<Shared>, Json(request): Json<AddRequest>) -> ApiResult<Json<usize>> {
    let manager = manager(&state)?;
    let id = mod_id_from_url(request.url.trim())
        .ok_or_else(|| bad_request("Expected a URL like https://www.nexusmods.com/stardewvalley/mods/<id>"))?
        .to_string();
    let job = state.jobs.start("Add a mod", vec![format!("Nexus:{id}")]);
    tokio::spawn(async move {
        let _turn = state.work.lock().await;
        state.jobs.running(job, 0);
        let progress = state.jobs.progress(job, 0);
        let result = manager.add_nexus(&id, &progress).await;
        progress.emit(Event::Done);
        state.jobs.finish_with(job, 0, result.map(|added| describe(&manager, &added)));
        state.jobs.complete(job);
    });
    Ok(Json(job))
}

#[derive(Serialize)]
pub struct ExportResponse {
    content: String,
    count: usize,
    skipped: Vec<String>,
}

pub async fn export(State(state): State<Shared>, Json(request): Json<PathsRequest>) -> ApiResult<Json<ExportResponse>> {
    let manager = manager(&state)?;
    let mods = select_mods(&manager, request.paths.as_deref()).await?;
    let collected = manager.collect_entries(&mods).await;
    if let Some(error) = &collected.lookup_error {
        eprintln!("Couldn't look up missing Nexus ids on SMAPI: {error}");
    }
    if collected.entries.is_empty() {
        return Err(bad_request("None of the selected mods can be exported"));
    }
    let count = collected.entries.len();
    Ok(Json(ExportResponse {
        content: list_json(collected.entries).map_err(|e| server_error(&e))?,
        count,
        skipped: collected.unexportable,
    }))
}

#[derive(Deserialize)]
pub struct ImportPreviewRequest {
    content: String,
}

#[derive(Serialize)]
pub struct PreviewEntry {
    index: usize,
    name: String,
    version: Option<String>,
    key: String,
    /// `installed`, `installable` or `unsupported`
    state: &'static str,
}

pub async fn import_preview(State(state): State<Shared>, Json(request): Json<ImportPreviewRequest>) -> ApiResult<Json<Vec<PreviewEntry>>> {
    let entries = parse_list(&request.content).map_err(|e| bad_request(&format!("Not a Prismatic mod list: {e}")))?;
    let mods = manager(&state)?.mods().await.map_err(|e| server_error(&e))?;
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
pub struct ImportRequest {
    content: String,
    indexes: Vec<usize>,
}

pub async fn import(State(state): State<Shared>, Json(request): Json<ImportRequest>) -> ApiResult<Json<usize>> {
    let entries = parse_list(&request.content).map_err(|e| bad_request(&format!("Not a Prismatic mod list: {e}")))?;
    let chosen: Vec<_> = request.indexes.iter().filter_map(|index| entries.get(*index).cloned()).collect();
    if chosen.is_empty() {
        return Err(bad_request("No mods selected"));
    }
    let manager = manager(&state)?;
    let job = state.jobs.start("Import mods", chosen.iter().map(|entry| entry.name.clone().unwrap_or_else(|| "Unnamed mod".into())).collect());
    tokio::spawn(async move {
        let _turn = state.work.lock().await;
        for (index, entry) in chosen.iter().enumerate() {
            state.jobs.running(job, index);
            let mods = match manager.mods().await {
                Ok(mods) => mods,
                Err(e) => {
                    state.jobs.finish(job, index, Status::Failed, e.to_string());
                    continue;
                }
            };
            if is_installed(&mods, entry) {
                state.jobs.finish(job, index, Status::Skipped, "Already installed".into());
                continue;
            }
            let progress = state.jobs.progress(job, index);
            let result = manager.install_entry(entry, &progress).await;
            progress.emit(Event::Done);
            state.jobs.finish_with(job, index, result.map(|added| describe(&manager, &added)));
        }
        state.jobs.complete(job);
    });
    Ok(Json(job))
}
