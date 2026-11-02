//! Runs the real binary against throwaway mods folders. Nothing here needs the network:
//! every case stops before SMAPI or Nexus would be asked.

// clippy.toml only relaxes unwrap inside #[test] functions, not in the helpers around them
#![allow(clippy::unwrap_used)]

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::atomic::{AtomicUsize, Ordering};

fn mods_dir(manifests: &[(&str, &str)]) -> PathBuf {
    static NEXT: AtomicUsize = AtomicUsize::new(0);
    let dir = std::env::temp_dir().join(format!(
        "prismatic-cli-test-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    for (folder, manifest) in manifests {
        fs::create_dir_all(dir.join(folder)).unwrap();
        fs::write(dir.join(folder).join("manifest.json"), manifest).unwrap();
    }
    fs::create_dir_all(&dir).unwrap();
    dir
}

fn run(dir: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_prismatic"))
        .arg("-d")
        .arg(dir)
        .args(args)
        .env_remove("NEXUS_COOKIE")
        .output()
        .unwrap()
}

fn stdout(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

#[test]
fn a_missing_mods_directory_exits_1() {
    let output = run(&mods_dir(&[]).join("missing"), &["list"]);
    assert_eq!(output.status.code(), Some(1));
    assert!(stderr(&output).contains("Invalid mods directory"));
}

#[test]
fn list_skips_a_broken_manifest_with_a_warning_and_still_succeeds() {
    let dir = mods_dir(&[
        ("Good", r#"{"Name":"Good","Version":"1.0"}"#),
        ("Broken", "{nope"),
    ]);
    let output = run(&dir, &["list"]);
    assert_eq!(output.status.code(), Some(0));
    assert!(stdout(&output).contains("Good") && stdout(&output).contains("1 mods found"));
    let errors = stderr(&output);
    assert!(
        errors.contains("Warning: skipped a mod") && errors.contains("Broken"),
        "{errors}"
    );
}

#[test]
fn mods_without_a_usable_source_are_reported_but_are_not_failures() {
    let dir = mods_dir(&[
        ("Bare", r#"{"Name":"Bare"}"#),
        (
            "Placeholder",
            r#"{"Name":"Placeholder","UpdateKeys":["Nexus:???"]}"#,
        ),
    ]);
    let output = run(&dir, &["update"]);
    assert_eq!(output.status.code(), Some(0));
    let errors = stderr(&output);
    assert!(
        errors.contains("0 failed") && errors.contains("2 without a usable source"),
        "{errors}"
    );
}

#[test]
fn commands_that_cannot_do_their_job_exit_1() {
    let dir = mods_dir(&[("Good", r#"{"Name":"Good"}"#)]);
    assert_eq!(
        run(&dir, &["add", "https://example.com"]).status.code(),
        Some(1)
    );
    assert_eq!(
        run(&dir, &["import", "--all", "no-such-file.json"])
            .status
            .code(),
        Some(1)
    );
    assert_eq!(
        run(
            &dir,
            &["export", "--all", dir.join("out.json").to_str().unwrap()]
        )
        .status
        .code(),
        Some(1),
        "nothing exportable"
    );
}
