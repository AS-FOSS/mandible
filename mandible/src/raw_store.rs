//! The raw output the verbatim view (`t`) shows (spec §11).
//!
//! A fill keeps the output of the probes it ran. The view renders from that
//! output and never runs a probe on the UI thread. A node with nothing kept
//! is fetched on the background pool, through the front lane that serves the
//! row under the cursor.

use crate::background::Warmer;
use mandible_core::CommandNode;
use mandible_extract::help_text::{raw_help_kept, render_argv};
use mandible_extract::{KeptProbes, NodeHints, ResolvedTool, Runner};
use mandible_tui::app::{App, RawHelp};
use std::collections::HashMap;
use std::sync::Arc;

/// Total raw output kept for one session. Past it, the entries least
/// recently filled or viewed are dropped.
pub const RAW_BUDGET_BYTES: usize = 64 * 1024 * 1024;

struct Slot {
    kept: KeptProbes,
    bytes: usize,
    used: u64,
}

/// Kept probe output by node path, within a byte budget.
pub struct RawStore {
    slots: HashMap<Vec<String>, Slot>,
    total: usize,
    clock: u64,
    budget: usize,
}

impl RawStore {
    pub fn new(budget: usize) -> Self {
        Self {
            slots: HashMap::new(),
            total: 0,
            clock: 0,
            budget,
        }
    }

    /// Keep `kept` for `path`, replacing what was there. Returns the paths
    /// dropped to stay within the budget; `path` itself is never one.
    pub fn insert(&mut self, path: Vec<String>, kept: KeptProbes) -> Vec<Vec<String>> {
        self.clock += 1;
        let bytes = kept.byte_size();
        let slot = Slot {
            kept,
            bytes,
            used: self.clock,
        };
        if let Some(old) = self.slots.insert(path.clone(), slot) {
            self.total -= old.bytes;
        }
        self.total += bytes;
        let mut dropped = Vec::new();
        while self.total > self.budget {
            let oldest = self
                .slots
                .iter()
                .filter(|(p, _)| **p != path)
                .min_by_key(|(_, s)| s.used)
                .map(|(p, _)| p.clone());
            let Some(oldest) = oldest else { break };
            if let Some(gone) = self.slots.remove(&oldest) {
                self.total -= gone.bytes;
            }
            dropped.push(oldest);
        }
        dropped
    }

    /// Forget everything, as a re-extract does.
    pub fn clear(&mut self) {
        self.slots.clear();
        self.total = 0;
    }

    #[cfg(test)]
    pub fn total_bytes(&self) -> usize {
        self.total
    }

    /// What the verbatim view shows for `path`, from the kept output alone.
    /// `None` when nothing is kept, or a probe the view needs is missing.
    pub fn view(
        &mut self,
        root: &CommandNode,
        resolved: &ResolvedTool,
        path: &[String],
    ) -> Option<RawHelp> {
        self.clock += 1;
        self.slots.get_mut(path)?.used = self.clock;
        let node = mandible_core::resolve(root, path);
        let hints = NodeHints {
            heading_attested: node.is_some_and(|n| n.heading_attested),
            abbrev_probe_attested: node.is_some_and(|n| n.abbrev_probe_attested),
            help_word_attested: node.is_some_and(|n| n.help_word_attested),
        };
        // The node's own log first, then its ancestors', which the
        // not-attested fallback reads for the root document.
        let sources: Vec<&KeptProbes> = (1..=path.len())
            .rev()
            .filter_map(|len| self.slots.get(&path[..len]))
            .map(|s| &s.kept)
            .collect();
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
}

/// Show `path` from the kept output. False when it cannot be.
pub fn serve(
    app: &mut App,
    store: &mut RawStore,
    resolved: &ResolvedTool,
    path: &[String],
) -> bool {
    match store.view(&app.root, resolved, path) {
        Some(shown) => {
            app.set_raw_help(path.to_vec(), shown);
            true
        }
        None => false,
    }
}

/// Keep what a fill or a raw fetch printed for `path`, and show it if the
/// view is waiting on it. `fetched` says the probes were run for the view
/// alone, so a view that still cannot be built from them is reported.
pub fn keep(
    app: &mut App,
    store: &mut RawStore,
    resolved: &ResolvedTool,
    path: Vec<String>,
    kept: KeptProbes,
    fetched: bool,
) {
    for gone in store.insert(path.clone(), kept) {
        if app.raw_mode && app.selected_path().as_deref() == Some(&gone[..]) {
            app.mark_raw_pending(gone);
        } else {
            app.forget_raw_help(&gone);
        }
    }
    if app.raw_is_pending(&path) && !serve(app, store, resolved, &path) && fetched {
        app.set_raw_help(
            path,
            RawHelp::Failed("could not read the tool's help output".to_string()),
        );
    }
}

/// Ask for the output of the row under the cursor when the view is waiting
/// on it, nothing is kept and no fill will supply it. Called when the
/// cursor has rested.
pub fn fetch_at_rest(
    app: &mut App,
    runner: &Arc<Runner>,
    resolved: &ResolvedTool,
    warmer: &Warmer,
) {
    let Some(path) = app.selected_path() else {
        return;
    };
    if !app.raw_mode || !app.raw_is_pending(&path) || !app.is_loaded(&path) || app.is_pending(&path)
    {
        return;
    }
    let Some(existing) = mandible_core::resolve(&app.root, &path).cloned() else {
        return;
    };
    let (tool, probe_path) = crate::discovery::probe_target(&app.root, resolved, &path);
    let outcome = warmer.prioritize_raw(Arc::clone(runner), tool, probe_path, path, existing);
    if let Some(dropped) = outcome.dropped {
        app.clear_pending(&dropped);
        app.clear_raw_pending(&dropped);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mandible_core::{Provenance, Source};
    use mandible_extract::exec::{ExecError, ExecOutput, InertArgv, Probe};
    use mandible_extract::help_text::probe_raw_with;
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
        let mut store = RawStore::new(RAW_BUDGET_BYTES);
        let mut app = App::new("t".to_string(), tree(41));
        for i in 0..41 {
            let name = format!("c{i:02}");
            store.insert(path(&name), kept_for(&probe, &name));
        }
        probe.0.store(0, Ordering::SeqCst);
        app.toggle_raw_mode();

        for _ in 0..40 {
            app.move_down();
            app.ensure_rows_fresh();
            let wanted = app.raw_fetch_needed();
            assert!(wanted.is_some(), "a new row asks for its text");
            let at = app.selected_path().unwrap();
            assert!(serve(&mut app, &mut store, &tool("/opt/t"), &at));
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
        let mut store = RawStore::new(one * 3);
        let tool = tool("/opt/t");
        for name in ["c00", "c01", "c02"] {
            assert!(store.insert(path(name), kept_for(&probe, name)).is_empty());
        }
        assert!(store.view(&root, &tool, &path("c00")).is_some());

        let dropped = store.insert(path("c03"), kept_for(&probe, "c03"));

        assert_eq!(dropped, vec![path("c01")]);
        assert!(store.total_bytes() <= one * 3);
        assert!(store.view(&root, &tool, &path("c01")).is_none());
        assert!(store.view(&root, &tool, &path("c00")).is_some());
        assert!(store.view(&root, &tool, &path("c03")).is_some());
    }

    #[test]
    fn a_re_extract_forgets_everything_kept() {
        let probe = Counting(AtomicUsize::new(0));
        let mut store = RawStore::new(RAW_BUDGET_BYTES);
        store.insert(path("c00"), kept_for(&probe, "c00"));
        store.clear();
        assert_eq!(store.total_bytes(), 0);
        assert!(store
            .view(&tree(1), &tool("/opt/t"), &path("c00"))
            .is_none());
    }

    #[test]
    fn an_evicted_node_is_fetched_in_the_background_and_shown() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let count = dir.path().join("count");
        let shim = dir.path().join("t");
        std::fs::write(
            &shim,
            format!(
                "#!/bin/sh\necho x >> {}\necho 'Usage: t c00 [options]'\n",
                count.display()
            ),
        )
        .unwrap();
        std::fs::set_permissions(&shim, std::fs::Permissions::from_mode(0o755)).unwrap();
        let resolved = tool(shim.to_str().unwrap());

        let mut app = App::new("t".to_string(), tree(2));
        let mut store = RawStore::new(RAW_BUDGET_BYTES);
        let runner = Arc::new(Runner::new(Vec::new()));
        let warmer = Warmer::new();
        app.toggle_raw_mode();
        app.move_down();
        app.ensure_rows_fresh();
        assert_eq!(app.selected_path(), Some(path("c00")));
        app.mark_raw_pending(path("c00"));

        fetch_at_rest(&mut app, &runner, &resolved, &warmer);

        let mut fetched = Vec::new();
        for _ in 0..200 {
            fetched = warmer.drain_raw();
            if !fetched.is_empty() {
                break;
            }
            std::thread::sleep(Duration::from_millis(25));
        }
        let fetched = fetched.pop().expect("the background fetch never finished");
        keep(
            &mut app,
            &mut store,
            &resolved,
            fetched.path,
            fetched.kept,
            true,
        );

        match app.raw_help_for_selected() {
            Some(RawHelp::Ready(lines, argv)) => {
                assert_eq!(lines[0].as_str(), "Usage: t c00 [options]");
                assert_eq!(argv, "t c00 --help");
            }
            other => panic!("expected the fetched text, got {other:?}"),
        }
        let runs = std::fs::read_to_string(&count).unwrap().lines().count();
        assert_eq!(runs, 1, "the fetch probed more than once");
        assert!(serve(&mut app, &mut store, &resolved, &path("c00")));
        assert_eq!(
            std::fs::read_to_string(&count).unwrap().lines().count(),
            1,
            "showing it again probed"
        );
    }
}
