//! Where Stardew Valley keeps its `Mods` folder when the user doesn't say.

use anyhow::{anyhow, bail, Result};
use std::env::home_dir;
use std::path::{Path, PathBuf};

const STEAM_GAME: &str = "steamapps/common/Stardew Valley";

/// Usual places for the `Mods` folder on `os` (a value of `std::env::consts::OS`), most likely first
fn candidates(os: &str, home: Option<&Path>) -> Vec<PathBuf> {
    let in_home = |paths: &[&str]| -> Vec<PathBuf> {
        home.map_or_else(Vec::new, |home| {
            paths.iter().map(|path| home.join(path)).collect()
        })
    };
    match os {
        "macos" => {
            let mut dirs = in_home(&[&format!(
                "Library/Application Support/Steam/{STEAM_GAME}/Contents/MacOS/Mods"
            )]);
            dirs.push(PathBuf::from(
                "/Applications/Stardew Valley.app/Contents/MacOS/Mods",
            ));
            dirs
        }
        "linux" => in_home(&[
            &format!(".steam/steam/{STEAM_GAME}/Mods"),
            &format!(".local/share/Steam/{STEAM_GAME}/Mods"),
            &format!(".var/app/com.valvesoftware.Steam/.local/share/Steam/{STEAM_GAME}/Mods"),
            "GOG Games/Stardew Valley/game/Mods",
        ]),
        "windows" => [
            r"C:\Program Files (x86)\Steam\steamapps\common\Stardew Valley\Mods",
            r"C:\Program Files\Steam\steamapps\common\Stardew Valley\Mods",
            r"C:\Program Files (x86)\GOG Galaxy\Games\Stardew Valley\Mods",
            r"C:\GOG Games\Stardew Valley\Mods",
        ]
        .map(PathBuf::from)
        .to_vec(),
        _ => Vec::new(),
    }
}

fn first_existing(candidates: &[PathBuf]) -> Result<PathBuf> {
    if candidates.is_empty() {
        bail!("Could not determine the mods directory on this system, please specify it with -d <dir>");
    }
    candidates
        .iter()
        .find(|dir| dir.is_dir())
        .cloned()
        .ok_or_else(|| {
            anyhow!(
        "Could not find the mods directory, please specify it with -d <dir>. Looked in:\n{}",
        candidates.iter().map(|dir| format!("  {}", dir.display())).collect::<Vec<_>>().join("\n"),
    )
        })
}

/// Every usual `Mods` folder for this system, found or not
pub fn default_candidates() -> Vec<PathBuf> {
    candidates(std::env::consts::OS, home_dir().as_deref())
}

/// A typed path with a leading `~` standing for the home folder
pub fn expand_home(input: &str) -> PathBuf {
    match (input.strip_prefix('~'), home_dir()) {
        (Some(rest), Some(home)) if rest.is_empty() || rest.starts_with(['/', '\\']) => {
            home.join(rest.trim_start_matches(['/', '\\']))
        }
        _ => PathBuf::from(input),
    }
}

/// The first usual `Mods` folder that exists on this machine
pub fn default_mods_dir() -> Result<PathBuf> {
    first_existing(&default_candidates())
}

#[cfg(test)]
mod tests {
    use super::{candidates, expand_home, first_existing};
    use crate::testutil::scratch_dir;
    use std::path::Path;

    #[test]
    fn every_supported_system_has_candidates_with_steam_first() {
        let home = Path::new("/home/me");
        let mac = candidates("macos", Some(home));
        assert_eq!(mac[0], home.join("Library/Application Support/Steam/steamapps/common/Stardew Valley/Contents/MacOS/Mods"));
        let linux = candidates("linux", Some(home));
        assert_eq!(
            linux[0],
            home.join(".steam/steam/steamapps/common/Stardew Valley/Mods")
        );
        assert!(linux.len() > 1);
        assert!(candidates("windows", None)[0]
            .to_string_lossy()
            .contains("Steam"));
        assert!(candidates("plan9", Some(home)).is_empty());
    }

    #[test]
    fn home_based_systems_have_no_candidates_without_a_home() {
        assert!(candidates("linux", None).is_empty());
    }

    #[test]
    fn a_leading_tilde_means_the_home_folder() {
        let home = std::env::home_dir().unwrap();
        assert_eq!(expand_home("~"), home);
        assert_eq!(expand_home("~/Games/Mods"), home.join("Games/Mods"));
        assert_eq!(expand_home("/opt/~/Mods"), Path::new("/opt/~/Mods"));
        assert_eq!(expand_home("~other/Mods"), Path::new("~other/Mods"));
    }

    #[test]
    fn picks_the_first_folder_that_exists() {
        let dir = scratch_dir();
        let present = dir.join("present");
        std::fs::create_dir(&present).unwrap();
        assert_eq!(
            first_existing(&[dir.join("absent"), present.clone(), dir]).unwrap(),
            present
        );
    }

    #[test]
    fn says_where_it_looked_when_none_exists() {
        let dir = scratch_dir();
        let (a, b) = (dir.join("a"), dir.join("b"));
        let error = first_existing(&[a.clone(), b.clone()])
            .unwrap_err()
            .to_string();
        assert!(
            error.contains("-d <dir>")
                && error.contains(&a.display().to_string())
                && error.contains(&b.display().to_string()),
            "{error}"
        );
        assert!(first_existing(&[]).is_err());
    }
}
