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

/// True if stdout is an interactive terminal. mandible must check this and
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
///
/// Order is load-bearing and the reverse of [`init`]: mouse capture is
/// switched off and pending input drained *while the terminal is still
/// raw*. The original order disabled raw mode first, leaving a window
/// where the terminal was cooked but still reporting mouse motion — any
/// movement during quit landed SGR report fragments (`35;24;9M…`) in the
/// shell's input buffer, echoed after exit as garbage. The drain eats
/// reports already in flight when the capture-off sequence was written;
/// `poll` with a zero timeout never blocks, so this is safe even when
/// `init` never got as far as raw mode.
pub fn restore() -> io::Result<()> {
    let mut stdout = io::stdout();
    execute!(stdout, DisableMouseCapture, LeaveAlternateScreen)?;
    while crossterm::event::poll(std::time::Duration::ZERO).unwrap_or(false) {
        let _ = crossterm::event::read();
    }
    disable_raw_mode()?;
    Ok(())
}
