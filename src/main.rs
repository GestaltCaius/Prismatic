#[cfg(feature = "nexus")]
mod add;
mod mod_list;
mod mod_locator;
mod list;
mod smapi;
mod update;

use std::env::home_dir;
use std::path::PathBuf;
use clap::{Parser, Subcommand};
use anyhow::{anyhow, bail, Result};
#[cfg(feature = "nexus")]
use crate::add::add;
use crate::list::list;
use crate::mod_list::{export, import, DEFAULT_FILE};
use crate::mod_locator::locate_mods;
use crate::update::update;

#[derive(Debug, Parser)]
#[command(author, version, about)]
struct Args {
    /// Directory that contains mods
    /// 
    /// Defaults to ~/.steam/steam/steamapps/common/Stardew Valley/Mods on Unix based systems
    /// and C:\Program Files (x86)\Steam\steamapps\common\Stardew Valley\Mods on Windows
    #[clap(long, short = 'd')]
    mods_dir: Option<PathBuf>,
    #[clap(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Installs a mod from a Nexus Mods page URL
    Add {
        /// e.g. <https://www.nexusmods.com/stardewvalley/mods/3109>
        source: String,
    },
    /// Not yet implemented
    Remove {
        id: String,
    },
    /// Lists all installed mods
    List,
    /// Exports installed mods to a JSON file you can back up or share
    ///
    /// Asks which mods to include, unless --all is given or there is no terminal
    Export {
        #[clap(default_value = DEFAULT_FILE)]
        file: PathBuf,
        /// Export every mod without asking
        #[clap(long, short)]
        all: bool,
    },
    /// Installs the mods listed in a file made by `export`
    ///
    /// Asks which mods to install, unless --all is given or there is no terminal
    Import {
        file: PathBuf,
        /// Install every mod in the file without asking
        #[clap(long, short)]
        all: bool,
    },
    /// Updates all installed mods if needed
    Update {
        /// Skip the update check and update all mods
        #[clap(long, short)]
        force: bool,
    },
}

#[tokio::main]
async fn main() {
    if let Err(err) = main_inner().await {
        eprintln!("Error: {err}");
        #[cfg(debug_assertions)] {
            eprintln!("{err:?}");
        }
    }
}

async fn main_inner() -> Result<()> {
    let args = Args::parse();
    let mods_dir = args.mods_dir.ok_or(()).or_else(|()|
        if cfg!(unix) {home_dir().map(|home| home.join(".steam/steam/steamapps/common/Stardew Valley/Mods")).ok_or_else(|| anyhow!("Could not determine home directory, please specify the mods directory with -d <dir>"))}
        else if cfg!(windows) {Ok(PathBuf::from(r"C:\Program Files (x86)\Steam\steamapps\common\Stardew Valley\Mods"))}
        else {Err(anyhow!("Could not determine mods directory, please specify with -d <dir>"))}
    )?;
    if !mods_dir.is_dir() {
        bail!("Invalid mods directory");
    }
    let mods = locate_mods(&mods_dir).await?;
    match args.command {
        #[cfg(feature = "nexus")]
        Command::Add { source } => add(&mods, &mods_dir, &source).await?,
        #[cfg(not(feature = "nexus"))]
        Command::Add { .. } => bail!("Prismatic was compiled without Nexus support"),
        Command::Remove { .. } => bail!("Not yet implemented"),
        Command::List => list(&mods, &mods_dir)?,
        Command::Export { file, all } => export(&mods, &file, all).await?,
        Command::Import { file, all } => import(&mods, &mods_dir, &file, all).await?,
        Command::Update { force } => update(&mods, &mods_dir, force).await?,
    }
    Ok(())
}
