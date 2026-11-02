//! The web front end: a local server for `ui.html` and the JSON API behind it.

mod api;
mod guard;
mod jobs;
mod page;
#[cfg(test)]
mod tests;

use std::fmt::Write;
use std::sync::Arc;
use anyhow::{anyhow, bail, Result};
use axum::routing::{get, post};
use axum::{middleware, Router};
use prismatic::Manager;
use tokio::net::TcpListener;
use jobs::Jobs;

pub const DEFAULT_PORT: u16 = 47821;

struct AppState {
    manager: Arc<Manager>,
    token: String,
    port: u16,
    jobs: Arc<Jobs>,
    /// Jobs touch the mods folder, so only one runs at a time
    work: tokio::sync::Mutex<()>,
}

type Shared = Arc<AppState>;

fn router(state: Shared) -> Router {
    Router::new()
        .route("/", get(page::page))
        .route("/assets/logo.png", get(page::logo))
        .route("/assets/duck.png", get(page::duck))
        .route("/assets/dog.gif", get(page::dog))
        .route("/api/state", get(api::state))
        .route("/api/cookie", post(api::set_cookie).delete(api::forget_cookie))
        .route("/api/cookie/environment", post(api::cookie_from_environment))
        .route("/api/cookie/browser", post(api::cookie_from_browser))
        .route("/api/check", post(api::check))
        .route("/api/update", post(api::update))
        .route("/api/add", post(api::add))
        .route("/api/export", post(api::export))
        .route("/api/import/preview", post(api::import_preview))
        .route("/api/import", post(api::import))
        .layer(middleware::from_fn_with_state(state.clone(), guard::guard))
        .with_state(state)
}

fn new_state(manager: Arc<Manager>, port: u16) -> Result<Shared> {
    Ok(Arc::new(AppState { manager, token: random_token()?, port, jobs: Arc::default(), work: tokio::sync::Mutex::new(()) }))
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

pub async fn run(manager: Arc<Manager>, port: u16, open: bool) -> Result<()> {
    let listener = bind(port).await?;
    let port = listener.local_addr()?.port();
    let state = new_state(manager, port)?;
    let url = format!("http://127.0.0.1:{port}/?token={}", state.token);
    println!("Prismatic is running at {url}\nPress Ctrl+C to stop.");
    if open {
        open_browser(&url);
    }
    if let Err(e) = axum::serve(listener, router(state)).await {
        bail!("Server stopped: {e}");
    }
    Ok(())
}
