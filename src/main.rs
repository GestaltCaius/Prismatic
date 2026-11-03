mod cli;
mod ui;

use crate::cli::DEFAULT_FILE;
use crate::ui::DEFAULT_PORT;
use anyhow::Result;
use clap::{Parser, Subcommand};
use prismatic::{paths, Manager};
use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::Arc;

#[derive(Debug, Parser)]
#[command(author, version, about)]
struct Args {
    /// Directory that contains mods
    ///
    /// Defaults to the first usual Steam or GOG location that exists for your system,
    /// e.g. ~/.steam/steam/steamapps/common/Stardew Valley/Mods on Linux
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
    /// Makes SMAPI ignore mods, and the mods that need them, by putting a dot in front of their folder name
    Disable {
        /// Mod names, folders or unique IDs, as `list` shows them. Without any, choose from a list
        mods: Vec<String>,
    },
    /// Undoes `disable`, and also enables the disabled mods they need
    Enable {
        /// Mod names, folders or unique IDs, as `list` shows them. Without any, choose from a list
        mods: Vec<String>,
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
            #[cfg(debug_assertions)]
            {
                eprintln!("{err:?}");
            }
            ExitCode::FAILURE
        }
    }
}

/// Exit code: 0 on success, 1 on any error or when an update or import left something undone
async fn main_inner() -> Result<ExitCode> {
    let args = Args::parse();
    let command = args.command.unwrap_or(Command::Ui {
        port: DEFAULT_PORT,
        no_open: false,
    });
    if let Command::Ui { port, no_open } = command {
        // The page can ask for the folder, so not finding one is no reason to stop
        let explicit = args.mods_dir.is_some();
        let mods_dir = args.mods_dir.or_else(|| paths::default_mods_dir().ok());
        ui::run(mods_dir, explicit, port, !no_open).await?;
        return Ok(ExitCode::SUCCESS);
    }
    let mods_dir = args.mods_dir.map_or_else(paths::default_mods_dir, Ok)?;
    let manager = Arc::new(Manager::new(mods_dir)?);
    match command {
        Command::Import { file, all } => return cli::import(&manager, &file, all).await,
        Command::Update { force } => return cli::update(&manager, force).await,
        Command::Enable { mods } => return cli::set_enabled(&manager, &mods, true).await,
        Command::Disable { mods } => return cli::set_enabled(&manager, &mods, false).await,
        Command::Add { source } => cli::add(&manager, &source).await?,
        Command::List => cli::list(&manager).await?,
        Command::Export { file, all } => cli::export(&manager, &file, all).await?,
        Command::Ui { .. } => unreachable!("handled above"),
    }
    Ok(ExitCode::SUCCESS)
}
