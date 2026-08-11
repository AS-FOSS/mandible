//! End-to-end proof of spec §6 rules 1-3, run through the actual sanctioned
//! path (`mandible_extract::exec::run_inert`), against a shim binary that
//! logs exactly what it was invoked with. Spec §13.3 calls this out
//! explicitly as a required test class ("Execution-policy tests: a shim
//! binary logs argv/env; any invocation outside the allowlist fails the
//! suite.") and as the fix for a specific prior bug ("Real-argv tests":
//! a mocked probe can pass while the real argv construction is broken).

use mandible_extract::exec::{run_inert, ExecError, InertArgv};
use mandible_extract::help_text::HelpTextTier;
use mandible_extract::{ExtractError, ExtractionTier, NodeHints, ResolvedTool};
use std::io::Write;
use std::path::Path;
use std::time::Duration;

/// Like [`write_shim`], but with a caller-chosen name and script — needed
/// for the [M-16] D1.3.2 tests below, which must name a shim `pkill` to
/// exercise `HELP_ONLY_PROBE` matching (spec §6 rule 0's file-name check)
/// and need custom argv-dependent behaviour rather than the fixed
/// argv-dumping script `write_shim` always installs.
fn write_named_shim(dir: &Path, name: &str, script: &str) -> std::path::PathBuf {
    let path = dir.join(name);
    let mut f = std::fs::File::create(&path).unwrap();
    f.write_all(script.as_bytes()).unwrap();
    drop(f);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    path
}

fn write_shim(dir: &std::path::Path) -> std::path::PathBuf {
    let path = dir.join("shim.sh");
    let script = r#"#!/bin/sh
echo "ARGC:$#"
i=0
for a in "$@"; do
    echo "ARGV[$i]:$a"
    i=$((i + 1))
done
echo "ENV_COMPLETE:${COMPLETE:-<unset>}"
echo "ENV_TERM:${TERM:-<unset>}"
echo "ENV_NO_COLOR:${NO_COLOR:-<unset>}"
echo "ENV_LESS:${LESS:-<unset>}"
if IFS= read -r line; then
    echo "STDIN:GOT:$line"
else
    echo "STDIN:EOF"
fi
"#;
    let mut f = std::fs::File::create(&path).unwrap();
    f.write_all(script.as_bytes()).unwrap();
    drop(f);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    path
}

/// Rule 1 (never bare) + rule 2 (only inert shapes): drive every
/// `InertArgv` variant through the real `run_inert` path and confirm the
/// shim actually received a non-empty, well-formed argv matching the
/// variant — not a mocked stand-in.
#[test]
fn every_inert_argv_shape_reaches_the_child_correctly() {
    let dir = tempfile::tempdir().unwrap();
    let shim = write_shim(dir.path());

    let cases: Vec<(InertArgv, Vec<&str>)> = vec![
        (
            InertArgv::CobraComplete {
                words: vec!["pr".to_string()],
            },
            vec!["__complete", "pr"],
        ),
        (
            InertArgv::CompletionScript {
                shell: "zsh".to_string(),
            },
            vec!["completion", "zsh"],
        ),
        (InertArgv::HelpLong, vec!["--help"]),
        (InertArgv::HelpShort, vec!["-h"]),
        (
            InertArgv::HelpSubcommand {
                words: vec!["rebase".to_string()],
            },
            vec!["help", "rebase"],
        ),
    ];

    for (argv, expected) in cases {
        let out = run_inert(&shim, &argv, Duration::from_secs(2)).unwrap();
        let text = String::from_utf8_lossy(&out.stdout);
        let argc_line = text.lines().next().unwrap();
        assert_eq!(
            argc_line,
            format!("ARGC:{}", expected.len()),
            "argv={argv:?}"
        );
        for (i, exp) in expected.iter().enumerate() {
            let want = format!("ARGV[{i}]:{exp}");
            assert!(
                text.contains(&want),
                "expected {want:?} in output for {argv:?}:\n{text}"
            );
        }
        // Rule 3: stdin is always /dev/null.
        assert!(
            text.contains("STDIN:EOF"),
            "stdin should be immediately EOF for {argv:?}:\n{text}"
        );
    }
}

/// Rule 6 (sanitized environment) end to end: the `COMPLETE=` variable is
/// set only for the clap probe shapes, and the always-present baseline
/// vars land correctly, through the real spawn path.
#[test]
fn clap_complete_env_shape_carries_its_env_var_to_the_real_child() {
    let dir = tempfile::tempdir().unwrap();
    let shim = write_shim(dir.path());

    let argv = InertArgv::ClapCompleteEnvProbe {
        shell: "zsh".to_string(),
    };
    let out = run_inert(&shim, &argv, Duration::from_secs(2)).unwrap();
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(
        text.contains("ARGV[0]:--"),
        "clap probe must send literal '--', never bare:\n{text}"
    );
    assert!(text.contains("ENV_COMPLETE:zsh"), "{text}");
    assert!(text.contains("ENV_TERM:dumb"), "{text}");
    assert!(text.contains("ENV_NO_COLOR:1"), "{text}");
    assert!(
        text.contains("ENV_LESS:<unset>"),
        "LESS must not leak through:\n{text}"
    );
}

/// Rule 2a: an empty argument the tool could read as its first positional
/// is refused before anything is spawned.
///
/// This is the shape behind the machine reset that motivated rule 0.
/// `ClapCompleteEnvComplete { partial: "" }` renders as `-- ""`; because
/// `--` is the option terminator essentially every getopt program
/// discards, the empty string arrives as the first positional, and a
/// program whose first positional is a pattern reads it as "match
/// everything". Measured: `pkill -- ""` terminated every process in a
/// private PID namespace, pkill included. The never-probe list hid this
/// for thirteen tools while the same argv was still emitted at the rest of
/// PATH, so the fix belongs at the chokepoint, not in a name list.
#[test]
fn empty_first_positional_is_refused_before_spawning() {
    let dir = tempfile::tempdir().unwrap();
    let shim = write_shim(dir.path());

    let refused = InertArgv::ClapCompleteEnvComplete {
        shell: "zsh".to_string(),
        partial: String::new(),
    };
    assert_eq!(refused.args(), vec!["--".to_string(), String::new()]);

    let err = run_inert(&shim, &refused, Duration::from_secs(2))
        .expect_err("`-- \"\"` must be refused, not spawned");
    assert!(
        err.to_string().contains("empty argument"),
        "unexpected error: {err}"
    );

    // The safe expression of the same request must still reach the child:
    // `--` alone, which `ClapCompleteEnvProbe` exists to produce.
    let allowed = InertArgv::ClapCompleteEnvProbe {
        shell: "zsh".to_string(),
    };
    let out = run_inert(&shim, &allowed, Duration::from_secs(2)).unwrap();
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(text.contains("ARGC:1"), "{text}");
    assert!(text.contains("ARGV[0]:--"), "{text}");
}

/// The one empty argument that is allowed, and why: cobra's completion
/// word is protocol-required — `docker __complete` without it fails with
/// "requires at least 1 arg(s), only received 0" and native detection
/// collapses for every cobra tool. It is safe for a reason the chokepoint
/// can check: it is never the first positional, always shielded behind the
/// `__complete` sentinel, which a non-cobra tool rejects rather than acts
/// on.
#[test]
fn cobra_completion_word_may_be_empty_because_a_sentinel_guards_it() {
    let dir = tempfile::tempdir().unwrap();
    let shim = write_shim(dir.path());

    let argv = InertArgv::CobraComplete {
        words: vec![String::new()],
    };
    let out = run_inert(&shim, &argv, Duration::from_secs(2))
        .expect("cobra's empty completion word must still be permitted");
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(text.contains("ARGC:2"), "{text}");
    assert!(text.contains("ARGV[0]:__complete"), "{text}");

    // But the sentinel must genuinely be the guard: an empty *first*
    // argument is refused even for this variant.
    let unguarded = InertArgv::HelpSubcommand {
        words: vec![String::new()],
    };
    assert!(
        run_inert(&shim, &unguarded, Duration::from_secs(2)).is_err(),
        "`help \"\"` has no sentinel shielding the empty word"
    );
}

// --- [M-16] sub-case (a): the `-h` fallback for a man-shaped subcommand
// --- probe, D1.3.2's "both halves matter" shim suite ---
//
// These drive `HelpTextTier::extract_node` — not `run_inert` directly —
// against real shim binaries, through the tier's actual `Probe`
// construction (`HelpTextTier::default()` uses the live `LiveProbe`, so
// every probe below is a real subprocess spawn through the real
// `run_inert` chokepoint, exactly like the tests above). A rendered man
// page is built from the same banner shape `git bisect --help` actually
// produces (identical `NAME(section)` token at both margins around a
// centred title) — see `help_text::sections::is_man_page_banner`'s own
// tests for the real fixture this is modeled on.

/// A minimal man-page banner in the exact shape `looks_like_man_page`
/// recognizes: identical `NAME(1)` token at both margins, a centred title
/// between them.
fn man_page_banner(name: &str) -> String {
    let tag = format!("{}(1)", name.to_uppercase());
    format!(
        "{tag}                Some Manual                {tag}\n\nNAME\n     {name} - a thing\n"
    )
}

/// Half one: a permitted tool's subcommand whose `--help` renders a man
/// The verbatim view (`t`) must fetch **the document the parse read**.
///
/// Its whole purpose is letting a reader check our reading against the
/// author's own bytes — which only works if both are the same bytes. When
/// [M-16] sub-case (a) fires the parse came from `-h`, not from the man
/// page `--help` returned, so a raw fetch that re-probed without the same
/// attestation would show a different document than the tree came from and
/// silently answer a question nobody asked. That shipped briefly:
/// `raw_help` hardcoded `heading_attested: false`.
#[test]
fn raw_help_fetches_the_same_document_the_parse_read() {
    let dir = tempfile::tempdir().unwrap();
    let script = format!(
        r#"#!/bin/sh
if [ "$1" = "sub" ] && [ "$2" = "--help" ]; then
    printf '%s' '{banner}'
    exit 0
fi
if [ "$1" = "sub" ] && [ "$2" = "-h" ]; then
    echo "Usage: manthing sub [options]"
    echo ""
    echo "Options:"
    echo "  --amend      Amend the previous thing"
    exit 0
fi
echo "unexpected argv: $@" >&2
exit 1
"#,
        banner = man_page_banner("manthing-sub").replace('\'', "'\\''")
    );
    let shim = write_named_shim(dir.path(), "manthing", &script);
    let tool = ResolvedTool {
        name: "manthing".to_string(),
        path: Some(shim.clone()),
        version: None,
    };
    let path = ["manthing".to_string(), "sub".to_string()];
    let attested = NodeHints {
        heading_attested: true,
    };

    let (raw, flag) = mandible_extract::help_text::raw_help(&tool, &path, attested)
        .expect("the shim answers both probes");
    // The pane labels itself from this, so a wrong value is a false claim
    // about where the bytes came from, not a cosmetic slip.
    assert_eq!(flag, "-h", "raw help must report the argv it actually ran");
    let joined: String = raw
        .iter()
        .map(|t| t.as_str().to_string())
        .collect::<Vec<_>>()
        .join("\n");

    assert!(
        joined.contains("--amend"),
        "raw help must show the -h document the parse actually read: {joined}"
    );
    assert!(
        !joined.contains("MANTHING-SUB(1)"),
        "raw help showed the man page the parse discarded — the verbatim \
         view is answering the wrong question: {joined}"
    );
}

/// page must trigger the `-h` fallback, and the fallback's output — an
/// ordinary option table — must actually be what the node parses to,
/// rather than the man page staying as verbatim degradation.
#[test]
fn man_shaped_subcommand_help_triggers_the_dash_h_fallback_when_permitted() {
    let dir = tempfile::tempdir().unwrap();
    let script = format!(
        r#"#!/bin/sh
if [ "$1" = "sub" ] && [ "$2" = "--help" ]; then
    printf '%s' '{banner}'
    exit 0
fi
if [ "$1" = "sub" ] && [ "$2" = "-h" ]; then
    touch "$0.dash_h_ran"
    echo "Usage: manthing sub [options]"
    echo ""
    echo "Options:"
    echo "  --amend      Amend the previous thing"
    echo "  --dry-run    Do not actually do anything"
    exit 0
fi
echo "unexpected argv: $@" >&2
exit 1
"#,
        banner = man_page_banner("manthing-sub").replace('\'', "'\\''")
    );
    let shim = write_named_shim(dir.path(), "manthing", &script);

    let tier = HelpTextTier::default();
    let tool = ResolvedTool {
        name: "manthing".to_string(),
        path: Some(shim.clone()),
        version: None,
    };
    let node = tier
        .extract_node(
            &tool,
            &["manthing".to_string(), "sub".to_string()],
            NodeHints {
                heading_attested: true,
            },
        )
        .expect("the shim always answers one of the two probes it's asked for");

    assert!(
        node.unparsed.is_empty(),
        "node degraded to verbatim instead of using the -h fallback's real flags: {node:?}"
    );
    let long_flags: Vec<&str> = node
        .flags
        .iter()
        .filter_map(|f| f.long.as_deref())
        .collect();
    assert!(long_flags.contains(&"amend"), "{long_flags:?}");
    assert!(long_flags.contains(&"dry-run"), "{long_flags:?}");
    assert!(
        dir.path().join("manthing.dash_h_ran").exists(),
        "the -h fallback's marker was never written — -h was never actually invoked"
    );
}

/// Half two: a shim named like a never-probe tool (spec §6 rule 0,
/// `HELP_ONLY_PROBE`) must never receive the `-h` fallback, even in a
/// scenario shaped to trigger it. `pkill`'s subcommand-path `--help` probe
/// (`InertArgv::HelpLongForPath` with non-empty words renders to
/// `[..words, "--help"]`, never exactly `["--help"]`) is itself already
/// refused by `run_inert`'s chokepoint before this tier's new fallback
/// logic ever runs — which is the strongest form of "cannot route around
/// it": the fallback code path is never even reached, because the probe
/// that would have supplied it man-shaped text to react to never completes.
/// The shim unconditionally leaves a marker on every invocation, so this
/// also proves the refusal happens before any spawn, not merely before the
/// tier acts on a result.
#[test]
fn never_probe_named_shim_never_receives_the_dash_h_fallback_even_when_man_shaped() {
    let dir = tempfile::tempdir().unwrap();
    let shim = write_named_shim(
        dir.path(),
        "pkill",
        "#!/bin/sh\ntouch \"$0.ran\"\necho ran\n",
    );

    let tier = HelpTextTier::default();
    let tool = ResolvedTool {
        name: "pkill".to_string(),
        path: Some(shim.clone()),
        version: None,
    };
    let result = tier.extract_node(
        &tool,
        &["pkill".to_string(), "sub".to_string()],
        NodeHints {
            heading_attested: true,
        },
    );

    assert!(
        matches!(
            result,
            Err(ExtractError::Exec(ExecError::RefusedUnsafeTool { .. }))
        ),
        "expected the never-probe list to refuse the subcommand `--help` probe outright, got {result:?}"
    );
    assert!(
        !dir.path().join("pkill.ran").exists(),
        "the never-probe shim was executed at all — refusal did not happen before spawn"
    );
}

/// Half three: a subcommand word that did *not* come from a structural
/// source (`hints.heading_attested: false`) must never receive the `-h`
/// fallback either, even though its (still-sent, spec §6-permitted)
/// `--help` probe comes back man-shaped. This is the provenance gate spec
/// §6 rule 0's closing paragraph calls for: `words` must be attested before
/// the *new* argv shape (`-h`) may be sent for it. It does not (and is not
/// meant to) touch the pre-existing `<words...> --help` probe itself, which
/// still fires regardless of attestation — a separate, pre-existing gap,
/// not one this test closes.
#[test]
fn non_attested_subcommand_word_gets_no_dash_h_probe_at_all() {
    let dir = tempfile::tempdir().unwrap();
    let script = format!(
        r#"#!/bin/sh
if [ "$1" = "sub" ] && [ "$2" = "--help" ]; then
    printf '%s' '{banner}'
    exit 0
fi
if [ "$1" = "sub" ] && [ "$2" = "-h" ]; then
    touch "$0.dash_h_ran"
    echo "Usage: unattested sub [options]"
    echo ""
    echo "Options:"
    echo "  --amend  Amend the previous thing"
    exit 0
fi
echo "unexpected argv: $@" >&2
exit 1
"#,
        banner = man_page_banner("unattested-sub").replace('\'', "'\\''")
    );
    let shim = write_named_shim(dir.path(), "unattested", &script);

    let tier = HelpTextTier::default();
    let tool = ResolvedTool {
        name: "unattested".to_string(),
        path: Some(shim.clone()),
        version: None,
    };
    let node = tier
        .extract_node(
            &tool,
            &["unattested".to_string(), "sub".to_string()],
            NodeHints {
                heading_attested: false,
            },
        )
        .expect("the --help probe itself is still permitted and still answered");

    assert!(
        !node.unparsed.is_empty(),
        "node parsed structure from the man page instead of degrading to verbatim: {node:?}"
    );
    assert!(
        !dir.path().join("unattested.dash_h_ran").exists(),
        "the -h fallback ran despite the word not being heading_attested"
    );
}
