//! A tier for tests that records what it was asked and can hold or time out
//! a command, standing in for a slow tool.

use mandible_core::{Authority, CommandNode, Provenance, Source};
use mandible_extract::exec::extract_limit;
use mandible_extract::{ExtractError, ExtractionTier, NodeHints, ResolvedTool, Runner};
use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

/// One probe the spy served: the path and the cap it ran under.
pub type Call = (Vec<String>, Duration);

#[derive(Default)]
struct State {
    calls: Vec<Call>,
    held: HashSet<String>,
    timing_out: HashSet<String>,
    children: HashMap<String, Vec<String>>,
}

/// Records every probe, serves the children it was told of, and holds or
/// times out the commands it was told to.
#[derive(Default)]
pub struct Spy {
    state: Mutex<State>,
    changed: Condvar,
}

impl Spy {
    pub fn new() -> Arc<Spy> {
        Arc::new(Spy::default())
    }

    /// The runner whose only tier is this spy.
    pub fn runner(self: &Arc<Self>) -> Arc<Runner> {
        Arc::new(Runner::new(vec![Box::new(SpyTier(Arc::clone(self)))]))
    }

    /// A resolved tool the spy answers for.
    pub fn tool() -> ResolvedTool {
        ResolvedTool {
            name: "t".to_string(),
            path: Some(std::path::PathBuf::from("/opt/t")),
            version: None,
        }
    }

    /// `word` has these subcommands once it is probed.
    pub fn children(&self, word: &str, names: &[&str]) {
        self.state.lock().unwrap().children.insert(
            word.to_string(),
            names.iter().map(|n| n.to_string()).collect(),
        );
    }

    /// Probing `word` blocks until [`Self::release`].
    pub fn hold(&self, word: &str) {
        self.state.lock().unwrap().held.insert(word.to_string());
    }

    /// Let every held command finish.
    pub fn release(&self) {
        self.state.lock().unwrap().held.clear();
        self.changed.notify_all();
    }

    /// Probing `word` is stopped at the time limit.
    pub fn time_out(&self, word: &str, yes: bool) {
        let mut state = self.state.lock().unwrap();
        if yes {
            state.timing_out.insert(word.to_string());
        } else {
            state.timing_out.remove(word);
        }
    }

    /// Every probe served so far.
    pub fn calls(&self) -> Vec<Call> {
        self.state.lock().unwrap().calls.clone()
    }

    /// How many times `path` was probed.
    pub fn count(&self, path: &[String]) -> usize {
        self.calls().iter().filter(|(p, _)| p == path).count()
    }

    /// Forget the calls so far.
    pub fn forget(&self) {
        self.state.lock().unwrap().calls.clear();
    }

    /// Wait until `path` has been probed `n` times.
    pub fn wait_for(&self, path: &[String], n: usize) -> bool {
        let end = Instant::now() + Duration::from_secs(10);
        while Instant::now() < end {
            if self.count(path) >= n {
                return true;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        false
    }
}

struct SpyTier(Arc<Spy>);

impl ExtractionTier for SpyTier {
    fn name(&self) -> &'static str {
        "test::spy"
    }

    fn authority(&self) -> Authority {
        Source::HelpText.authority()
    }

    fn detect(&self, _tool: &ResolvedTool) -> bool {
        true
    }

    fn extract_node(
        &self,
        _tool: &ResolvedTool,
        path: &[String],
        _hints: NodeHints,
    ) -> Result<CommandNode, ExtractError> {
        let word = path.last().cloned().unwrap_or_default();
        let mut state = self.0.state.lock().unwrap();
        state.calls.push((path.to_vec(), extract_limit()));
        let end = Instant::now() + Duration::from_secs(15);
        while state.held.contains(&word) && Instant::now() < end {
            state = self
                .0
                .changed
                .wait_timeout(state, Duration::from_millis(50))
                .unwrap()
                .0;
        }
        if state.timing_out.contains(&word) {
            return Err(ExtractError::TimedOut);
        }
        let mut node = CommandNode::new(word.clone(), Provenance::single(Source::HelpText));
        node.children_filled = true;
        node.heading_attested = true;
        for name in state.children.get(&word).into_iter().flatten() {
            let mut child = CommandNode::new(name.clone(), Provenance::single(Source::HelpText));
            child.heading_attested = true;
            node.subcommands.push(child);
        }
        Ok(node)
    }
}
