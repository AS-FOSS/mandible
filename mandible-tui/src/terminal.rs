//! Terminal setup/teardown. Kept separate from [`crate::render`] so
//! rendering can always be exercised in tests via
//! `ratatui::backend::TestBackend`, without a real tty.
//!
//! **Everything this crate writes to the terminal goes through a [`Sink`]**
//! — the drawing, the alternate-screen and mouse-capture sequences, and the
//! OSC-52 clipboard fallback — and the same [`Sink`] answers whether there
//! is a terminal at the other end at all ([`Sink::is_tty`], which decides
//! color). That is one choice, made once at startup, rather than three
//! modules each reaching for `io::stdout()` on their own.
//!
//! It exists because `mandible --print-selection` (spec §2) draws its UI
//! while stdout carries exactly one line — the composed command — for the
//! calling shell to read. Under that mode stdout is a pipe: an escape
//! sequence written there is corruption of the mode's only output, and a
//! `stdout().is_terminal()` color check answers about the pipe rather than
//! about the screen the user is looking at.
//! `mandible-tui/tests/drawing_goes_through_the_sink.rs` is what keeps that
//! true as the crate grows.

use crossterm::event::{DisableMouseCapture, EnableMouseCapture};
use crossterm::execute;
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use ratatui::backend::CrosstermBackend;
use ratatui::Terminal;
use std::io::{self, IsTerminal, Stderr, Stdout};

/// A live terminal handle, backed by `crossterm`.
pub type Term = Terminal<CrosstermBackend<SinkWriter>>;

/// Which stream the TUI draws on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Sink {
    /// stdout: the ordinary `mandible <tool>` session, where stdout *is*
    /// the screen.
    Stdout,
    /// stderr: `mandible --print-selection`, where stdout is reserved for
    /// the one line the calling shell reads back.
    Stderr,
}

impl Sink {
    /// True if this sink is an interactive terminal. mandible must check
    /// this and fail with a clear message rather than let
    /// `enable_raw_mode` produce an opaque OS error ("No such device or
    /// address") when the stream is redirected or there is no controlling
    /// tty.
    pub fn is_tty(self) -> bool {
        match self {
            Sink::Stdout => io::stdout().is_terminal(),
            Sink::Stderr => io::stderr().is_terminal(),
        }
    }

    /// The name to put in an error message about this stream.
    pub fn name(self) -> &'static str {
        match self {
            Sink::Stdout => "stdout",
            Sink::Stderr => "stderr",
        }
    }

    /// A fresh handle to this sink's stream. Handles are cheap and
    /// independently buffered locks, so callers take one per write rather
    /// than sharing.
    pub fn writer(self) -> SinkWriter {
        match self {
            Sink::Stdout => SinkWriter::Stdout(io::stdout()),
            Sink::Stderr => SinkWriter::Stderr(io::stderr()),
        }
    }
}

/// A writable handle to a [`Sink`]. An enum rather than a `Box<dyn Write>`
/// so the backend type stays one concrete type and nothing has to be
/// allocated to draw a frame.
#[derive(Debug)]
pub enum SinkWriter {
    /// A handle to stdout.
    Stdout(Stdout),
    /// A handle to stderr.
    Stderr(Stderr),
}

impl io::Write for SinkWriter {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        match self {
            SinkWriter::Stdout(w) => w.write(buf),
            SinkWriter::Stderr(w) => w.write(buf),
        }
    }

    fn flush(&mut self) -> io::Result<()> {
        match self {
            SinkWriter::Stdout(w) => w.flush(),
            SinkWriter::Stderr(w) => w.flush(),
        }
    }
}

/// Enter raw mode, the alternate screen, and mouse capture on `sink`.
/// Callers must pair this with [`restore`] on the *same* sink (ideally via
/// a panic hook / drop guard in the binary) so a crash doesn't leave the
/// user's terminal in raw mode.
pub fn init(sink: Sink) -> io::Result<Term> {
    enable_raw_mode()?;
    let mut writer = sink.writer();
    execute!(writer, EnterAlternateScreen, EnableMouseCapture)?;
    let backend = CrosstermBackend::new(writer);
    Terminal::new(backend)
}

/// Leave raw mode, the alternate screen, and mouse capture. Safe to call
/// even if [`init`] partially failed, and must be given the same `sink`
/// [`init`] was — the sequences that undo the alternate screen have to
/// reach the stream that entered it.
///
/// Order is load-bearing and the reverse of [`init`]: mouse capture is
/// switched off and pending input drained *while the terminal is still
/// raw*. The original order disabled raw mode first, leaving a window
/// where the terminal was cooked but still reporting mouse motion — any
/// movement during quit landed SGR report fragments (`35;24;9M…`) in the
/// shell's input buffer, echoed after exit as garbage. The drain eats
/// reports already in flight when the capture-off sequence was written;
/// `poll` with a zero timeout never blocks, so this is safe even when
/// [`init`] never got as far as raw mode.
pub fn restore(sink: Sink) -> io::Result<()> {
    let mut writer = sink.writer();
    execute!(writer, DisableMouseCapture, LeaveAlternateScreen)?;
    while crossterm::event::poll(std::time::Duration::ZERO).unwrap_or(false) {
        let _ = crossterm::event::read();
    }
    disable_raw_mode()?;
    Ok(())
}
