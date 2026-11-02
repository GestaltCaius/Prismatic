//! The command line front end: one function per command, printing to the terminal.

use std::fmt;
use std::io::IsTerminal;
use std::path::Path;
use std::process::ExitCode;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use anyhow::{anyhow, bail, Result};
use indicatif::{MultiProgress, ProgressBar, ProgressStyle};
use inquire::MultiSelect;
use prismatic::manager::{Added, UpdateResult, Verdict};
use prismatic::manifest::{Mod, UpdateKeys};
use prismatic::modlist::{is_installed, installable, list_json, parse_list, Entry};
use prismatic::nexus::mod_id_from_url;
use prismatic::progress::{Event, Progress};
use prismatic::Manager;
use tokio::task::JoinSet;

pub const DEFAULT_FILE: &str = "prismatic-mods.json";
/// inquire wraps rows wider than the terminal and corrupts the redraw, so rows are capped well below 80 columns
const LABEL_WIDTH: usize = 60;

fn style(template: &str) -> ProgressStyle {
    ProgressStyle::with_template(template).unwrap_or_else(|_| ProgressStyle::default_bar())
}

/// Shows progress as a bar in `multi`. The bar only appears with the first event, so mods that
/// need no work stay invisible. Bars are hidden when stderr is not a terminal.
fn bar(multi: &MultiProgress, label: &str) -> Progress {
    let multi = multi.clone();
    let label = label.to_string();
    let slot: Mutex<Option<ProgressBar>> = Mutex::new(None);
    Progress::new(move |event| {
        let Ok(mut slot) = slot.lock() else { return };
        if matches!(event, Event::Done) {
            if let Some(bar) = slot.take() {
                bar.finish_and_clear();
            }
            return;
        }
        let bar = slot.get_or_insert_with(|| {
            let bar = multi.add(ProgressBar::new_spinner().with_prefix(label.clone()));
            bar.enable_steady_tick(Duration::from_millis(100));
            bar
        });
        match event {
            Event::Stage(stage) => {
                bar.set_style(style("{prefix:.bold} {spinner} {msg}"));
                bar.unset_length();
                bar.set_message(stage);
            }
            Event::Download { done, total } => {
                if let Some(total) = total {
                    if bar.length() != Some(total) {
                        bar.set_length(total);
                        bar.set_style(style("{prefix:.bold} {msg} {wide_bar} {bytes}/{total_bytes}"));
                    }
                }
                bar.set_message("Downloading");
                bar.set_position(done);
            }
            Event::Done => {}
        }
    })
}

/// An entry as the picker shows it, truncated
struct Row(Entry);

impl fmt::Display for Row {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let label = self.0.describe();
        match label.char_indices().nth(LABEL_WIDTH) {
            Some((end, _)) => write!(f, "{}...", &label[..end]),
            None => f.write_str(&label),
        }
    }
}

/// Everything starts selected, so a subset is made by deselecting.
/// Without a terminal (or with `all`) there is nobody to ask and every entry is kept.
fn pick(entries: Vec<Entry>, prompt: &str, all: bool) -> Result<Vec<Entry>> {
    if all || entries.is_empty() || !(std::io::stdin().is_terminal() && std::io::stderr().is_terminal()) {
        return Ok(entries);
    }
    let defaults: Vec<usize> = (0..entries.len()).collect();
    let picked = MultiSelect::new(prompt, entries.into_iter().map(Row).collect())
        .with_default(&defaults)
        .with_page_size(15)
        .raw_prompt()
        .map_err(|e| anyhow!("Cancelled: {e}"))?;
    Ok(picked.into_iter().map(|option| option.value.0).collect())
}

/// The installed mods, after warning about the manifests that couldn't be read
async fn scan(manager: &Manager) -> Result<Vec<Mod>> {
    let scan = manager.scan().await?;
    for error in &scan.errors {
        eprintln!("Warning: skipped a mod, {error}");
    }
    Ok(scan.mods)
}

fn print_added(added: &Added, mods_dir: &Path) {
    for line in added.lines(mods_dir) {
        println!("{line}");
    }
    for warning in &added.warnings {
        eprintln!("Warning: {warning}");
    }
}

pub async fn list(manager: &Manager) -> Result<()> {
    let mods = scan(manager).await?;
    for m in &mods {
        let manifest = &m.manifest;
        let relative = m.relative(manager.mods_dir());
        match &manifest.name {
            Some(name) => println!("{name} ({relative})"),
            None => println!("{relative}"),
        }
        if let Some(description) = &manifest.description {
            println!("{description}");
        }
        if let Some(author) = &manifest.author {
            println!("Author: {author}");
        }
        if let Some(version) = &manifest.version {
            println!("Version: {version}");
        }
        if let Some(key) = manifest.update_keys.preferred() {
            println!("Source: {}", key.source);
        }
        println!();
    }
    println!("{} mods found", mods.len());
    Ok(())
}

pub async fn add(manager: &Manager, source: &str) -> Result<()> {
    scan(manager).await?;
    let id = mod_id_from_url(source).ok_or_else(|| anyhow!("Expected a URL like https://www.nexusmods.com/stardewvalley/mods/<id>"))?;
    let progress = bar(&MultiProgress::new(), &format!("Nexus:{id}"));
    let result = manager.add_nexus(id, &progress).await;
    progress.emit(Event::Done);
    print_added(&result?, manager.mods_dir());
    Ok(())
}

/// Fails when an update was attempted and went wrong. Mods without a usable source are only reported.
pub async fn update(manager: &Arc<Manager>, force: bool) -> Result<ExitCode> {
    let mods = scan(manager).await?;
    let verdicts = manager.verdicts(&mods, force).await?;
    let multi = MultiProgress::new();
    let (mut updated, mut failed, mut skipped, mut unsupported) = (0, 0, 0, 0);
    let mut set = JoinSet::new();
    for (m, verdict) in mods.into_iter().zip(verdicts) {
        let name = m.display_name(manager.mods_dir());
        match verdict {
            Verdict::Update => {
                let (manager, progress) = (manager.clone(), bar(&multi, &name));
                set.spawn(async move {
                    let result = manager.update_mod(&m, force, &progress).await;
                    (m, result)
                });
            }
            Verdict::UpToDate => {
                skipped += 1;
                multi.suspend(|| eprintln!("{name}: Already up to date"));
            }
            Verdict::Unsupported(reason) => {
                unsupported += 1;
                multi.suspend(|| eprintln!("{name}: {reason}"));
            }
        }
    }
    while let Some(res) = set.join_next().await {
        let (finished, result) = res?;
        let name = finished.display_name(manager.mods_dir());
        match result {
            UpdateResult::Success { new_version } => {
                updated += 1;
                match finished.manifest.version {
                    Some(old_version) => multi.suspend(|| println!("{name}: {old_version} -> {new_version}")),
                    None => multi.suspend(|| println!("{name}: {new_version}")),
                }
            }
            UpdateResult::AlreadyUpToDate => {
                skipped += 1;
                multi.suspend(|| eprintln!("{name}: Already up to date"));
            }
            UpdateResult::Failure(message) => {
                failed += 1;
                multi.suspend(|| eprintln!("{name}: {message}"));
            }
        }
    }
    let mut summary = vec![format!("{updated} updated"), format!("{failed} failed")];
    if !force {
        summary.push(format!("{skipped} already up to date"));
    }
    if unsupported > 0 {
        summary.push(format!("{unsupported} without a usable source"));
    }
    eprintln!("{}", summary.join(", "));
    Ok(exit_code(failed))
}

/// Success unless something failed
const fn exit_code(failed: usize) -> ExitCode {
    if failed == 0 { ExitCode::SUCCESS } else { ExitCode::FAILURE }
}

pub async fn export(manager: &Manager, file: &Path, all: bool) -> Result<()> {
    if file.exists() {
        bail!("File exists: {}", file.display());
    }
    let mods = scan(manager).await?;
    let collected = manager.collect_entries(&mods).await;
    if let Some(error) = &collected.lookup_error {
        eprintln!("Couldn't look up missing Nexus ids on SMAPI: {error}");
    }
    for name in &collected.unexportable {
        eprintln!("{name}: No usable update key, can't be exported");
    }
    let entries = pick(collected.entries, "Pick mods to export (space: toggle, right: all, left: none, enter: confirm)", all)?;
    if entries.is_empty() {
        bail!("No mods selected");
    }
    let count = entries.len();
    std::fs::write(file, list_json(entries)?)?;
    println!("Exported {count} mods to {}", file.display());
    Ok(())
}

/// Fails when an entry could not be installed
pub async fn import(manager: &Manager, file: &Path, all: bool) -> Result<ExitCode> {
    let list = parse_list(&std::fs::read_to_string(file)?)?;
    let entries = pick(list, "Pick mods to install (space: toggle, right: all, left: none, enter: confirm)", all)?;
    scan(manager).await?;
    let multi = MultiProgress::new();
    let (mut installed, mut skipped, mut failed) = (0, 0, 0);
    // Sequential: Nexus rate-limits, and a pack can satisfy later entries
    for entry in entries {
        if is_installed(&manager.mods().await?, &entry) {
            skipped += 1;
            eprintln!("{}: Already installed", entry.describe());
            continue;
        }
        if installable(&entry.keys).is_some_and(|key| key.subkey.is_some()) {
            eprintln!("{}: Subkeys are not yet supported, installing the main file", entry.describe());
        }
        let progress = bar(&multi, entry.name.as_deref().unwrap_or("Unnamed mod"));
        let result = manager.install_entry(&entry, &progress).await;
        progress.emit(Event::Done);
        match result {
            Ok(added) => {
                print_added(&added, manager.mods_dir());
                installed += 1;
            }
            Err(e) => {
                failed += 1;
                eprintln!("{}: {e}", entry.describe());
            }
        }
    }
    eprintln!("{installed} installed, {failed} failed, {skipped} already installed");
    Ok(exit_code(failed))
}
