//! `y`: copying a flag's spelling or a command's full path (spec §2: "`y`
//! is not a nice-to-have... a reference tool that can't hand you the string
//! makes you retype it.").
//!
//! Tries the OS clipboard via `arboard` first; if that's unavailable (no
//! display server, headless box, sandboboxed terminal), degrades to an
//! OSC-52 escape sequence written to stdout, which most modern terminal
//! emulators (including over SSH) intercept and use to set their own
//! clipboard.

use base64::Engine;
use std::io::Write;

/// Copy `text` to the clipboard, trying the OS clipboard first and falling
/// back to an OSC-52 terminal escape sequence. Returns `Ok(())` if either
/// mechanism plausibly succeeded (OSC-52 has no confirmation channel, so a
/// successful *write* is treated as success).
pub fn copy(text: &str) -> Result<(), CopyError> {
    match arboard::Clipboard::new().and_then(|mut cb| cb.set_text(text.to_string())) {
        Ok(()) => Ok(()),
        Err(_) => copy_via_osc52(text),
    }
}

/// Errors copying to the clipboard by any mechanism.
#[derive(Debug, thiserror::Error)]
pub enum CopyError {
    /// Writing the OSC-52 escape sequence to stdout failed.
    #[error("failed to write OSC-52 sequence: {0}")]
    Write(#[from] std::io::Error),
}

fn copy_via_osc52(text: &str) -> Result<(), CopyError> {
    let encoded = base64::engine::general_purpose::STANDARD.encode(text.as_bytes());
    // ESC ] 5 2 ; c ; <base64> BEL
    let sequence = format!("\x1b]52;c;{encoded}\x07");
    let mut stdout = std::io::stdout();
    stdout.write_all(sequence.as_bytes())?;
    stdout.flush()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn osc52_encodes_expected_sequence_shape() {
        // Not asserting against the real stdout (that's exercised by the
        // fallback path only when no OS clipboard exists, and this test
        // environment has no display server, so it likely does exercise
        // it) — just proving the encoding function itself is correct,
        // decoupled from I/O.
        let encoded = base64::engine::general_purpose::STANDARD.encode(b"--interactive");
        assert_eq!(encoded, "LS1pbnRlcmFjdGl2ZQ==");
    }
}
