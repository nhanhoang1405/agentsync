use std::io::IsTerminal;

use agentsync::{config::Config, service, tui};
use anyhow::{Context, Result, bail};
use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(version, about = "Browse and synchronize Codex skills and histories")]
struct Args {
    /// SQL connection URL (or use the saved connection)
    #[arg(long, global = true)]
    database_url: Option<String>,
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Subcommand)]
enum Command {
    /// Sync owned global resources and discovered local projects; newer files win
    SyncAll,
}

fn run() -> Result<()> {
    let args = Args::parse();
    let config = match args.database_url {
        Some(url) => Some(Config::from_url(url)),
        None => Config::load_optional()?,
    };
    match args.command {
        Some(Command::SyncAll) => {
            let config = config.context("a SQL connection URL is required: use --database-url or AGENTSYNC_DATABASE_URL, or connect once in the TUI")?;
            let result = service::sync_all(&config)?;
            println!("{}", tui::summary(&result));
            Ok(())
        }
        None => {
            if !std::io::stdin().is_terminal() || !std::io::stdout().is_terminal() {
                bail!(
                    "the TUI requires a terminal; use agentsync sync-all for noninteractive sync"
                );
            }
            tui::run(config)
        }
    }
}

fn main() {
    if let Err(error) = run() {
        eprintln!("agentsync: {error:#}");
        std::process::exit(1);
    }
}
