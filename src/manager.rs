use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, PoisonError};
use anyhow::{anyhow, bail, Result};
use bytes::Bytes;
use crate::manifest::{cascade, is_nexus_id, locate_mods, locate_mods_at, missing, Mod, Scan, UpdateKey, UpdateKeys};
use crate::modlist::{collect_entries, installable, Collected, Entry};
use crate::nexus::Nexus;
use crate::progress::{Event, Progress};
use crate::{github, http, install, smapi};

/// What the next update should do about one mod
#[derive(Debug)]
pub enum Verdict {
    Update,
    UpToDate,
    /// Several mods come from one download (a pack), the named one takes care of it
    Together(String),
    Unsupported(&'static str),
}

#[derive(Debug)]
pub enum UpdateResult {
    Success {
        new_version: String,
    },
    AlreadyUpToDate,
    Failure(String),
}

/// Mods an install put on disk
#[derive(Debug)]
pub struct Added {
    pub mods: Vec<Mod>,
    pub warnings: Vec<String>,
}

impl Added {
    /// One line per installed mod: name, version and folder
    pub fn lines(&self, mods_dir: &Path) -> Vec<String> {
        self.mods.iter().map(|m| format!(
            "{} {} ({})",
            m.manifest.name.as_deref().unwrap_or("Unnamed mod"),
            m.manifest.version.as_deref().unwrap_or(""),
            m.relative(mods_dir),
        )).collect()
    }
}

/// A mod that installed mods can't work without and that isn't installed
#[derive(Debug, Clone)]
pub struct Missing {
    pub unique_id: String,
    /// Names of the enabled mods that need it
    pub needed_by: Vec<String>,
    /// Its Nexus page according to SMAPI's database. Without one it can't be installed from here.
    pub nexus_id: Option<String>,
}

/// One mods directory and everything needed to look after it
pub struct Manager {
    mods_dir: PathBuf,
    http: wreq::Client,
    nexus: Arc<Nexus>,
    /// Nexus ids SMAPI's database knows, by lowercase `UniqueID`. `None` is a mod it doesn't know.
    known_ids: Mutex<HashMap<String, Option<String>>>,
}

impl std::fmt::Debug for Manager {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Manager").field("mods_dir", &self.mods_dir).finish_non_exhaustive()
    }
}

/// Version in the manifest of a freshly installed mod, picking the one matching `unique_id` out of a pack
fn installed_version(dir: &Path, unique_id: Option<&str>) -> Option<String> {
    let mods = locate_mods_at(dir).ok()?.mods;
    mods.iter()
        .find(|m| m.manifest.unique_id.as_deref().zip(unique_id).is_some_and(|(a, b)| a.eq_ignore_ascii_case(b)))
        .or_else(|| mods.first())?
        .manifest.version.clone()
}

/// The part of a `UniqueID` before the first dot, which is the author's namespace
fn author_namespace(m: &Mod) -> Option<String> {
    Some(m.manifest.unique_id.as_ref()?.split_once('.')?.0.to_lowercase())
}

/// The mods of a pack sit in one folder and share a download, but SMAPI's database may only know one of them.
/// A mod without a source takes the key its siblings (same folder below the mods directory, same author
/// namespace) agree on. Siblings that disagree, or that live loose in the mods directory, give nothing.
fn inherit_pack_keys(mods: &mut [Mod], mods_dir: &Path) {
    let pack = |m: &Mod| Some((m.path.parent().filter(|parent| *parent != mods_dir)?.to_path_buf(), author_namespace(m)?));
    let mut keys: HashMap<(PathBuf, String), Vec<UpdateKey>> = HashMap::new();
    for m in mods.iter() {
        if let (Some(key), Some(pack)) = (m.manifest.update_keys.preferred(), pack(m)) {
            let known = keys.entry(pack).or_default();
            if !known.iter().any(|other| other.to_string().eq_ignore_ascii_case(&key.to_string())) {
                known.push(key.clone());
            }
        }
    }
    for m in mods.iter_mut().filter(|m| m.manifest.update_keys.preferred().is_none()) {
        if let Some([key]) = pack(m).and_then(|pack| keys.get(&pack)).map(Vec::as_slice) {
            m.manifest.update_keys.push(key.clone());
        }
    }
}

impl Manager {
    pub fn new(mods_dir: PathBuf) -> Result<Self> {
        Self::with_nexus(mods_dir, Arc::new(Nexus::new()))
    }

    /// Like `new`, sharing the Nexus session (the pasted cookie) with other managers
    pub fn with_nexus(mods_dir: PathBuf, nexus: Arc<Nexus>) -> Result<Self> {
        if !mods_dir.is_dir() {
            bail!("Invalid mods directory");
        }
        Ok(Self { mods_dir, http: http::client()?, nexus, known_ids: Mutex::default() })
    }

    pub fn mods_dir(&self) -> &Path {
        &self.mods_dir
    }

    pub fn nexus(&self) -> &Nexus {
        &self.nexus
    }

    /// The installed mods and the manifests that couldn't be read, from disk every time
    pub async fn scan(&self) -> Result<Scan> {
        let dir = self.mods_dir.clone();
        tokio::task::spawn_blocking(move || locate_mods(&dir)).await?
    }

    /// Like `scan`, for callers with nothing to do about the unreadable manifests
    pub async fn mods(&self) -> Result<Vec<Mod>> {
        Ok(self.scan().await?.mods)
    }

    /// Enables or disables `targets` and the mods that have to follow them (see [`cascade`]).
    /// Returns every mod that changed, which does not include targets that already were as asked.
    pub async fn set_enabled(&self, targets: &[Mod], enabled: bool) -> Result<Vec<Mod>> {
        let mods = self.mods().await?;
        let changed: Vec<Mod> = cascade(&mods, &targets.iter().collect::<Vec<_>>(), enabled, &self.mods_dir).into_iter().cloned().collect();
        // Taking the dot off a folder enables everything below it, so the mods there that stay off get a dot of their own
        let dotted_parents: Vec<&Path> = changed.iter()
            .flat_map(|m| m.path.ancestors().skip(1).take_while(|dir| *dir != self.mods_dir))
            .filter(|dir| dir.file_name().is_some_and(|name| name.to_string_lossy().starts_with('.')))
            .collect();
        let keep_off: Vec<PathBuf> = if enabled {
            mods.iter()
                .filter(|m| !changed.iter().any(|c| c.path == m.path) && !m.path.file_name().is_some_and(|name| name.to_string_lossy().starts_with('.')))
                .filter(|m| dotted_parents.iter().any(|parent| m.path.starts_with(parent)))
                .map(|m| m.path.clone())
                .collect()
        } else {
            Vec::new()
        };
        let (mods_dir, paths) = (self.mods_dir.clone(), changed.iter().map(|m| m.path.clone()).collect::<Vec<_>>());
        tokio::task::spawn_blocking(move || -> Result<()> {
            for path in &keep_off {
                install::set_enabled(&mods_dir, path, false)?;
            }
            for path in &paths {
                install::set_enabled(&mods_dir, path, enabled)?;
            }
            Ok(())
        }).await??;
        Ok(changed)
    }

    /// Newest version SMAPI suggests for each mod, in the order of `mods`. `None` means up to date.
    pub async fn suggested_versions(&self, mods: &[Mod]) -> Result<Vec<Option<String>>> {
        smapi::suggested_versions(&self.http, mods).await
    }

    /// Gives mods whose manifest has no usable update key the Nexus key SMAPI's database knows for their `UniqueID`.
    /// Asks SMAPI once for all the mods it hasn't been asked about yet, and remembers the answers.
    /// If SMAPI can't be reached the mods come back as they were.
    pub async fn resolve_sources(&self, mut mods: Vec<Mod>) -> Vec<Mod> {
        let sourceless = |m: &Mod| m.manifest.update_keys.preferred().is_none();
        self.learn_nexus_ids(mods.iter().filter(|m| sourceless(m)).filter_map(|m| m.manifest.unique_id.clone()).collect()).await;
        let known = self.known_ids.lock().unwrap_or_else(PoisonError::into_inner);
        for m in mods.iter_mut().filter(|m| sourceless(m)) {
            if let Some(id) = m.manifest.unique_id.as_ref().and_then(|unique_id| known.get(&unique_id.to_lowercase())).cloned().flatten() {
                m.manifest.update_keys.push(UpdateKey { source: "Nexus".into(), id, subkey: None });
            }
        }
        drop(known);
        inherit_pack_keys(&mut mods, &self.mods_dir);
        mods
    }

    /// Asks SMAPI once about the `UniqueID`s it hasn't been asked about yet and remembers the answers.
    /// If SMAPI can't be reached nothing is remembered, so a later call asks again.
    async fn learn_nexus_ids(&self, ids: Vec<String>) {
        let to_ask: Vec<String> = {
            let known = self.known_ids.lock().unwrap_or_else(PoisonError::into_inner);
            ids.into_iter().filter(|id| !known.contains_key(&id.to_lowercase())).collect::<HashSet<_>>().into_iter().collect()
        };
        if to_ask.is_empty() {
            return;
        }
        if let Ok(found) = smapi::nexus_ids(&self.http, &to_ask.iter().map(String::as_str).collect::<Vec<_>>()).await {
            let mut known = self.known_ids.lock().unwrap_or_else(PoisonError::into_inner);
            for id in &to_ask {
                known.insert(id.to_lowercase(), found.get(id).cloned());
            }
        }
    }

    /// The mods that enabled mods in `mods` can't work without and that aren't installed, with their Nexus pages
    /// where SMAPI's database knows them
    pub async fn missing_dependencies(&self, mods: &[Mod]) -> Vec<Missing> {
        let missing = missing(mods, &self.mods_dir);
        self.learn_nexus_ids(missing.iter().map(|(id, _)| id.clone()).collect()).await;
        let known = self.known_ids.lock().unwrap_or_else(PoisonError::into_inner);
        missing.into_iter().map(|(unique_id, needed_by)| Missing {
            nexus_id: known.get(&unique_id.to_lowercase()).cloned().flatten(),
            needed_by: needed_by.into_iter().map(|m| m.display_name(&self.mods_dir)).collect(),
            unique_id,
        }).collect()
    }

    /// Installs a missing mod from its Nexus page. Does nothing, and returns `None`, if an earlier install
    /// (a pack can hold several mods) already provided it.
    pub async fn install_missing(&self, missing: &Missing, progress: &Progress) -> Result<Option<Added>> {
        let provided = |m: &Mod| m.manifest.unique_id.as_deref().is_some_and(|own| own.eq_ignore_ascii_case(&missing.unique_id));
        if self.mods().await?.iter().any(provided) {
            return Ok(None);
        }
        let Some(id) = &missing.nexus_id else {
            bail!("SMAPI's database has no Nexus page for {}, install it yourself", missing.unique_id);
        };
        self.add_nexus(id, progress).await.map(Some)
    }

    /// What an update should do with each of `mods`, asking SMAPI once for all of them.
    /// `force` skips the question and updates every mod that has a source.
    pub async fn verdicts(&self, mods: &[Mod], force: bool) -> Result<Vec<Verdict>> {
        let suggested = if force { vec![None; mods.len()] } else { self.suggested_versions(mods).await? };
        let mut verdicts: Vec<Verdict> = mods.iter().zip(suggested).map(|(m, suggested)| match m.manifest.update_keys.preferred() {
            None => Verdict::Unsupported("Unknown or unsupported source"),
            Some(key) if key.subkey.is_some() => Verdict::Unsupported("Subkeys are not yet supported"),
            Some(_) if force || suggested.is_some() => Verdict::Update,
            Some(_) => Verdict::UpToDate,
        }).collect();
        // The mods of a pack share one key and one download, which replaces them all at once
        let mut leaders: HashMap<String, &Mod> = HashMap::new();
        for (m, verdict) in mods.iter().zip(&mut verdicts).filter(|(_, verdict)| matches!(verdict, Verdict::Update)) {
            let Some(key) = m.manifest.update_keys.preferred() else { continue };
            match leaders.entry(key.to_string().to_lowercase()) {
                std::collections::hash_map::Entry::Occupied(leader) => *verdict = Verdict::Together(leader.get().display_name(&self.mods_dir)),
                std::collections::hash_map::Entry::Vacant(slot) => {
                    slot.insert(m);
                }
            }
        }
        Ok(verdicts)
    }

    /// Updates one mod from its preferred update key, `force` also reinstalls a GitHub release it already has
    pub async fn update_mod(&self, mod_: &Mod, force: bool, progress: &Progress) -> UpdateResult {
        let result = match mod_.manifest.update_keys.preferred() {
            None => Err(anyhow!("Unknown or unsupported source")),
            Some(key) if key.subkey.is_some() => Err(anyhow!("Subkeys are not yet supported")),
            // `preferred` only returns GitHub or Nexus keys
            Some(key) if key.is_source("github") => self.update_from_github(mod_, key, force, progress).await,
            Some(key) => self.update_from_nexus(mod_, key, progress).await,
        };
        progress.emit(Event::Done);
        result.unwrap_or_else(|e| UpdateResult::Failure(e.to_string()))
    }

    async fn update_from_github(&self, mod_: &Mod, key: &UpdateKey, force: bool, progress: &Progress) -> Result<UpdateResult> {
        let release = github::latest(&self.http, &key.id).await?;
        if !force && mod_.manifest.version.as_deref() == Some(release.version.as_str()) {
            return Ok(UpdateResult::AlreadyUpToDate);
        }
        progress.emit(Event::Stage("Starting download"));
        let response = self.http.get(&release.url).send().await?.error_for_status()?;
        let archive = http::read_body(response, progress).await?;
        self.replace(mod_, archive, &release.version, progress).await
    }

    async fn update_from_nexus(&self, mod_: &Mod, key: &UpdateKey, progress: &Progress) -> Result<UpdateResult> {
        let archive = self.nexus.download(&self.http, &key.id, progress).await?;
        self.replace(mod_, archive, "unknown", progress).await
    }

    /// `fallback` is the version reported when the new manifest can't be read
    async fn replace(&self, mod_: &Mod, archive: Bytes, fallback: &str, progress: &Progress) -> Result<UpdateResult> {
        progress.emit(Event::Stage("Extracting"));
        let (mods_dir, path, version) = (self.mods_dir.clone(), mod_.path.clone(), mod_.manifest.version.clone());
        let new_dir = tokio::task::spawn_blocking(move || install::replace_mod(&mods_dir, &path, version.as_deref(), &archive)).await??;
        let new_version = installed_version(&new_dir, mod_.manifest.unique_id.as_deref()).unwrap_or_else(|| fallback.into());
        Ok(UpdateResult::Success { new_version })
    }

    /// Installs a Stardew Valley mod from its Nexus id. A pack can hold several mods, all are installed.
    pub async fn add_nexus(&self, id: &str, progress: &Progress) -> Result<Added> {
        if !is_nexus_id(id) {
            bail!("Invalid Nexus mod id: {id}");
        }
        if let Some(installed) = self.mods().await?.iter().find(|m| m.has_update_key("nexus", id)) {
            bail!("Already installed: {}", installed.relative(&self.mods_dir));
        }
        let archive = self.nexus.download(&self.http, id, progress).await?;
        progress.emit(Event::Stage("Extracting"));
        let dir = self.mods_dir.clone();
        let roots = tokio::task::spawn_blocking(move || install::extract_new(&dir, &archive)).await??;
        progress.emit(Event::Done);

        let mods: Vec<_> = self.mods().await?.into_iter()
            .filter(|m| roots.iter().any(|root| m.path.starts_with(self.mods_dir.join(root))))
            .collect();
        let mut warnings = Vec::new();
        if mods.is_empty() {
            warnings.push(format!("extracted {roots:?} but found no manifest.json"));
        }
        if mods.len() > 1 {
            warnings.push(format!("{} mods installed from one archive, delete the ones you don't want", mods.len()));
        }
        if !mods.is_empty() && !mods.iter().any(|m| m.has_update_key("nexus", id)) {
            warnings.push(format!("no Nexus:{id} update key in the manifest(s), `update` can only update this mod if SMAPI's database knows it"));
        }
        Ok(Added { mods, warnings })
    }

    /// Installs the mod an exported entry describes, from the first source that is supported
    pub async fn install_entry(&self, entry: &Entry, progress: &Progress) -> Result<Added> {
        let Some(key) = installable(&entry.keys) else {
            if entry.keys.iter().any(|key| key.is_source("github")) {
                bail!("GitHub installs are not yet implemented");
            }
            bail!("No supported source, keys: {}", entry.keys.iter().map(ToString::to_string).collect::<Vec<_>>().join(", "));
        };
        self.add_nexus(&key.id, progress).await
    }

    /// Export entries for `mods`, asking SMAPI for the Nexus ids their manifests lack
    pub async fn collect_entries(&self, mods: &[Mod]) -> Collected {
        collect_entries(&self.http, mods).await
    }
}

#[cfg(test)]
mod tests {
    use std::fs;
    use super::{inherit_pack_keys, Manager, Verdict};
    use crate::manifest::UpdateKeys;
    use crate::testutil::scratch_dir;

    fn write_mod(dir: &std::path::Path, folder: &str, manifest: &str) {
        fs::create_dir_all(dir.join(folder)).unwrap();
        fs::write(dir.join(folder).join("manifest.json"), manifest).unwrap();
    }

    #[test]
    fn rejects_a_missing_mods_directory() {
        let error = Manager::new(scratch_dir().join("nope")).err().unwrap();
        assert_eq!(error.to_string(), "Invalid mods directory");
    }

    #[tokio::test]
    async fn scans_the_folder_fresh_every_time() {
        let dir = scratch_dir();
        let manager = Manager::new(dir.clone()).unwrap();
        assert!(manager.mods().await.unwrap().is_empty());
        write_mod(&dir, "A", r#"{"Name":"A"}"#);
        assert_eq!(manager.mods().await.unwrap().len(), 1);
    }

    #[tokio::test]
    async fn scan_keeps_the_readable_mods_and_lists_the_broken_ones() {
        let dir = scratch_dir();
        write_mod(&dir, "Good", r#"{"Name":"Good"}"#);
        write_mod(&dir, "Broken", "{nope");
        let scan = Manager::new(dir).unwrap().scan().await.unwrap();
        assert_eq!((scan.mods.len(), scan.errors.len()), (1, 1));
    }

    #[tokio::test]
    async fn pack_mods_without_a_source_take_the_one_their_siblings_agree_on() {
        let dir = scratch_dir();
        write_mod(&dir, "Pack/Code", r#"{"Name":"Code","UniqueID":"Author.Code","UpdateKeys":["Nexus:5"]}"#);
        write_mod(&dir, "Pack/CP", r#"{"Name":"CP","UniqueID":"author.CP","UpdateKeys":["Nexus:???"]}"#);
        write_mod(&dir, "Pack/Other", r#"{"Name":"Other","UniqueID":"Someone.Other"}"#);
        write_mod(&dir, "Solo", r#"{"Name":"Solo","UniqueID":"Author.Solo"}"#);
        write_mod(&dir, "Split/A", r#"{"Name":"A","UniqueID":"Au.A","UpdateKeys":["Nexus:1"]}"#);
        write_mod(&dir, "Split/B", r#"{"Name":"B","UniqueID":"Au.B","UpdateKeys":["Nexus:2"]}"#);
        write_mod(&dir, "Split/C", r#"{"Name":"C","UniqueID":"Au.C"}"#);
        let mut mods = Manager::new(dir.clone()).unwrap().mods().await.unwrap();

        inherit_pack_keys(&mut mods, &dir);

        let key = |name: &str| mods.iter().find(|m| m.manifest.name.as_deref() == Some(name)).unwrap().manifest.update_keys.preferred().map(ToString::to_string);
        assert_eq!(key("CP").as_deref(), Some("Nexus:5"));
        assert_eq!(key("Other"), None, "another author's mod is not part of the pack");
        assert_eq!(key("Solo"), None, "loose mods have no siblings");
        assert_eq!(key("C"), None, "siblings that disagree give nothing");
    }

    #[tokio::test]
    async fn mods_of_a_pack_share_one_download() {
        let dir = scratch_dir();
        write_mod(&dir, "Pack/[CP] Pack", r#"{"Name":"CP","UpdateKeys":["Nexus:3753"]}"#);
        write_mod(&dir, "Pack/[FTM] Pack", r#"{"Name":"FTM","UpdateKeys":["nexus:3753"]}"#);
        write_mod(&dir, "Other", r#"{"Name":"Other","UpdateKeys":["Nexus:1"]}"#);
        let manager = Manager::new(dir).unwrap();
        let mut mods = manager.mods().await.unwrap();
        mods.sort_by_key(|m| m.manifest.name.clone());

        let verdicts = manager.verdicts(&mods, true).await.unwrap();

        // sorted: CP, FTM, Other
        assert!(matches!(verdicts[0], Verdict::Update));
        assert!(matches!(&verdicts[1], Verdict::Together(leader) if leader == "CP"));
        assert!(matches!(verdicts[2], Verdict::Update));
    }

    #[tokio::test]
    async fn resolving_leaves_mods_that_already_have_a_source_or_no_id_alone() {
        let dir = scratch_dir();
        write_mod(&dir, "Keyed", r#"{"Name":"Keyed","UniqueID":"x.keyed","UpdateKeys":["Nexus:1"]}"#);
        write_mod(&dir, "NoId", r#"{"Name":"NoId"}"#);
        let manager = Manager::new(dir).unwrap();
        let mods = manager.mods().await.unwrap();

        let resolved = manager.resolve_sources(mods).await;

        assert_eq!(resolved.iter().map(|m| m.manifest.update_keys.len()).sum::<usize>(), 1);
    }

    #[tokio::test]
    async fn forced_updates_skip_the_check_and_unsupported_mods_say_why() {
        let dir = scratch_dir();
        write_mod(&dir, "Keyed", r#"{"Name":"Keyed","UpdateKeys":["Nexus:1"]}"#);
        write_mod(&dir, "Sub", r#"{"Name":"Sub","UpdateKeys":["Nexus:2@beta"]}"#);
        write_mod(&dir, "Bare", r#"{"Name":"Bare"}"#);
        let manager = Manager::new(dir).unwrap();
        let mut mods = manager.mods().await.unwrap();
        mods.sort_by_key(|m| m.manifest.name.clone());

        let verdicts = manager.verdicts(&mods, true).await.unwrap();

        assert!(matches!(verdicts[0], Verdict::Unsupported("Unknown or unsupported source")));
        assert!(matches!(verdicts[1], Verdict::Update));
        assert!(matches!(verdicts[2], Verdict::Unsupported("Subkeys are not yet supported")));
    }

    #[tokio::test]
    async fn disabling_follows_the_mods_that_need_it_and_enabling_the_mods_it_needs() {
        let dir = scratch_dir();
        write_mod(&dir, "Frame", r#"{"Name":"Frame","UniqueID":"a.frame"}"#);
        write_mod(&dir, "Lib", r#"{"Name":"Lib","UniqueID":"a.lib","Dependencies":[{"UniqueID":"A.Frame"}]}"#);
        write_mod(&dir, "Pack", r#"{"Name":"Pack","UniqueID":"a.pack","ContentPackFor":{"UniqueID":"a.lib"}}"#);
        write_mod(&dir, "Free", r#"{"Name":"Free","UniqueID":"a.free","Dependencies":[{"UniqueID":"a.frame","IsRequired":false}]}"#);
        let manager = Manager::new(dir.clone()).unwrap();
        let find = |mods: &[crate::manifest::Mod], name: &str| mods.iter().find(|m| m.manifest.name.as_deref() == Some(name)).unwrap().clone();

        let mods = manager.mods().await.unwrap();
        let changed = manager.set_enabled(&[find(&mods, "Frame")], false).await.unwrap();
        assert_eq!(changed.len(), 3);
        for folder in [".Frame", ".Lib", ".Pack", "Free"] {
            assert!(dir.join(folder).is_dir(), "{folder}");
        }

        let mods = manager.mods().await.unwrap();
        let changed = manager.set_enabled(&[find(&mods, "Pack")], true).await.unwrap();
        assert_eq!(changed.len(), 3, "Pack brings Lib and Frame");
        assert!(dir.join("Frame").is_dir() && dir.join("Pack").is_dir());
        assert!(manager.set_enabled(&[find(&manager.mods().await.unwrap(), "Pack")], true).await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn enabling_a_mod_of_a_disabled_pack_keeps_its_siblings_off() {
        let dir = scratch_dir();
        write_mod(&dir, ".Pack/A", r#"{"Name":"A","UniqueID":"x.a"}"#);
        write_mod(&dir, ".Pack/B", r#"{"Name":"B","UniqueID":"x.b"}"#);
        let manager = Manager::new(dir.clone()).unwrap();
        let mods = manager.mods().await.unwrap();
        let a = mods.iter().find(|m| m.manifest.name.as_deref() == Some("A")).unwrap().clone();

        manager.set_enabled(&[a], true).await.unwrap();

        assert!(dir.join("Pack/A").is_dir() && dir.join("Pack/.B").is_dir());
    }

    #[test]
    fn a_requirement_an_enabled_mod_already_provides_is_not_enabled_again() {
        use crate::manifest::{cascade, locate_mods};
        let dir = scratch_dir();
        write_mod(&dir, "Needs", r#"{"Name":"Needs","UniqueID":"x.needs","Dependencies":[{"UniqueID":"x.lib"}]}"#);
        write_mod(&dir, "Lib", r#"{"Name":"Lib","UniqueID":"x.lib"}"#);
        write_mod(&dir, ".Lib old", r#"{"Name":"Lib old","UniqueID":"X.LIB"}"#);
        let mods = locate_mods(&dir).unwrap().mods;
        let needs = mods.iter().find(|m| m.manifest.name.as_deref() == Some("Needs")).unwrap();
        assert!(cascade(&mods, &[needs], true, &dir).is_empty());
    }

    #[test]
    fn missing_lists_what_no_installed_mod_provides_once_with_everything_that_needs_it() {
        use crate::manifest::{locate_mods, missing};
        let dir = scratch_dir();
        write_mod(&dir, "A", r#"{"Name":"A","UniqueID":"x.a","Dependencies":[{"UniqueID":"x.lib"},{"UniqueID":"X.Lib"},{"UniqueID":"x.opt","IsRequired":false}]}"#);
        write_mod(&dir, "B", r#"{"Name":"B","UniqueID":"x.b","ContentPackFor":{"UniqueID":"X.LIB"},"Dependencies":[{"UniqueID":"x.off"}]}"#);
        write_mod(&dir, ".Off", r#"{"Name":"Off","UniqueID":"x.off"}"#);
        write_mod(&dir, ".C", r#"{"Name":"C","UniqueID":"x.c","Dependencies":[{"UniqueID":"x.gone"}]}"#);
        let mods = locate_mods(&dir).unwrap().mods;

        let found = missing(&mods, &dir);

        assert_eq!(found.len(), 1, "an installed but disabled mod and a disabled mod's needs are not missing");
        let (id, needed_by) = &found[0];
        assert_eq!(id, "x.lib");
        let mut names: Vec<_> = needed_by.iter().filter_map(|m| m.manifest.name.clone()).collect();
        names.sort();
        assert_eq!(names, ["A", "B"]);
    }

    #[tokio::test]
    async fn installing_a_missing_mod_skips_what_is_there_and_says_when_nexus_has_no_page() {
        use super::Missing;
        let dir = scratch_dir();
        write_mod(&dir, "Lib", r#"{"Name":"Lib","UniqueID":"x.lib"}"#);
        let manager = Manager::new(dir).unwrap();
        let progress = crate::progress::Progress::new(|_| {});
        let missing = |id: &str| Missing { unique_id: id.into(), needed_by: Vec::new(), nexus_id: None };

        assert!(manager.install_missing(&missing("X.LIB"), &progress).await.unwrap().is_none());
        let Err(error) = manager.install_missing(&missing("x.other"), &progress).await else { panic!("expected an error") };
        assert!(error.to_string().contains("no Nexus page"), "{error}");
    }
}
