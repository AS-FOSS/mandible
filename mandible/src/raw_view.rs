//! The raw output the verbatim view (`t`) shows (spec §11).
//!
//! A fill keeps the output of the probes it ran, in the command's probe
//! record. The view renders from that output and never runs a probe on the
//! UI thread. A node with nothing kept is read again on the background pool,
//! as a forced request through the queue (`crate::probing::at_rest`).

use mandible_core::CommandNode;
use mandible_extract::help_text::{raw_help_kept, render_argv};
use mandible_extract::{NodeHints, ResolvedTool};
use mandible_tui::app::{App, RawHelp};
use mandible_tui::probes::Probes;

/// What the verbatim view shows for `path`, from the kept output alone.
/// `None` when nothing is kept, or a probe the view needs is missing.
fn view(
    root: &CommandNode,
    probes: &Probes,
    resolved: &ResolvedTool,
    path: &[String],
) -> Option<RawHelp> {
    let node = mandible_core::resolve(root, path);
    let hints = NodeHints {
        heading_attested: node.is_some_and(|n| n.heading_attested),
        abbrev_probe_attested: node.is_some_and(|n| n.abbrev_probe_attested),
        help_word_attested: node.is_some_and(|n| n.help_word_attested),
    };
    // The node's own log first, then its ancestors', which the
    // not-attested fallback reads for the root document.
    let sources = probes.kept_chain(path);
    let (raw_tool, raw_path) = crate::discovery::probe_target(root, resolved, path);
    let shown = raw_help_kept(&sources, &raw_tool, &raw_path, hints)?;
    Some(match shown {
        Ok((lines, flag)) => {
            let words: Vec<String> = raw_path.iter().skip(1).cloned().collect();
            RawHelp::Ready(lines, render_argv(&raw_tool.name, &words, &flag))
        }
        Err(e) => RawHelp::Failed(format!("could not fetch raw --help: {e}")),
    })
}

/// Show `path` from the kept output. False when it cannot be.
pub fn serve(app: &mut App, resolved: &ResolvedTool, path: &[String]) -> bool {
    if !app.probes.touch(path) {
        return false;
    }
    match view(&app.root, &app.probes, resolved, path) {
        Some(shown) => {
            app.set_raw_help(path.to_vec(), shown);
            true
        }
        None => false,
    }
}

/// After a fill kept its output for `path`: forget the views whose output
/// was dropped, and show the view if it is waiting on this output or was
/// shown from the output this one replaced.
/// `forced` says the fill was run for the view alone, so a view that still
/// cannot be built from it is reported.
pub fn after_fill(
    app: &mut App,
    resolved: &ResolvedTool,
    path: &[String],
    evicted: Vec<Vec<String>>,
    forced: bool,
) {
    for gone in evicted {
        if app.raw_mode && app.selected_path().as_deref() == Some(&gone[..]) {
            app.mark_raw_pending(gone);
        } else {
            app.forget_raw_help(&gone);
        }
    }
    // A view shown from the output a re-read replaced is out of date.
    if app.has_raw_help(path) && !app.raw_is_pending(path) {
        app.forget_raw_help(path);
        app.mark_raw_pending(path.to_vec());
    }
    if app.raw_is_pending(path) && !serve(app, resolved, path) && forced {
        app.set_raw_help(
            path.to_vec(),
            RawHelp::Failed("could not read the tool's help output".to_string()),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mandible_core::{Provenance, Source};
    use mandible_extract::exec::{ExecError, ExecOutput, InertArgv, Probe};
    use mandible_extract::help_text::probe_raw_with;
    use mandible_extract::KeptProbes;
    use mandible_tui::probes::{Outcome, Probes, KEPT_BUDGET_BYTES};
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::Duration;

    struct Counting(AtomicUsize);

    impl Probe for Counting {
        fn run(
            &self,
            _tool: &Path,
            argv: &InertArgv,
            _timeout: Duration,
        ) -> Result<ExecOutput, ExecError> {
            self.0.fetch_add(1, Ordering::SeqCst);
            Ok(ExecOutput {
                stdout: format!("Usage: t {}\n", argv.args().join(" ")).into_bytes(),
                stderr: Vec::new(),
                exit_code: Some(0),
                timed_out: false,
            })
        }
    }

    fn tool(path: &str) -> ResolvedTool {
        ResolvedTool {
            name: "t".to_string(),
            path: Some(PathBuf::from(path)),
            version: None,
        }
    }

    fn tree(children: usize) -> CommandNode {
        let mut root = CommandNode::new("t", Provenance::single(Source::HelpText));
        root.children_filled = true;
        for i in 0..children {
            let mut child =
                CommandNode::new(format!("c{i:02}"), Provenance::single(Source::HelpText));
            child.heading_attested = true;
            child.children_filled = true;
            root.subcommands.push(child);
        }
        root
    }

    fn path(name: &str) -> Vec<String> {
        vec!["t".to_string(), name.to_string()]
    }

    fn kept_for(probe: &Counting, name: &str) -> KeptProbes {
        let hints = NodeHints {
            heading_attested: true,
            abbrev_probe_attested: false,
            help_word_attested: false,
        };
        probe_raw_with(probe, &tool("/opt/t"), &path(name), hints)
    }

    #[test]
    fn holding_a_movement_key_with_the_view_on_never_probes() {
        let probe = Counting(AtomicUsize::new(0));
        let mut app = App::new("t".to_string(), tree(41));
        for i in 0..41 {
            let name = format!("c{i:02}");
            app.probes
                .finish(path(&name), Outcome::Parsed, kept_for(&probe, &name));
        }
        probe.0.store(0, Ordering::SeqCst);
        app.toggle_raw_mode();

        for _ in 0..40 {
            app.move_down();
            app.ensure_rows_fresh();
            let wanted = app.raw_fetch_needed();
            assert!(wanted.is_some(), "a new row asks for its text");
            let at = app.selected_path().unwrap();
            assert!(serve(&mut app, &tool("/opt/t"), &at));
            assert!(matches!(
                app.raw_help_for_selected(),
                Some(RawHelp::Ready(..))
            ));
        }
        assert_eq!(probe.0.load(Ordering::SeqCst), 0, "a row ran a probe");
    }

    #[test]
    fn past_the_budget_the_least_recently_viewed_entry_goes_first() {
        let probe = Counting(AtomicUsize::new(0));
        let root = tree(4);
        let one = kept_for(&probe, "c00").byte_size();
        let mut probes = Probes::new(one * 3);
        let tool = tool("/opt/t");
        for name in ["c00", "c01", "c02"] {
            let dropped = probes.finish(path(name), Outcome::Parsed, kept_for(&probe, name));
            assert!(dropped.is_empty());
        }
        assert!(probes.touch(&path("c00")));

        let dropped = probes.finish(path("c03"), Outcome::Parsed, kept_for(&probe, "c03"));

        assert_eq!(dropped, vec![path("c01")]);
        assert!(probes.kept_bytes() <= one * 3);
        assert!(view(&root, &probes, &tool, &path("c01")).is_none());
        assert!(view(&root, &probes, &tool, &path("c00")).is_some());
        assert!(view(&root, &probes, &tool, &path("c03")).is_some());
        assert!(
            probes.finished(&path("c01")),
            "the record outlives its output"
        );
    }

    #[test]
    fn a_reset_forgets_everything_kept() {
        let probe = Counting(AtomicUsize::new(0));
        let mut probes = Probes::new(KEPT_BUDGET_BYTES);
        probes.finish(path("c00"), Outcome::Parsed, kept_for(&probe, "c00"));
        probes.reset();
        assert_eq!(probes.kept_bytes(), 0);
        assert!(view(&tree(1), &probes, &tool("/opt/t"), &path("c00")).is_none());
    }

    #[test]
    fn a_view_shown_from_replaced_output_is_shown_again_from_the_new_output() {
        let probe = Counting(AtomicUsize::new(0));
        let mut app = App::new("t".to_string(), tree(2));
        let resolved = tool("/opt/t");
        app.probes
            .finish(path("c00"), Outcome::TimedOut, kept_for(&probe, "c00"));
        app.set_raw_help(path("c00"), RawHelp::Failed("old".to_string()));
        app.toggle_raw_mode();

        app.probes
            .finish(path("c00"), Outcome::Parsed, kept_for(&probe, "c00"));
        after_fill(&mut app, &resolved, &path("c00"), Vec::new(), true);

        app.move_down();
        assert_eq!(app.selected_path(), Some(path("c00")));
        assert!(matches!(
            app.raw_help_for_selected(),
            Some(RawHelp::Ready(..))
        ));
    }
}
