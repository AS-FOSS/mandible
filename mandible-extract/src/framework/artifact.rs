//! Step 1 of framework identification (spec §7 Tier A′): scan the artifact
//! itself — a compiled binary's embedded strings, or a script's shebang
//! plus import lines. Ground truth when it matches: `docker` contains the
//! literal bytes `spf13/cobra` 583 times and `gh` 283 times [M-13],
//! independent of how that particular cobra version happens to render its
//! `--help` section headings.
//!
//! Bounded on purpose (spec §7 Tier A′ step 1: "do not slurp a 100 MB
//! binary"): reads at most [`MAX_BINARY_SCAN_BYTES`] of a binary artifact,
//! or [`MAX_SCRIPT_SCAN_BYTES`] of a script (shebang and imports are, by
//! convention, at the top of the file). Marker matching is a plain byte
//! substring search — no `strings`(1)-style printable-run extraction —
//! which works because every marker here is itself a short run of ASCII
//! that appears verbatim in the artifact regardless of what
//! (binary-garbage) bytes surround it.
//!
//! **Measured, not assumed:** on the real `docker`/`gh` binaries this
//! project's own spec cites for the `spf13/cobra` marker [M-13], the
//! first occurrence sits at roughly 28-33% into the file — 11.9 MB into a
//! 43 MB `docker`, 14.9 MB into a 45 MB `gh` (both are Go binaries built
//! with debug info, not stripped). A conservative few-MB cap would miss
//! spec's own reference examples entirely. [`MAX_BINARY_SCAN_BYTES`] is
//! set high enough to fully cover binaries in that size class while still
//! bounding the pathological (100+ MB) case this doc comment's "don't
//! slurp" guidance is about.

use super::Framework;
use std::fs::File;
use std::io::Read;
use std::path::Path;

/// Total bytes read from a binary artifact across the whole scan. See the
/// module doc comment for why this is tens, not single-digit, megabytes.
const MAX_BINARY_SCAN_BYTES: usize = 48 * 1024 * 1024;

/// Read chunk size for the streaming continuation of a binary scan.
const CHUNK_SIZE: usize = 512 * 1024;

/// Carried over between chunks so a marker split across a chunk boundary
/// is never missed — must be at least as long as the longest marker minus
/// one byte. The longest marker here is 74 bytes (GNU argp's mandatory-
/// arguments sentence); 128 leaves headroom for longer markers added
/// later without needing to revisit this constant.
const OVERLAP: usize = 128;

/// Bytes read from a script artifact. Scripts are text and shebang/import
/// conventions put everything relevant at the top, so this is far smaller
/// than the binary budget.
const MAX_SCRIPT_SCAN_BYTES: usize = 64 * 1024;

/// Marker → framework table for compiled-binary artifacts, **in priority
/// order** (index 0 = most authoritative; see [`update_best`]). Each
/// marker is either a crate/package path fragment that only appears when
/// that framework is actually linked in, or a literal message string
/// unique to one framework's own source.
///
/// Priority order matters because markers can genuinely co-occur in one
/// binary without the tool actually being built on the lower-priority
/// framework: measured directly on the real `docker` and `gh` binaries
/// this project's spec cites for the `spf13/cobra` marker [M-13], both
/// *also* contain Go's stdlib `flag` package's `"flag provided but not
/// defined:"` error string — 330 KB *earlier* in `docker`'s case — even
/// though neither tool's CLI is built on stdlib `flag`. Something else in
/// their dependency graph pulls the package in transitively (the stdlib
/// `flag` package is a common transitive import — e.g. via `testing` or
/// other tooling — independent of whether it's the CLI framework in use).
/// `GoFlag` is therefore ordered last: a scan keeps looking past a
/// `GoFlag` hit for anything earlier in this list before settling.
const BINARY_MARKERS: &[(&[u8], Framework)] = &[
    (b"spf13/cobra", Framework::Cobra),
    (b"urfave/cli", Framework::UrfaveCli),
    // clap v3+ split its implementation into an internal `clap_builder`
    // crate; v2 predates that split, so its absence alongside a `/clap-2.`
    // path fragment is the v2 signal instead.
    (b"clap_builder", Framework::ClapV3V4),
    (b"/clap-2.", Framework::ClapV2),
    // Two distinct wordings measured on real systems, both GNU argp
    // conventions: coreutils' own hand-rolled `--help` printer (`ls`,
    // `cp`, `rm`, ...) uses the first; the actual glibc `argp` library
    // footer (`tar`, and other tools that link it directly) uses the
    // second, newer phrasing.
    (
        b"Mandatory arguments to long options are mandatory for short options too.",
        Framework::GnuArgp,
    ),
    (
        b"Mandatory or optional arguments to long options are also mandatory or optional",
        Framework::GnuArgp,
    ),
    (b"BusyBox is copyrighted", Framework::Busybox),
    (b"picocli.CommandLine", Framework::Picocli),
    (b"System.CommandLine", Framework::DotNetSystemCommandLine),
    (b"flag provided but not defined:", Framework::GoFlag),
];

/// Interpreter named on a script's shebang line, used to pick which of
/// [`SCRIPT_MARKERS`] apply.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ScriptLang {
    Python,
    Node,
    Php,
    Ruby,
}

/// Marker → framework table for scripts, gated by [`ScriptLang`] so e.g. a
/// Python docstring mentioning "commander" can't be mistaken for a Node
/// script.
const SCRIPT_MARKERS: &[(ScriptLang, &[u8], Framework)] = &[
    (ScriptLang::Python, b"import argparse", Framework::Argparse),
    (ScriptLang::Python, b"from argparse", Framework::Argparse),
    (ScriptLang::Python, b"import click", Framework::Click),
    (ScriptLang::Python, b"from click", Framework::Click),
    (ScriptLang::Python, b"@click.command", Framework::Click),
    (ScriptLang::Python, b"import docopt", Framework::Docopt),
    (ScriptLang::Python, b"from docopt", Framework::Docopt),
    (
        ScriptLang::Node,
        b"require('commander')",
        Framework::Commander,
    ),
    (
        ScriptLang::Node,
        b"require(\"commander\")",
        Framework::Commander,
    ),
    (ScriptLang::Node, b"from 'commander'", Framework::Commander),
    (
        ScriptLang::Node,
        b"from \"commander\"",
        Framework::Commander,
    ),
    (ScriptLang::Node, b"require('yargs')", Framework::Yargs),
    (ScriptLang::Node, b"require(\"yargs\")", Framework::Yargs),
    (ScriptLang::Node, b"from 'yargs'", Framework::Yargs),
    (ScriptLang::Node, b"@oclif/core", Framework::Oclif),
    (
        ScriptLang::Php,
        b"Symfony\\Component\\Console",
        Framework::SymfonyConsole,
    ),
    (
        ScriptLang::Ruby,
        b"require 'optparse'",
        Framework::OptionParserOrThor,
    ),
    (
        ScriptLang::Ruby,
        b"require \"optparse\"",
        Framework::OptionParserOrThor,
    ),
    (
        ScriptLang::Ruby,
        b"require 'thor'",
        Framework::OptionParserOrThor,
    ),
    (
        ScriptLang::Ruby,
        b"require \"thor\"",
        Framework::OptionParserOrThor,
    ),
];

/// Scan `path`'s artifact bytes for a framework marker. `None` on any I/O
/// error (treated identically to "no marker found" — artifact scanning is
/// best-effort, and step 2 of spec §7 Tier A′ exists for exactly this
/// case) or if nothing matched within the bound.
pub fn scan(path: &Path) -> Option<Framework> {
    let mut file = File::open(path).ok()?;
    let mut head = vec![0u8; MAX_SCRIPT_SCAN_BYTES];
    let n = read_fill(&mut file, &mut head).ok()?;
    head.truncate(n);

    if head.starts_with(b"#!") {
        return scan_script(&head);
    }

    let mut best: Option<usize> = None;
    update_best(&mut best, &head);
    if best == Some(0) {
        return Some(BINARY_MARKERS[0].1);
    }
    // Seed the streaming continuation with `head`'s own tail, so a marker
    // straddling the boundary between this initial read and the first
    // streamed chunk is still caught (not just boundaries *between*
    // streamed chunks).
    let keep_from = head.len().saturating_sub(OVERLAP);
    let initial_tail = head[keep_from..].to_vec();
    scan_binary_stream(&mut file, head.len(), initial_tail, best).map(|i| BINARY_MARKERS[i].1)
}

fn scan_script(head: &[u8]) -> Option<Framework> {
    let lang = shebang_lang(head)?;
    for (marker_lang, marker, framework) in SCRIPT_MARKERS {
        if *marker_lang == lang && contains(head, marker) {
            return Some(*framework);
        }
    }
    None
}

fn shebang_lang(head: &[u8]) -> Option<ScriptLang> {
    let end = head.iter().position(|&b| b == b'\n').unwrap_or(head.len());
    let first_line = std::str::from_utf8(&head[..end]).ok()?.to_lowercase();
    if first_line.contains("python") {
        Some(ScriptLang::Python)
    } else if first_line.contains("node") {
        Some(ScriptLang::Node)
    } else if first_line.contains("php") {
        Some(ScriptLang::Php)
    } else if first_line.contains("ruby") {
        Some(ScriptLang::Ruby)
    } else {
        None
    }
}

/// Continue a binary scan past the bytes already examined in `head`,
/// streaming further chunks (each with the previous chunk's tail
/// prepended, so a marker spanning a chunk boundary is never missed) up to
/// [`MAX_BINARY_SCAN_BYTES`] total. `best` carries forward whatever
/// [`update_best`] already found in `head`; scanning stops as soon as it
/// reaches index 0 (nothing can outrank it) or the byte budget/file ends.
fn scan_binary_stream(
    file: &mut File,
    already_read: usize,
    initial_tail: Vec<u8>,
    mut best: Option<usize>,
) -> Option<usize> {
    if best == Some(0) {
        return best;
    }
    let mut remaining = MAX_BINARY_SCAN_BYTES.saturating_sub(already_read);
    let mut tail: Vec<u8> = initial_tail;
    let mut chunk = vec![0u8; CHUNK_SIZE];

    while remaining > 0 {
        let want = CHUNK_SIZE.min(remaining);
        let n = match file.read(&mut chunk[..want]) {
            Ok(0) => break,
            Ok(n) => n,
            Err(_) => break,
        };
        let mut window = Vec::with_capacity(tail.len() + n);
        window.extend_from_slice(&tail);
        window.extend_from_slice(&chunk[..n]);

        update_best(&mut best, &window);
        if best == Some(0) {
            return best;
        }

        let keep_from = window.len().saturating_sub(OVERLAP);
        tail = window[keep_from..].to_vec();
        remaining = remaining.saturating_sub(n);
    }
    best
}

/// Update `best` (an index into [`BINARY_MARKERS`], lower = higher
/// priority) if `chunk` contains a marker that outranks whatever `best`
/// already holds. Only checks markers ranked better than the current best
/// — once something has matched, nothing worse than it could ever change
/// the answer, so there's no reason to keep testing for it.
fn update_best(best: &mut Option<usize>, chunk: &[u8]) {
    let limit = best.unwrap_or(BINARY_MARKERS.len());
    for (i, (marker, _)) in BINARY_MARKERS.iter().enumerate() {
        if i >= limit {
            break;
        }
        if contains(chunk, marker) {
            *best = Some(i);
            if i == 0 {
                return;
            }
        }
    }
}

fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    needle.len() <= haystack.len() && haystack.windows(needle.len()).any(|w| w == needle)
}

/// Read into `buf` until it's full or EOF, returning the number of bytes
/// actually read — a single `Read::read` call may return fewer bytes than
/// requested even mid-stream.
fn read_fill(file: &mut File, buf: &mut [u8]) -> std::io::Result<usize> {
    let mut total = 0;
    while total < buf.len() {
        match file.read(&mut buf[total..])? {
            0 => break,
            n => total += n,
        }
    }
    Ok(total)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn write_file(dir: &Path, name: &str, contents: &[u8]) -> std::path::PathBuf {
        let path = dir.join(name);
        let mut f = File::create(&path).unwrap();
        f.write_all(contents).unwrap();
        path
    }

    #[test]
    fn detects_cobra_marker_in_binary_bytes() {
        let dir = tempfile::tempdir().unwrap();
        let mut payload = vec![0xFFu8; 4096];
        payload.extend_from_slice(b"github.com/spf13/cobra");
        payload.extend_from_slice(&[0x00u8; 4096]);
        let path = write_file(dir.path(), "fakebin", &payload);
        assert_eq!(scan(&path), Some(Framework::Cobra));
    }

    #[test]
    fn detects_clap_v3_v4_marker() {
        let dir = tempfile::tempdir().unwrap();
        let mut payload = vec![0xAAu8; 1024];
        payload.extend_from_slice(
            b"/root/.cargo/registry/src/index.crates.io/clap_builder-4.5.0/src/lib.rs",
        );
        let path = write_file(dir.path(), "fakebin", &payload);
        assert_eq!(scan(&path), Some(Framework::ClapV3V4));
    }

    #[test]
    fn detects_marker_spanning_a_chunk_boundary() {
        let dir = tempfile::tempdir().unwrap();
        // Place the marker so it straddles the boundary between the
        // initial head-read and the first streamed chunk.
        let mut payload = vec![0u8; MAX_SCRIPT_SCAN_BYTES - 5];
        payload.extend_from_slice(b"spf13/cobra");
        payload.extend_from_slice(&[0u8; 4096]);
        let path = write_file(dir.path(), "fakebin", &payload);
        assert_eq!(scan(&path), Some(Framework::Cobra));
    }

    /// Regression for the real collision found on the actual `docker`
    /// binary (see [`BINARY_MARKERS`]'s doc comment): a lower-priority
    /// marker ([`Framework::GoFlag`]) appearing *earlier* in the file than
    /// a higher-priority one ([`Framework::Cobra`]) must not win just
    /// because it was scanned first.
    #[test]
    fn higher_priority_marker_wins_even_when_it_appears_later() {
        let dir = tempfile::tempdir().unwrap();
        let mut payload = vec![0u8; 4096];
        payload.extend_from_slice(b"flag provided but not defined:");
        payload.extend_from_slice(&[0u8; 8192]);
        payload.extend_from_slice(b"spf13/cobra");
        payload.extend_from_slice(&[0u8; 4096]);
        let path = write_file(dir.path(), "fakebin", &payload);
        assert_eq!(scan(&path), Some(Framework::Cobra));
    }

    /// The mirror case: with no higher-priority marker anywhere in the
    /// scanned budget, the lower-priority one is still the right answer —
    /// this isn't "GoFlag never wins," it's "GoFlag loses only when
    /// outranked."
    #[test]
    fn lower_priority_marker_wins_when_alone() {
        let dir = tempfile::tempdir().unwrap();
        let mut payload = vec![0u8; 4096];
        payload.extend_from_slice(b"flag provided but not defined:");
        let path = write_file(dir.path(), "fakebin", &payload);
        assert_eq!(scan(&path), Some(Framework::GoFlag));
    }

    #[test]
    fn no_marker_present_returns_none() {
        let dir = tempfile::tempdir().unwrap();
        let path = write_file(dir.path(), "fakebin", &[0x01, 0x02, 0x03, 0x04]);
        assert_eq!(scan(&path), None);
    }

    #[test]
    fn detects_argparse_from_python_shebang_and_import() {
        let dir = tempfile::tempdir().unwrap();
        let path = write_file(
            dir.path(),
            "tool.py",
            b"#!/usr/bin/env python3\nimport argparse\n\ndef main():\n    pass\n",
        );
        assert_eq!(scan(&path), Some(Framework::Argparse));
    }

    #[test]
    fn detects_click_from_python_shebang_and_decorator() {
        let dir = tempfile::tempdir().unwrap();
        let path = write_file(
            dir.path(),
            "tool.py",
            b"#!/usr/bin/env python3\nimport click\n\n@click.command()\ndef main():\n    pass\n",
        );
        assert_eq!(scan(&path), Some(Framework::Click));
    }

    #[test]
    fn detects_commander_from_node_shebang_and_require() {
        let dir = tempfile::tempdir().unwrap();
        let path = write_file(
            dir.path(),
            "tool.js",
            b"#!/usr/bin/env node\nconst { program } = require('commander');\n",
        );
        assert_eq!(scan(&path), Some(Framework::Commander));
    }

    /// A Python script mentioning "commander" in, say, a comment must not
    /// be misread as a Node script — script markers are gated by the
    /// interpreter named on the shebang line, not matched language-
    /// agnostically.
    #[test]
    fn script_markers_are_gated_by_shebang_language() {
        let dir = tempfile::tempdir().unwrap();
        let path = write_file(
            dir.path(),
            "tool.py",
            b"#!/usr/bin/env python3\n# not a require('commander') call, just prose\nimport sys\n",
        );
        assert_eq!(scan(&path), None);
    }

    #[test]
    fn missing_file_returns_none_not_a_panic() {
        let path = Path::new("/definitely/does/not/exist/xyz");
        assert_eq!(scan(path), None);
    }
}
