//! Prismatic core: finds installed Stardew Valley mods, checks them against SMAPI and
//! installs or updates them from Nexus Mods and GitHub. The command line and the web UI
//! are front ends on top of [`Manager`].

mod github;
mod http;
mod install;
pub mod manager;
pub mod manifest;
pub mod modlist;
pub mod nexus;
pub mod progress;
mod smapi;

pub use manager::Manager;

#[cfg(test)]
mod testutil {
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// A new empty folder under the system temp dir, unique per call
    pub fn scratch_dir() -> PathBuf {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let dir = std::env::temp_dir().join(format!("prismatic-test-{}-{}", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed)));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }
}
