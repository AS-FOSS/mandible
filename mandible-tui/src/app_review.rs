//! `mandible --review`: state and key handling for the in-app audit review
//! overlay that turns the ordinary per-tool tree view into
//! `xtask/src/audit.rs`'s review loop, run inside the real product instead
//! of a text transcript (spec: the review effort is capitalized into
//! `corpus/` fixtures, and reviewing offline shows only root-only trees —
//! `openssl` emits 151 empty subcommand stubs and zero flags — while the
//! in-app path fills subcommands lazily as the reviewer navigates, so it
//! shows a tool's *real* subcommand flags).
//!
//! This module owns only in-memory UI state and pure key-handling logic —
//! no file I/O, matching [`crate::app`]'s own "pure state, no rendering, no
//! terminal I/O" discipline. The surrounding session (loading the manifest,
//! saving a verdict after every entry, advancing to the next pending tool)
//! is owned by `mandible/src/app_runner.rs`'s `run_review`, which is the
//! only place in the `mandible` binary that touches
//! `mandible_core::audit::{load, save}`.

use crate::app::App;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

/// Read-only review context for the tool currently on screen, attached to
/// [`App::review`] for the duration of one tool's session. Mirrors
/// `mandible_core::audit::Entry`'s pre-tag fields, but carries only what
/// the overlay displays — the manifest file itself is never read or
/// written from here.
#[derive(Debug, Clone, PartialEq)]
pub struct ReviewOverlay {
    /// The tool on screen, exactly as `mandible <tool>` would show it.
    pub tool: String,
    /// The stratum this tool was drawn from (`ok`, `suspicious`, ...).
    pub stratum: String,
    /// The K1 pre-tag suggestion, as computed by `xtask audit sample`.
    pub k1: Option<bool>,
    /// The K2 pre-tag suggestion.
    pub k2: Option<bool>,
    /// The K3 pre-tag suggestion.
    pub k3: Option<bool>,
    /// Why this entry was force-included in the sample, if it was.
    pub include_reason: Option<String>,
    /// How many entries — this one included — are still pending.
    pub remaining: usize,
    /// The sample's total entry count.
    pub total: usize,
    /// `Some` once a verdict key (`c`/`i`/`w`/`s`) has been pressed and the
    /// reviewer is typing an optional note before confirming with `Enter`.
    pub draft: Option<ReviewDraft>,
}

/// A verdict chosen but not yet confirmed: the canonical word plus whatever
/// note text the reviewer has typed so far.
#[derive(Debug, Clone, PartialEq)]
pub struct ReviewDraft {
    /// The canonical verdict word (`"correct"`/`"incomplete"`/`"wrong"`/
    /// `"skip"`), fixed the moment the draft starts.
    pub verdict: &'static str,
    /// The note as typed so far, including any `k1=`/`k2=`/`k3=` override
    /// tokens still embedded — [`ReviewSubmission::note`] carries these
    /// through unparsed; pulling them out is
    /// `mandible_core::audit::extract_tag_override`'s job, run once by the
    /// caller so a verdict entered here and one entered via `xtask audit
    /// ingest` parse identically.
    pub note: String,
    /// Set when `Enter` was pressed on a `wrong`/`incomplete` draft whose
    /// note was still blank, so the overlay can say why nothing happened.
    /// Cleared as soon as the reviewer types anything, because by then the
    /// complaint has been answered and a stale warning is worse than none.
    pub note_required: bool,
}

/// What pressing a key did to an active review overlay. `None` from
/// [`handle_review_key`] means the overlay didn't claim the key at all, so
/// the caller should fall through to [`crate::event::handle_key`].
#[derive(Debug, Clone, PartialEq)]
pub enum ReviewKeyOutcome {
    /// The key was handled entirely within the overlay (a note character, a
    /// cancelled draft, a verdict key starting a new one) — nothing else to
    /// do this event.
    Consumed,
    /// The reviewer confirmed a verdict with `Enter`. The caller applies it
    /// to the manifest, saves, and advances to the next pending tool.
    Submit(ReviewSubmission),
}

/// A confirmed verdict, ready for the caller to record.
#[derive(Debug, Clone, PartialEq)]
pub struct ReviewSubmission {
    /// The canonical verdict word.
    pub verdict: &'static str,
    /// The raw note text as typed, tag-override tokens (if any) still
    /// embedded.
    pub note: String,
}

/// Handle one key while `app.review` is attached. Returns `None` when
/// review isn't active, the reviewer is mid-`Ctrl`-chord (so the ordinary
/// `Ctrl-C`-quits-from-anywhere binding still reaches
/// [`crate::event::handle_key`]), or the key isn't one the overlay claims —
/// in every one of those cases the caller should try the ordinary handler
/// next.
pub fn handle_review_key(app: &mut App, key: KeyEvent) -> Option<ReviewKeyOutcome> {
    if key.modifiers.contains(KeyModifiers::CONTROL) {
        return None;
    }
    let focus = app.focus;
    let overlay = app.review.as_mut()?;

    if let Some(draft) = &mut overlay.draft {
        // Drafting a note: every key is the overlay's, so an accidental
        // arrow press or letter never leaks into tree navigation or a
        // half-typed search query while a verdict is in flight.
        return Some(match key.code {
            KeyCode::Enter => {
                // A `wrong`/`incomplete` verdict with a blank note is
                // refused rather than recorded: for those two the note *is*
                // the finding, and an entry naming a tool with nothing about
                // what was wrong with it gives later triage nothing to act
                // on. The draft stays open with everything typed so far, so
                // this costs a keystroke, never work.
                if mandible_core::audit::verdict_requires_note(draft.verdict)
                    && draft.note.trim().is_empty()
                {
                    draft.note_required = true;
                    ReviewKeyOutcome::Consumed
                } else {
                    let submission = ReviewSubmission {
                        verdict: draft.verdict,
                        note: std::mem::take(&mut draft.note),
                    };
                    overlay.draft = None;
                    ReviewKeyOutcome::Submit(submission)
                }
            }
            KeyCode::Esc => {
                overlay.draft = None;
                ReviewKeyOutcome::Consumed
            }
            KeyCode::Backspace => {
                draft.note.pop();
                ReviewKeyOutcome::Consumed
            }
            KeyCode::Char(c) => {
                draft.note.push(c);
                draft.note_required = false;
                ReviewKeyOutcome::Consumed
            }
            _ => ReviewKeyOutcome::Consumed,
        });
    }

    // Not drafting: `/` still opens the search box as normal, and while
    // it's focused, typing `c`/`i`/`w`/`s` types those letters into the
    // query rather than starting a verdict — a reviewer narrowing the tree
    // by name must not have every "c" hijacked into a "correct" draft.
    if focus == crate::app::Focus::Search {
        return None;
    }

    let verdict = match key.code {
        KeyCode::Char('c') => "correct",
        KeyCode::Char('i') => "incomplete",
        KeyCode::Char('w') => "wrong",
        KeyCode::Char('s') => "skip",
        _ => return None,
    };
    overlay.draft = Some(ReviewDraft {
        verdict,
        note: String::new(),
        note_required: false,
    });
    Some(ReviewKeyOutcome::Consumed)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::Focus;
    use mandible_core::{CommandNode, Provenance, Source};

    fn app_with_review() -> App {
        let root = CommandNode::new("git", Provenance::single(Source::HelpText));
        let mut app = App::new("git".to_string(), root);
        app.review = Some(ReviewOverlay {
            tool: "git".to_string(),
            stratum: "ok".to_string(),
            k1: None,
            k2: None,
            k3: Some(true),
            include_reason: None,
            remaining: 3,
            total: 12,
            draft: None,
        });
        app
    }

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    #[test]
    fn no_review_overlay_means_the_key_is_never_claimed() {
        let mut app = App::new(
            "git".to_string(),
            CommandNode::new("git", Provenance::single(Source::HelpText)),
        );
        assert_eq!(handle_review_key(&mut app, key(KeyCode::Char('c'))), None);
    }

    /// Enter on a blank-note `wrong` draft must refuse: no submission, the
    /// draft stays open with whatever was typed, and the overlay is told to
    /// explain itself.
    #[test]
    fn enter_refuses_a_wrong_verdict_with_no_note() {
        let mut app = app_with_review();
        handle_review_key(&mut app, key(KeyCode::Char('w')));
        let outcome = handle_review_key(&mut app, key(KeyCode::Enter));
        assert_eq!(outcome, Some(ReviewKeyOutcome::Consumed));
        let draft = app.review.as_ref().unwrap().draft.as_ref().unwrap();
        assert_eq!(draft.verdict, "wrong");
        assert!(draft.note_required, "the overlay must be told to explain");
    }

    /// Typing answers the complaint, so the warning must clear rather than
    /// linger while the reviewer is visibly doing what it asked.
    #[test]
    fn typing_clears_the_note_required_warning_and_then_enter_submits() {
        let mut app = app_with_review();
        handle_review_key(&mut app, key(KeyCode::Char('w')));
        handle_review_key(&mut app, key(KeyCode::Enter));
        handle_review_key(&mut app, key(KeyCode::Char('x')));
        assert!(
            !app.review
                .as_ref()
                .unwrap()
                .draft
                .as_ref()
                .unwrap()
                .note_required
        );
        let outcome = handle_review_key(&mut app, key(KeyCode::Enter));
        match outcome {
            Some(ReviewKeyOutcome::Submit(s)) => {
                assert_eq!(s.verdict, "wrong");
                assert_eq!(s.note, "x");
            }
            other => panic!("expected a submission, got {other:?}"),
        }
    }

    /// `correct` carries no obligation, so Enter submits immediately.
    #[test]
    fn enter_submits_a_correct_verdict_with_no_note() {
        let mut app = app_with_review();
        handle_review_key(&mut app, key(KeyCode::Char('c')));
        match handle_review_key(&mut app, key(KeyCode::Enter)) {
            Some(ReviewKeyOutcome::Submit(s)) => assert_eq!(s.verdict, "correct"),
            other => panic!("expected a submission, got {other:?}"),
        }
    }

    #[test]
    fn pressing_a_verdict_letter_starts_a_draft() {
        let mut app = app_with_review();
        let outcome = handle_review_key(&mut app, key(KeyCode::Char('c')));
        assert_eq!(outcome, Some(ReviewKeyOutcome::Consumed));
        let draft = app.review.as_ref().unwrap().draft.as_ref().unwrap();
        assert_eq!(draft.verdict, "correct");
        assert_eq!(draft.note, "");
    }

    #[test]
    fn every_verdict_letter_maps_to_its_canonical_word() {
        for (letter, word) in [
            ('c', "correct"),
            ('i', "incomplete"),
            ('w', "wrong"),
            ('s', "skip"),
        ] {
            let mut app = app_with_review();
            handle_review_key(&mut app, key(KeyCode::Char(letter)));
            assert_eq!(
                app.review.as_ref().unwrap().draft.as_ref().unwrap().verdict,
                word
            );
        }
    }

    #[test]
    fn an_unrelated_letter_is_not_claimed_and_falls_through() {
        let mut app = app_with_review();
        assert_eq!(handle_review_key(&mut app, key(KeyCode::Char('x'))), None);
        assert!(app.review.as_ref().unwrap().draft.is_none());
    }

    #[test]
    fn typing_while_drafting_builds_the_note() {
        let mut app = app_with_review();
        handle_review_key(&mut app, key(KeyCode::Char('i')));
        for c in "flags missing".chars() {
            let outcome = handle_review_key(&mut app, key(KeyCode::Char(c)));
            assert_eq!(outcome, Some(ReviewKeyOutcome::Consumed));
        }
        assert_eq!(
            app.review.as_ref().unwrap().draft.as_ref().unwrap().note,
            "flags missing"
        );
    }

    #[test]
    fn backspace_edits_the_note() {
        let mut app = app_with_review();
        handle_review_key(&mut app, key(KeyCode::Char('w')));
        handle_review_key(&mut app, key(KeyCode::Char('a')));
        handle_review_key(&mut app, key(KeyCode::Char('b')));
        handle_review_key(&mut app, key(KeyCode::Backspace));
        assert_eq!(
            app.review.as_ref().unwrap().draft.as_ref().unwrap().note,
            "a"
        );
    }

    #[test]
    fn escape_cancels_the_draft_without_submitting() {
        let mut app = app_with_review();
        handle_review_key(&mut app, key(KeyCode::Char('w')));
        handle_review_key(&mut app, key(KeyCode::Char('x')));
        let outcome = handle_review_key(&mut app, key(KeyCode::Esc));
        assert_eq!(outcome, Some(ReviewKeyOutcome::Consumed));
        assert!(app.review.as_ref().unwrap().draft.is_none());
    }

    #[test]
    fn enter_submits_the_verdict_and_note_and_clears_the_draft() {
        let mut app = app_with_review();
        handle_review_key(&mut app, key(KeyCode::Char('i')));
        for c in "k1=false looked fine".chars() {
            handle_review_key(&mut app, key(KeyCode::Char(c)));
        }
        let outcome = handle_review_key(&mut app, key(KeyCode::Enter));
        assert_eq!(
            outcome,
            Some(ReviewKeyOutcome::Submit(ReviewSubmission {
                verdict: "incomplete",
                note: "k1=false looked fine".to_string(),
            }))
        );
        assert!(app.review.as_ref().unwrap().draft.is_none());
    }

    #[test]
    fn arrow_keys_are_swallowed_while_drafting() {
        let mut app = app_with_review();
        handle_review_key(&mut app, key(KeyCode::Char('c')));
        let outcome = handle_review_key(&mut app, key(KeyCode::Down));
        assert_eq!(outcome, Some(ReviewKeyOutcome::Consumed));
        assert_eq!(
            app.review.as_ref().unwrap().draft.as_ref().unwrap().note,
            "",
            "an arrow key must not be typed into the note or leak to navigation"
        );
    }

    #[test]
    fn ctrl_c_is_never_claimed_so_quit_from_anywhere_still_works() {
        let mut app = app_with_review();
        let ev = KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL);
        assert_eq!(handle_review_key(&mut app, ev), None);
    }

    #[test]
    fn ctrl_c_is_never_claimed_even_mid_draft() {
        let mut app = app_with_review();
        handle_review_key(&mut app, key(KeyCode::Char('w')));
        let ev = KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL);
        assert_eq!(
            handle_review_key(&mut app, ev),
            None,
            "even while drafting, Ctrl-C must reach the ordinary quit handler"
        );
    }

    #[test]
    fn verdict_letters_type_into_the_search_box_instead_of_starting_a_draft() {
        let mut app = app_with_review();
        app.focus = Focus::Search;
        assert_eq!(handle_review_key(&mut app, key(KeyCode::Char('c'))), None);
        assert!(app.review.as_ref().unwrap().draft.is_none());
    }
}
