mod app;
mod clipboard;
mod output;
mod widgets;

use crate::{config::Config, service::SyncResult};
use anyhow::Result;
use crossterm::{
    event::{DisableBracketedPaste, DisableMouseCapture, EnableBracketedPaste, EnableMouseCapture},
    execute,
};

pub fn summary(result: &SyncResult) -> String {
    format!(
        "Uploaded: {} | Downloaded: {} | Metadata: {} | Unchanged: {} | Conflicts: {}",
        result.uploaded,
        result.written,
        result.metadata_updated,
        result.unchanged,
        result.conflicts
    )
}

struct TerminalGuard;
impl Drop for TerminalGuard {
    fn drop(&mut self) {
        let _ = execute!(
            std::io::stdout(),
            DisableMouseCapture,
            DisableBracketedPaste
        );
        ratatui::restore();
    }
}

pub fn run(config: Option<Config>) -> Result<()> {
    let mut terminal = ratatui::init();
    let _guard = TerminalGuard;
    execute!(std::io::stdout(), EnableMouseCapture, EnableBracketedPaste)?;
    let hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let _ = execute!(
            std::io::stdout(),
            DisableMouseCapture,
            DisableBracketedPaste
        );
        ratatui::restore();
        hook(info);
    }));
    app::App::new(config).run(&mut terminal)
}
