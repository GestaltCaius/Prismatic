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
fn roots(zip: &ZipArchive<Cursor<&[u8]>>) -> Result<HashSet<String>> {
    let mut roots = HashSet::new();
    for name in zip.file_names() {
        if let Some((root, _)) = name?.split_once('/') {
            roots.insert(root.to_string());
        }
    }
    Ok(roots)
}

/// Extracts an archive of a mod that is not installed yet into `mods_dir`.
/// Returns the folders it created, and touches nothing if one of them already exists.
pub fn extract_new(mods_dir: &Path, archive: &[u8]) -> Result<HashSet<String>> {
    let mut zip = ZipArchive::new(Cursor::new(archive))?;
    let roots = roots(&zip)?;
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
/// A mod that belongs to a pack sits inside the folder the archive is rooted at, and that whole folder is replaced.
/// The old folder moves to `<mods_dir>/.old/<folder> - <version>`, and the `config.json` or `config` of each mod in it
/// is copied into the new one. Returns the new folder. If extraction fails the old folder is put back.
pub fn replace_mod(mods_dir: &Path, mod_path: &Path, version: Option<&str>, archive: &[u8]) -> Result<PathBuf> {
    let mut zip = ZipArchive::new(Cursor::new(archive))?;
    if zip.is_empty() {
        bail!("Zip archive is empty");
    }
    let mut roots = roots(&zip)?.into_iter();
    let (Some(root), None) = (roots.next(), roots.next()) else {
        bail!("Zip archive must contain exactly one item in the root");
    };
    let target = mod_path.ancestors()
        .take_while(|dir| dir.starts_with(mods_dir) && *dir != mods_dir)
        .find(|dir| dir.file_name().is_some_and(|name| name == root.as_str()))
        .unwrap_or(mod_path);
    if target == mod_path && mod_path.file_name().is_some_and(|name| name != root.as_str()) && zip.file_names().filter(|name| name.as_ref().is_ok_and(|name| name.ends_with("manifest.json"))).count() > 1 {
        bail!("This download is a pack of mods, but the mod is not in a folder named {root}. Update it by hand or rename the pack folder");
    }
    let parent = target.parent().ok_or_else(|| anyhow!("Mod path has no parent directory"))?;
    let dir_name = target.file_name().ok_or_else(|| anyhow!("Mod path has no directory name"))?;
    let new_dir = parent.join(&root);
    let old_dir = mods_dir.join(OLD_DIR);
    let backup = old_dir.join(format!("{} - {}", dir_name.to_string_lossy(), version.unwrap_or("unknown")));
    if backup.exists() {
        bail!("File exists: {}", backup.display());
    }
    if new_dir != target && new_dir.exists() {
        bail!("File exists: {root}");
    }

    fs::create_dir_all(&old_dir)?;
    fs::rename(target, &backup)?;
    if let Err(e) = zip.extract(parent) {
        let _ = fs::remove_dir_all(&new_dir);
        let _ = fs::rename(&backup, target);
        return Err(e.into());
    }
    carry_configs(&backup, &new_dir)?;
    Ok(new_dir)
}

/// Disables or enables the mod in `mod_path` by renaming its folder, since SMAPI ignores folders that start with a dot.
/// Enabling also takes the dot off the folders above it, deepest first, so the whole path is loaded again.
pub fn set_enabled(mods_dir: &Path, mod_path: &Path, enabled: bool) -> Result<()> {
    let parts: Vec<_> = mod_path.strip_prefix(mods_dir)?.iter().collect();
    for depth in (0..parts.len()).rev() {
        let name = parts[depth].to_string_lossy();
        let renamed = match (enabled, name.strip_prefix('.')) {
            (true, Some(visible)) => visible.to_string(),
            (false, None) if depth == parts.len() - 1 => format!(".{name}"),
            _ => continue,
        };
        let from = mods_dir.join(parts[..=depth].iter().collect::<PathBuf>());
        // A sibling enabled earlier in the same batch may have renamed this folder already
        if !from.exists() {
            continue;
        }
        let to = from.with_file_name(&renamed);
        if to.exists() {
            bail!("Can't rename {} to {renamed}, a folder with that name already exists", from.display());
        }
        fs::rename(&from, &to)?;
    }
    Ok(())
}

/// Folders at or below `dir` that hold a manifest, found without reading it so a broken manifest doesn't matter
fn mod_dirs(dir: &Path, found: &mut Vec<PathBuf>) {
    if dir.join("manifest.json").is_file() {
        found.push(dir.to_path_buf());
        return;
    }
    for entry in dir.read_dir().into_iter().flatten().flatten() {
        if entry.file_type().is_ok_and(|kind| kind.is_dir()) && !entry.file_name().to_string_lossy().starts_with('.') {
            mod_dirs(&entry.path(), found);
        }
    }
}

/// Copies the config of the old folder and of every mod below it into the same place in the new one
fn carry_configs(backup: &Path, new_dir: &Path) -> Result<()> {
    let mut dirs = vec![backup.to_path_buf()];
    mod_dirs(backup, &mut dirs);
    dirs.dedup();
    for old in dirs {
        let new_root = new_dir.join(old.strip_prefix(backup)?);
        if !new_root.is_dir() {
            continue;
        }
        for file in old.read_dir()?.flatten().filter(|file| ["config.json", "config"].contains(&&*file.file_name().to_string_lossy())) {
            let new_path = new_root.join(file.file_name());
            if new_path.exists() {
                fs::rename(&new_path, new_path.with_file_name(format!("{}.new", file.file_name().to_string_lossy())))?;
            }
            copy(&file.path(), &new_path)?;
        }
    }
    Ok(())
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
    use zip::write::SimpleFileOptions;
    use zip::ZipWriter;
    use super::{extract_new, replace_mod, set_enabled};
    use crate::testutil::scratch_dir;

    fn archive(files: &[(&str, &str)]) -> Vec<u8> {
        let mut zip = ZipWriter::new(Cursor::new(Vec::new()));
        for (name, text) in files {
            zip.start_file(*name, SimpleFileOptions::default()).unwrap();
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
    fn disabling_puts_a_dot_on_the_mod_folder_and_enabling_takes_every_dot_off() {
        let mods = scratch_dir();
        for folder in ["Pack/A", "Pack/B", "Solo"] {
            fs::create_dir_all(mods.join(folder)).unwrap();
        }
        set_enabled(&mods, &mods.join("Pack/A"), false).unwrap();
        set_enabled(&mods, &mods.join("Solo"), false).unwrap();
        assert!(mods.join("Pack/.A").is_dir() && mods.join(".Solo").is_dir());

        // a whole pack disabled by hand, then both its mods enabled in one batch
        fs::rename(mods.join("Pack"), mods.join(".Pack")).unwrap();
        set_enabled(&mods, &mods.join(".Pack/.A"), true).unwrap();
        set_enabled(&mods, &mods.join(".Pack/B"), true).unwrap();
        assert!(mods.join("Pack/A").is_dir() && mods.join("Pack/B").is_dir() && !mods.join(".Pack").exists());
    }

    #[test]
    fn enabling_refuses_to_replace_a_folder_that_exists() {
        let mods = scratch_dir();
        fs::create_dir_all(mods.join(".A")).unwrap();
        fs::create_dir_all(mods.join("A")).unwrap();
        assert!(set_enabled(&mods, &mods.join(".A"), true).unwrap_err().to_string().contains("already exists"));
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
    fn updating_a_mod_of_a_pack_replaces_the_whole_pack_and_keeps_each_mods_config() {
        let mods = scratch_dir();
        installed(&mods, "Pack/[CP] Pack", "cp config");
        installed(&mods, "Pack/[FTM] Pack", "ftm config");
        let zip = archive(&[
            ("Pack/[CP] Pack/manifest.json", "new cp"), ("Pack/[CP] Pack/config.json", "default"),
            ("Pack/[FTM] Pack/manifest.json", "new ftm"),
        ]);

        let new_dir = replace_mod(&mods, &mods.join("Pack/[CP] Pack"), Some("1.0"), &zip).unwrap();

        assert_eq!(new_dir, mods.join("Pack"));
        assert!(!mods.join("Pack/Pack").exists(), "must not nest a second copy of the pack");
        assert_eq!(fs::read_to_string(new_dir.join("[CP] Pack/manifest.json")).unwrap(), "new cp");
        assert_eq!(fs::read_to_string(new_dir.join("[CP] Pack/config.json")).unwrap(), "cp config");
        assert_eq!(fs::read_to_string(new_dir.join("[FTM] Pack/config.json")).unwrap(), "ftm config");
        assert_eq!(fs::read_to_string(mods.join(".old/Pack - 1.0/[FTM] Pack/manifest.json")).unwrap(), "old");
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
    fn a_pack_download_is_refused_for_a_mod_outside_a_folder_named_like_its_root() {
        let mods = scratch_dir();
        installed(&mods, "Renamed/[CP] Pack", "mine");
        let zip = archive(&[("Pack/[CP] Pack/manifest.json", "new"), ("Pack/[FTM] Pack/manifest.json", "new")]);

        let error = replace_mod(&mods, &mods.join("Renamed/[CP] Pack"), Some("1.0"), &zip).unwrap_err();

        assert!(error.to_string().contains("pack of mods"), "{error}");
        assert_eq!(fs::read_to_string(mods.join("Renamed/[CP] Pack/manifest.json")).unwrap(), "old");
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

    #[test]
    #[cfg(unix)]
    fn a_symlink_entry_pointing_outside_the_mods_folder_is_not_followed() {
        let (mods, outside) = (scratch_dir(), scratch_dir());
        let mut zip = ZipWriter::new(Cursor::new(Vec::new()));
        zip.start_file("Mod/manifest.json", SimpleFileOptions::default()).unwrap();
        zip.write_all(b"{}").unwrap();
        zip.add_symlink("Mod/escape", outside.to_str().unwrap(), SimpleFileOptions::default()).unwrap();
        let bytes = zip.finish().unwrap().into_inner();

        let _ = extract_new(&mods, &bytes);

        let escape = mods.join("Mod/escape");
        assert!(!escape.is_symlink() || !escape.canonicalize().is_ok_and(|target| target.starts_with(outside.canonicalize().unwrap())),
            "extraction left a link to a folder outside the mods folder");
    }
}
