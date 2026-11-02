//! Putting a downloaded archive on disk. Blocking, so callers run it with `spawn_blocking`.

use std::collections::HashSet;
use std::fs;
use std::io::Cursor;
use std::path::{Path, PathBuf};
use anyhow::{anyhow, bail, Result};
use zip::ZipArchive;

/// Backups of replaced mods go here, under the mods directory
pub const OLD_DIR: &str = ".old";

/// Top-level folders of the archive
fn roots(zip: &ZipArchive<Cursor<&[u8]>>) -> HashSet<String> {
    zip.file_names().filter_map(|name| Some(name.split_once('/')?.0.to_string())).collect()
}

/// Extracts an archive of a mod that is not installed yet into `mods_dir`.
/// Returns the folders it created, and touches nothing if one of them already exists.
pub fn extract_new(mods_dir: &Path, archive: &[u8]) -> Result<HashSet<String>> {
    let mut zip = ZipArchive::new(Cursor::new(archive))?;
    let roots = roots(&zip);
    if roots.is_empty() {
        bail!("Zip archive has no top-level folder");
    }
    if let Some(root) = roots.iter().find(|root| mods_dir.join(root).exists()) {
        bail!("File exists: {root}");
    }
    zip.extract(mods_dir)?;
    Ok(roots)
}

/// Replaces the mod in `mod_path` with the one in `archive`, which must hold exactly one top-level folder.
/// The old folder moves to `<mods_dir>/.old/<folder> - <version>` and its `config.json` or `config` is copied
/// into the new one. Returns the new folder, which may be named differently. If extraction fails the old
/// folder is put back.
pub fn replace_mod(mods_dir: &Path, mod_path: &Path, version: Option<&str>, archive: &[u8]) -> Result<PathBuf> {
    let mut zip = ZipArchive::new(Cursor::new(archive))?;
    if zip.is_empty() {
        bail!("Zip archive is empty");
    }
    let mut roots = roots(&zip).into_iter();
    let (Some(root), None) = (roots.next(), roots.next()) else {
        bail!("Zip archive must contain exactly one item in the root");
    };
    let parent = mod_path.parent().ok_or_else(|| anyhow!("Mod path has no parent directory"))?;
    let dir_name = mod_path.file_name().ok_or_else(|| anyhow!("Mod path has no directory name"))?;
    let new_dir = parent.join(&root);
    let old_dir = mods_dir.join(OLD_DIR);
    let backup = old_dir.join(format!("{} - {}", dir_name.to_string_lossy(), version.unwrap_or("unknown")));
    if backup.exists() {
        bail!("File exists: {}", backup.display());
    }
    if new_dir != mod_path && new_dir.exists() {
        bail!("File exists: {root}");
    }

    fs::create_dir_all(&old_dir)?;
    fs::rename(mod_path, &backup)?;
    if let Err(e) = zip.extract(parent) {
        let _ = fs::remove_dir_all(&new_dir);
        let _ = fs::rename(&backup, mod_path);
        return Err(e.into());
    }
    for file in backup.read_dir()?.flatten().filter(|file| ["config.json", "config"].contains(&&*file.file_name().to_string_lossy())) {
        let new_path = new_dir.join(file.file_name());
        if new_path.exists() {
            fs::rename(&new_path, new_path.with_file_name(format!("{}.new", file.file_name().to_string_lossy())))?;
        }
        copy(&file.path(), &new_path)?;
    }
    Ok(new_dir)
}

fn copy(from: &Path, to: &Path) -> Result<()> {
    if from.is_file() {
        fs::copy(from, to)?;
    } else if from.is_dir() {
        fs::create_dir(to)?;
        for file in from.read_dir()?.flatten() {
            copy(&file.path(), &to.join(file.file_name()))?;
        }
    } else {
        bail!("{} is neither a file nor a directory", from.display());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::io::{Cursor, Write};
    use zip::write::FileOptions;
    use zip::ZipWriter;
    use super::{extract_new, replace_mod};
    use crate::testutil::scratch_dir;

    fn archive(files: &[(&str, &str)]) -> Vec<u8> {
        let mut zip = ZipWriter::new(Cursor::new(Vec::new()));
        for (name, text) in files {
            zip.start_file(*name, FileOptions::<()>::default()).unwrap();
            zip.write_all(text.as_bytes()).unwrap();
        }
        zip.finish().unwrap().into_inner()
    }

    fn installed(mods_dir: &std::path::Path, folder: &str, config: &str) {
        fs::create_dir_all(mods_dir.join(folder)).unwrap();
        fs::write(mods_dir.join(folder).join("manifest.json"), "old").unwrap();
        fs::write(mods_dir.join(folder).join("config.json"), config).unwrap();
    }

    #[test]
    fn replaces_a_mod_keeping_a_backup_and_the_config() {
        let mods = scratch_dir();
        installed(&mods, "Mod", "mine");
        let zip = archive(&[("Mod/manifest.json", "new"), ("Mod/config.json", "default")]);

        let new_dir = replace_mod(&mods, &mods.join("Mod"), Some("1.0"), &zip).unwrap();

        assert_eq!(new_dir, mods.join("Mod"));
        assert_eq!(fs::read_to_string(new_dir.join("manifest.json")).unwrap(), "new");
        assert_eq!(fs::read_to_string(new_dir.join("config.json")).unwrap(), "mine");
        assert_eq!(fs::read_to_string(new_dir.join("config.json.new")).unwrap(), "default");
        assert_eq!(fs::read_to_string(mods.join(".old/Mod - 1.0/manifest.json")).unwrap(), "old");
    }

    #[test]
    fn backups_of_nested_mods_go_to_the_mods_directory() {
        let mods = scratch_dir();
        installed(&mods, "Pack/Mod", "mine");
        let zip = archive(&[("Mod/manifest.json", "new")]);
        replace_mod(&mods, &mods.join("Pack/Mod"), None, &zip).unwrap();
        assert!(mods.join(".old/Mod - unknown/manifest.json").is_file());
        assert!(!mods.join("Pack/.old").exists());
    }

    #[test]
    fn a_renamed_root_that_already_exists_leaves_the_old_mod_in_place() {
        let mods = scratch_dir();
        installed(&mods, "Mod", "mine");
        installed(&mods, "Other", "theirs");
        let zip = archive(&[("Other/manifest.json", "new")]);

        let error = replace_mod(&mods, &mods.join("Mod"), Some("1.0"), &zip).unwrap_err();

        assert!(error.to_string().contains("File exists"), "{error}");
        assert_eq!(fs::read_to_string(mods.join("Mod/manifest.json")).unwrap(), "old");
        assert!(!mods.join(".old").exists());
    }

    #[test]
    fn refuses_archives_without_exactly_one_root() {
        let mods = scratch_dir();
        installed(&mods, "Mod", "mine");
        let two = archive(&[("A/manifest.json", "x"), ("B/manifest.json", "x")]);
        assert!(replace_mod(&mods, &mods.join("Mod"), None, &two).is_err());
        assert!(replace_mod(&mods, &mods.join("Mod"), None, &archive(&[])).is_err());
        assert!(mods.join("Mod/manifest.json").is_file());
    }

    #[test]
    fn extracts_new_mods_but_not_over_existing_folders() {
        let mods = scratch_dir();
        let zip = archive(&[("Pack/A/manifest.json", "a"), ("Other/manifest.json", "b")]);

        let roots = extract_new(&mods, &zip).unwrap();
        assert_eq!(roots.len(), 2);
        assert!(mods.join("Pack/A/manifest.json").is_file());

        let error = extract_new(&mods, &zip).unwrap_err();
        assert!(error.to_string().contains("File exists"), "{error}");
    }
}
