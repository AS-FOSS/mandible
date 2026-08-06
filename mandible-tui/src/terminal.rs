//! Terminal setup/teardown. Kept separate from [`crate::render`] so
//! rendering can always be exercised in tests via
//! `ratatui::backend::TestBackend`, without a real tty.

use crossterm::event::{DisableMouseCapture, EnableMouseCapture};
use crossterm::execute;
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use ratatui::backend::CrosstermBackend;
use ratatui::Terminal;
use std::io::{self, IsTerminal, Stdout};

/// A live terminal handle, backed by `crossterm`.
pub type Term = Terminal<CrosstermBackend<Stdout>>;

/// True if stdout is an interactive terminal. mantui must check this and
/// fail with a clear message rather than let `enable_raw_mode` produce an
/// opaque OS error ("No such device or address") when stdout is redirected
/// or there is no controlling tty.
pub fn stdout_is_tty() -> bool {
    io::stdout().is_terminal()
}

/// Enter raw mode, the alternate screen, and mouse capture. Callers must
/// pair this with [`restore`] (ideally via a panic hook / drop guard in the
/// binary) so a crash doesn't leave the user's terminal in raw mode.
pub fn init() -> io::Result<Term> {
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen, EnableMouseCapture)?;
    let backend = CrosstermBackend::new(stdout);
    Terminal::new(backend)
}

/// Leave raw mode, the alternate screen, and mouse capture. Safe to call
/// even if `init` partially failed.
pub fn restore() -> io::Result<()> {
    disable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, LeaveAlternateScreen, DisableMouseCapture)?;
    Ok(())
}
