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
use std::time::{Duration, Instant};

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
/// shell's input buffer, echoed after exit as garbage.
///
/// The drain eats those reports, and it waits for them rather than
/// sampling once. The emulator processes the capture-off sequence
/// asynchronously, so a report can still be on its way *after* the
/// sequence was written: a zero-timeout drain finds an empty queue,
/// returns, and the report arrives a millisecond later with nobody left
/// to read it — the same litter, now from a narrower race. So each poll
/// is given [`DRAIN_POLL`] to produce something, and the drain ends at
/// the first poll that comes back empty. [`DRAIN_BUDGET`] caps the whole
/// thing, because a terminal that never stops talking must not be able to
/// stop mandible from exiting.
///
/// Safe to call when [`init`] never got as far as raw mode: `poll` on a
/// stream that is not a terminal errors rather than blocking, and an
/// error ends the drain.
pub fn restore(sink: Sink) -> io::Result<()> {
    let mut writer = sink.writer();
    execute!(writer, DisableMouseCapture, LeaveAlternateScreen)?;
    drain_pending_input();
    disable_raw_mode()?;
    Ok(())
}

/// How long one drain poll waits for a report still in transit. Long
/// enough to cover an emulator's turnaround, short enough to be invisible
/// on the way out.
const DRAIN_POLL: Duration = Duration::from_millis(25);

/// The drain's total budget. Quitting is not allowed to become a wait,
/// however much the terminal has to say.
const DRAIN_BUDGET: Duration = Duration::from_millis(250);

/// Read and discard input until a poll comes back empty or the budget runs
/// out. See [`restore`] for why the wait exists.
fn drain_pending_input() {
    let started = Instant::now();
    while let Some(timeout) = drain_poll_timeout(started.elapsed()) {
        match crossterm::event::poll(timeout) {
            Ok(true) => {
                let _ = crossterm::event::read();
            }
            // Empty (the drain is done) or no terminal to poll at all.
            _ => return,
        }
    }
}

/// How long the next drain poll may block: at most [`DRAIN_POLL`], and
/// never past what is left of [`DRAIN_BUDGET`]. `None` once the budget is
/// spent, which is what bounds [`drain_pending_input`]'s loop.
fn drain_poll_timeout(elapsed: Duration) -> Option<Duration> {
    let left = DRAIN_BUDGET.checked_sub(elapsed)?;
    if left.is_zero() {
        return None;
    }
    Some(left.min(DRAIN_POLL))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The drain's bound, which is the half no terminal test can check:
    /// every poll is short, and the sum of them cannot exceed the budget,
    /// so `restore` returns even against a terminal that never goes quiet.
    #[test]
    fn the_drain_is_bounded_by_its_budget() {
        assert_eq!(drain_poll_timeout(Duration::ZERO), Some(DRAIN_POLL));
        assert_eq!(
            drain_poll_timeout(DRAIN_BUDGET - DRAIN_POLL * 2),
            Some(DRAIN_POLL),
            "a poll well inside the budget gets a full slice"
        );
        assert_eq!(
            drain_poll_timeout(DRAIN_BUDGET - Duration::from_millis(10)),
            Some(Duration::from_millis(10)),
            "the last poll is trimmed to what is left, never rounded up"
        );
        assert_eq!(
            drain_poll_timeout(DRAIN_BUDGET),
            None,
            "a spent budget ends the loop"
        );
        assert_eq!(
            drain_poll_timeout(DRAIN_BUDGET * 4),
            None,
            "and an overrun does not wrap around into another slice"
        );
    }
}
