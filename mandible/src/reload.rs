//! Re-extraction (`r`) off the UI thread.
//!
//! The load runs on its own thread and the finished tree is handed back
//! over a channel the event loop polls, so the page on screen stays usable
//! while a slow tool answers.

use crate::pipeline::{self, LoadedTool};
use std::sync::mpsc::{self, Receiver, Sender};

/// At most one background re-extraction at a time.
pub struct Reloader {
    tx: Sender<(String, LoadedTool)>,
    rx: Receiver<(String, LoadedTool)>,
    running: bool,
}

impl Reloader {
    pub fn new() -> Self {
        let (tx, rx) = mpsc::channel();
        Self {
            tx,
            rx,
            running: false,
        }
    }

    /// Start re-extracting `tool`. False when one is already running.
    pub fn start(&mut self, tool: &str) -> bool {
        self.start_with(tool, pipeline::load)
    }

    fn start_with(
        &mut self,
        tool: &str,
        load: impl FnOnce(&str) -> LoadedTool + Send + 'static,
    ) -> bool {
        if self.running {
            return false;
        }
        self.running = true;
        let tx = self.tx.clone();
        let tool = tool.to_string();
        std::thread::spawn(move || {
            let loaded = load(&tool);
            let _ = tx.send((tool, loaded));
        });
        true
    }

    /// The finished tree for `tool`, if one has arrived. A result for any
    /// other tool is dropped.
    pub fn poll(&mut self, tool: &str) -> Option<LoadedTool> {
        let (loaded_tool, loaded) = self.rx.try_recv().ok()?;
        self.running = false;
        (loaded_tool == tool).then_some(loaded)
    }

    #[cfg(test)]
    fn is_running(&self) -> bool {
        self.running
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc::{channel, Sender};
    use std::sync::{Arc, Mutex};
    use std::time::{Duration, Instant};

    fn empty(tool: &str) -> LoadedTool {
        LoadedTool {
            tool: tool.to_string(),
            root: None,
            tier_statuses: Vec::new(),
            elapsed: Duration::ZERO,
        }
    }

    fn gated() -> (Sender<()>, Arc<Mutex<std::sync::mpsc::Receiver<()>>>) {
        let (tx, rx) = channel();
        (tx, Arc::new(Mutex::new(rx)))
    }

    fn wait(r: &mut Reloader, tool: &str) -> Option<LoadedTool> {
        let end = Instant::now() + Duration::from_secs(5);
        while Instant::now() < end {
            if let Some(l) = r.poll(tool) {
                return Some(l);
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        None
    }

    #[test]
    fn starting_a_reload_returns_while_the_tool_is_still_answering() {
        let (open, closed) = gated();
        let mut r = Reloader::new();
        let started = Instant::now();
        assert!(r.start_with("t", move |t| {
            let _ = closed.lock().unwrap().recv_timeout(Duration::from_secs(3));
            empty(t)
        }));
        assert!(started.elapsed() < Duration::from_secs(1), "start blocked");
        assert!(r.poll("t").is_none(), "nothing has arrived yet");
        open.send(()).unwrap();
        assert!(wait(&mut r, "t").is_some());
    }

    #[test]
    fn a_second_reload_is_ignored_while_one_runs() {
        let (open, closed) = gated();
        let mut r = Reloader::new();
        assert!(r.start_with("t", move |t| {
            let _ = closed.lock().unwrap().recv_timeout(Duration::from_secs(3));
            empty(t)
        }));
        let ran = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let flag = Arc::clone(&ran);
        assert!(!r.start_with("t", move |t| {
            flag.store(true, std::sync::atomic::Ordering::SeqCst);
            empty(t)
        }));
        assert!(r.is_running());
        open.send(()).unwrap();
        assert!(wait(&mut r, "t").is_some());
        assert!(!ran.load(std::sync::atomic::Ordering::SeqCst));
        assert!(r.start_with("t", empty), "free again once it arrived");
    }

    #[test]
    fn a_result_for_another_tool_is_dropped() {
        let mut r = Reloader::new();
        assert!(r.start_with("old", empty));
        let end = Instant::now() + Duration::from_secs(5);
        while r.is_running() && Instant::now() < end {
            assert!(r.poll("new").is_none(), "stale result applied");
            std::thread::sleep(Duration::from_millis(5));
        }
        assert!(!r.is_running());
    }
}
