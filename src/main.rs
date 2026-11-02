mod cli;
mod ui;

use std::env::home_dir;
use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::Arc;
use anyhow::{anyhow, Result};
use clap::{Parser, Subcommand};
use prismatic::Manager;
use crate::cli::DEFAULT_FILE;
use crate::ui::DEFAULT_PORT;

#[derive(Debug, Parser)]
#[command(author, version, about)]
struct Args {
    /// Directory that contains mods
    ///
    /// Defaults to ~/.steam/steam/steamapps/common/Stardew Valley/Mods on Unix based systems
    /// and C:\Program Files (x86)\Steam\steamapps\common\Stardew Valley\Mods on Windows
    #[clap(long, short = 'd')]
    mods_dir: Option<PathBuf>,
    /// Opens the web interface when no command is given
    #[clap(subcommand)]
    command: Option<Command>,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Installs a mod from a Nexus Mods page URL
    Add {
        /// e.g. <https://www.nexusmods.com/stardewvalley/mods/3109>
        source: String,
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
    /// Opens a web interface on this machine to manage mods from the browser
    Ui {
        /// Port to listen on (127.0.0.1 only), a free one is used if it is taken
        #[clap(long, default_value_t = DEFAULT_PORT)]
        port: u16,
        /// Don't open the browser, just print the address
        #[clap(long)]
        no_open: bool,
    },
    /// Updates all installed mods if needed
    Update {
        /// Skip the update check and update all mods
        #[clap(long, short)]
        force: bool,
    },
}

#[tokio::main]
async fn main() -> ExitCode {
    match main_inner().await {
        Ok(code) => code,
        Err(err) => {
            eprintln!("Error: {err}");
            #[cfg(debug_assertions)] {
                eprintln!("{err:?}");
            }
            ExitCode::FAILURE
        }
    }
}

fn default_mods_dir() -> Result<PathBuf> {
    if cfg!(unix) {
        home_dir()
            .map(|home| home.join(".steam/steam/steamapps/common/Stardew Valley/Mods"))
            .ok_or_else(|| anyhow!("Could not determine home directory, please specify the mods directory with -d <dir>"))
    } else if cfg!(windows) {
        Ok(PathBuf::from(r"C:\Program Files (x86)\Steam\steamapps\common\Stardew Valley\Mods"))
    } else {
        Err(anyhow!("Could not determine mods directory, please specify with -d <dir>"))
    }
}

/// Exit code: 0 on success, 1 on any error or when an update or import left something undone
async fn main_inner() -> Result<ExitCode> {
    let args = Args::parse();
    let mods_dir = args.mods_dir.map_or_else(default_mods_dir, Ok)?;
    let manager = Arc::new(Manager::new(mods_dir)?);
    match args.command.unwrap_or(Command::Ui { port: DEFAULT_PORT, no_open: false }) {
        Command::Import { file, all } => return cli::import(&manager, &file, all).await,
        Command::Update { force } => return cli::update(&manager, force).await,
        Command::Add { source } => cli::add(&manager, &source).await?,
        Command::List => cli::list(&manager).await?,
        Command::Export { file, all } => cli::export(&manager, &file, all).await?,
        Command::Ui { port, no_open } => ui::run(manager, port, !no_open).await?,
    }
    Ok(ExitCode::SUCCESS)
}
