//! Tier C: completion script structural parsing (spec §7 Tier C).
//!
//! Generates `<tool> completion zsh` (falling back to `bash`), never
//! executes the result, and walks it as a real shell AST via
//! [`brush_parser`] — not `conch-parser` (unmaintained, emits a
//! future-incompat build warning [M-9]) and not `yash-syntax` (GPLv3,
//! which would oblige the whole binary).
//!
//! **zsh first.** A zsh completion function's `_arguments` calls carry a
//! flag's spelling *and* description in one structure:
//! `'-v[enable verbose output]'`, or the common brace-pair idiom for a
//! flag with both forms, `'(-v --verbose)'{-v,--verbose}'[enable verbose
//! output]'`. `brush_parser`'s `Word` keeps a token's *raw* source text
//! (quotes, braces, and all) rather than performing shell expansion —
//! correct, since parsing must never execute anything a real shell would
//! (brace expansion is a runtime step) — so this module does its own
//! small, targeted expansion of exactly that idiom before applying a
//! hand-written `_arguments` spec-string grammar. It does not attempt
//! general shell brace-expansion or full `_arguments` grammar coverage
//! (value messages, actions, and nested exclusion lists past the first
//! are read past, not interpreted) — the payload this tier exists for is
//! spelling + description, not a completion engine.
//!
//! **bash as a fallback, spellings only.** Bash completion functions
//! mostly compute candidates at runtime (`compgen`, `COMPREPLY=(...)`
//! built from calling the tool itself) rather than declaring them
//! statically, so there is usually much less to recover structurally than
//! there first appears. This tier only recognizes the one common static
//! shape, `complete -W "word1 word2 ..." <tool>`, and recovers bare
//! spellings with no descriptions — matching spec §7 Tier C's own
//! characterization of what bash can realistically offer here.
//!
//! **Root-only contribution.** A completion script's dispatch logic
//! (usually a `case "$words[2]"`-style branch per subcommand) would need
//! to be *simulated*, not just parsed, to correctly attribute a nested
//! `_arguments` call to the right subcommand path — attempting that
//! without actually running the script risks silently misattributing a
//! subcommand's flags to a sibling or to the root. Rather than guess,
//! this tier is **not incremental** (spec §5.2's `is_incremental`):
//! it contributes once, to the root node only, and never claims to know
//! about deeper nodes.
//!
//! **Authority** (spec §4.4): structural 150, prose 30 — a completion
//! script's *existence* of a flag is fairly trustworthy (someone wrote it
//! down deliberately), but its prose is usually terser than a real
//! `--help` or a hand-maintained catalog entry.

use crate::errors::ExtractError;
use crate::exec::{InertArgv, LiveProbe, Probe};
use crate::resolve::ResolvedTool;
use crate::tier::ExtractionTier;
use brush_parser::ast;
use mandible_core::{Authority, CommandNode, Flag, Provenance, Source, Text, ValueKind};
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

/// Wall-clock cap for a `detect`/`extract_node` probe (spec §6 rule 4).
/// Generating a completion script is a single spawn with no further
/// per-node cost (this tier isn't incremental), so both use the more
/// generous extract cap rather than detect's tighter one.
const PROBE_TIMEOUT: Duration = Duration::from_secs(10);

/// Tier C: parses a generated zsh (preferred) or bash completion script
/// for flag spellings and descriptions.
pub struct CompletionScriptTier {
    /// The source of a `completion <shell>` probe's output — [`LiveProbe`]
    /// in production ([`Self::default`]), or a [`crate::exec::Transcript`]
    /// to replay frozen bytes with zero subprocesses.
    probe: Arc<dyn Probe>,
}

impl Default for CompletionScriptTier {
    fn default() -> Self {
        Self::new(Arc::new(LiveProbe))
    }
}

impl CompletionScriptTier {
    /// Build this tier against an explicit probe.
    pub fn new(probe: Arc<dyn Probe>) -> Self {
        Self { probe }
    }
}

impl ExtractionTier for CompletionScriptTier {
    fn name(&self) -> &'static str {
        "completion_script"
    }

    fn authority(&self) -> Authority {
        Source::CompletionScript {
            shell: String::new(),
        }
        .authority()
    }

    fn detect(&self, tool: &ResolvedTool) -> bool {
        let Some(tool_path) = &tool.path else {
            return false;
        };
        probe_and_extract_flags(self.probe.as_ref(), tool_path).is_some()
    }

    fn extract_node(
        &self,
        tool: &ResolvedTool,
        path: &[String],
    ) -> Result<CommandNode, ExtractError> {
        let tool_path = tool.path.as_ref().ok_or(ExtractError::ToolNotFound)?;
        // Root-only contribution (see module doc). `is_incremental() ==
        // false` already keeps the lazy-fill path from ever calling this
        // for a deeper node, but this guard is cheap insurance against a
        // future caller doing so anyway.
        if path.len() > 1 {
            return Err(ExtractError::PathNotFound);
        }
        let (shell, flags) = probe_and_extract_flags(self.probe.as_ref(), tool_path)
            .ok_or_else(|| ExtractError::Other("no usable completion script found".to_string()))?;
        let name = path.last().cloned().unwrap_or_else(|| tool.name.clone());
        let mut node =
            CommandNode::new(name, Provenance::single(Source::CompletionScript { shell }));
        node.flags = flags;
        Ok(node)
    }

    fn is_incremental(&self) -> bool {
        // A single probe already returns everything this tier will ever
        // know (see the module doc's "root-only contribution").
        false
    }
}

/// Request zsh first, bash as fallback (spec §7 Tier C); return the shell
/// name used and the flags recovered, or `None` if neither produced a
/// script with anything this tier recognizes.
fn probe_and_extract_flags(probe: &dyn Probe, tool_path: &Path) -> Option<(String, Vec<Flag>)> {
    for shell in ["zsh", "bash"] {
        let Ok(out) = probe.run(
            tool_path,
            &InertArgv::CompletionScript {
                shell: shell.to_string(),
            },
            PROBE_TIMEOUT,
        ) else {
            continue;
        };
        if out.stdout.is_empty() {
            continue;
        }
        let text = String::from_utf8_lossy(&out.stdout);
        let provenance = Provenance::single(Source::CompletionScript {
            shell: shell.to_string(),
        });
        let flags = if shell == "zsh" {
            extract_zsh_flags(&text, &provenance)
        } else {
            extract_bash_flags(&text, &provenance)
        };
        if !flags.is_empty() {
            return Some((shell.to_string(), flags));
        }
    }
    None
}

/// Parse `script` as shell source and collect every `SimpleCommand`
/// anywhere in it (regardless of nesting inside functions, if/case/while
/// blocks, brace groups, or subshells) — this tier only ever looks for
/// specific command *names* (`_arguments`, `complete`), never simulates
/// control flow, so flattening is sufficient and much simpler than a
/// structure-preserving walk.
fn all_simple_commands(script: &str) -> Vec<(String, Vec<String>)> {
    let cursor = std::io::Cursor::new(script.as_bytes());
    let reader = std::io::BufReader::new(cursor);
    let options = brush_parser::ParserOptions::default();
    let mut parser = brush_parser::Parser::new(reader, &options);
    let Ok(program) = parser.parse_program() else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for list in &program.complete_commands {
        walk_compound_list(list, &mut out);
    }
    out
}

fn walk_compound_list(list: &ast::CompoundList, out: &mut Vec<(String, Vec<String>)>) {
    for item in &list.0 {
        walk_and_or_list(&item.0, out);
    }
}

fn walk_and_or_list(list: &ast::AndOrList, out: &mut Vec<(String, Vec<String>)>) {
    walk_pipeline(&list.first, out);
    for a in &list.additional {
        match a {
            ast::AndOr::And(p) | ast::AndOr::Or(p) => walk_pipeline(p, out),
        }
    }
}

fn walk_pipeline(pipeline: &ast::Pipeline, out: &mut Vec<(String, Vec<String>)>) {
    for cmd in &pipeline.seq {
        walk_command(cmd, out);
    }
}

fn walk_command(cmd: &ast::Command, out: &mut Vec<(String, Vec<String>)>) {
    match cmd {
        ast::Command::Simple(simple) => {
            let Some(name) = &simple.word_or_name else {
                return;
            };
            let mut words = Vec::new();
            if let Some(suffix) = &simple.suffix {
                for item in &suffix.0 {
                    if let ast::CommandPrefixOrSuffixItem::Word(w) = item {
                        words.push(w.value.clone());
                    }
                }
            }
            out.push((name.value.clone(), words));
        }
        ast::Command::Compound(cc, _) => walk_compound_command(cc, out),
        ast::Command::Function(f) => walk_compound_command(&f.body.0, out),
        ast::Command::ExtendedTest(_, _) => {}
    }
}

fn walk_compound_command(cc: &ast::CompoundCommand, out: &mut Vec<(String, Vec<String>)>) {
    match cc {
        ast::CompoundCommand::BraceGroup(b) => walk_compound_list(&b.list, out),
        ast::CompoundCommand::Subshell(s) => walk_compound_list(&s.list, out),
        ast::CompoundCommand::ForClause(f) => walk_compound_list(&f.body.list, out),
        ast::CompoundCommand::CaseClause(c) => {
            for item in &c.cases {
                if let Some(cmd_list) = &item.cmd {
                    walk_compound_list(cmd_list, out);
                }
            }
        }
        ast::CompoundCommand::IfClause(i) => {
            walk_compound_list(&i.condition, out);
            walk_compound_list(&i.then, out);
            if let Some(elses) = &i.elses {
                for e in elses {
                    if let Some(cond) = &e.condition {
                        walk_compound_list(cond, out);
                    }
                    walk_compound_list(&e.body, out);
                }
            }
        }
        ast::CompoundCommand::WhileClause(w) | ast::CompoundCommand::UntilClause(w) => {
            walk_compound_list(&w.0, out);
            walk_compound_list(&w.1.list, out);
        }
        ast::CompoundCommand::Coprocess(c) => walk_command(&c.body, out),
        ast::CompoundCommand::Arithmetic(_) | ast::CompoundCommand::ArithmeticForClause(_) => {}
    }
}

/// Find every `_arguments` call anywhere in `script` and parse its
/// argument words as zsh `_arguments` spec strings into [`Flag`]s.
fn extract_zsh_flags(script: &str, provenance: &Provenance) -> Vec<Flag> {
    let mut flags = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for (name, words) in all_simple_commands(script) {
        if name != "_arguments" {
            continue;
        }
        for word in words {
            for spec in expand_brace_alternatives(&dequote_concat(&word)) {
                if let Some(parsed) = parse_arg_spec(&spec) {
                    let key = (parsed.short, parsed.long.clone());
                    if !seen.insert(key) {
                        continue;
                    }
                    flags.push(parsed.into_flag(provenance.clone()));
                }
            }
        }
    }
    flags
}

/// Find `complete -W "word1 word2 ..." <tool>` calls and recover the
/// listed words as bare, description-less flag spellings (spec §7 Tier
/// C: "bash ... spellings only").
fn extract_bash_flags(script: &str, provenance: &Provenance) -> Vec<Flag> {
    let mut flags = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for (name, words) in all_simple_commands(script) {
        if name != "complete" {
            continue;
        }
        let mut iter = words.iter();
        while let Some(word) = iter.next() {
            if word == "-W" {
                if let Some(list) = iter.next() {
                    for candidate in dequote_concat(list).split_whitespace() {
                        if let Some(parsed) = parse_bare_flag_spelling(candidate) {
                            let key = (parsed.short, parsed.long.clone());
                            if !seen.insert(key) {
                                continue;
                            }
                            flags.push(parsed.into_flag(provenance.clone()));
                        }
                    }
                }
            }
        }
    }
    flags
}

/// Strip a single level of quoting from each maximal quoted run in `raw`,
/// concatenating with any unquoted runs left as-is (in particular, an
/// unquoted `{...}` brace expression survives untouched, ready for
/// [`expand_brace_alternatives`]). Not general shell quote-removal
/// (no escape processing inside double quotes, no `$(...)` handling) —
/// `_arguments` spec strings in practice are simple literal single-quoted
/// text, and this only needs to handle that.
fn dequote_concat(raw: &str) -> String {
    let mut out = String::new();
    let mut chars = raw.chars();
    while let Some(c) = chars.next() {
        match c {
            '\'' => {
                for c2 in chars.by_ref() {
                    if c2 == '\'' {
                        break;
                    }
                    out.push(c2);
                }
            }
            '"' => {
                for c2 in chars.by_ref() {
                    if c2 == '"' {
                        break;
                    }
                    out.push(c2);
                }
            }
            other => out.push(other),
        }
    }
    out
}

/// Expand the common `(excl){-a,--long}[desc]` brace-pair idiom (already
/// dequoted) into one spec string per alternative. A word with no `{...}`
/// segment at all is returned unchanged as the sole element. Only the
/// first (and typically only) brace segment is expanded — `_arguments`
/// spec strings essentially never have more than one.
fn expand_brace_alternatives(dequoted: &str) -> Vec<String> {
    if let Some(start) = dequoted.find('{') {
        if let Some(rel_end) = dequoted[start..].find('}') {
            let end = start + rel_end;
            let prefix = &dequoted[..start];
            let alts = &dequoted[start + 1..end];
            let suffix = &dequoted[end + 1..];
            return alts
                .split(',')
                .map(|alt| format!("{prefix}{alt}{suffix}"))
                .collect();
        }
    }
    vec![dequoted.to_string()]
}

/// One parsed `_arguments` spec string's flag identity and description.
struct ParsedArgSpec {
    short: Option<char>,
    long: Option<String>,
    description: Option<String>,
    takes_value: bool,
}

impl ParsedArgSpec {
    fn into_flag(self, provenance: Provenance) -> Flag {
        Flag {
            short: self.short,
            long: self.long,
            value_name: None,
            value_kind: if self.takes_value {
                ValueKind::Required
            } else {
                ValueKind::None
            },
            choices: Vec::new(),
            repeatable: false,
            required: false,
            hidden: false,
            deprecated: None,
            inherited: false,
            group: None,
            description: self.description.as_deref().map(Text::sanitize),
            default: None,
            env_var: None,
            provenance,
        }
    }
}

/// Parse one already-dequoted, already-brace-expanded `_arguments` spec
/// string, e.g. `-v[enable verbose output]` or
/// `--format=[format]:format:(json yaml table)`. Returns `None` for
/// anything that isn't an option spec at all (`_arguments` also lists
/// positional-argument specs like `'*::args:_normal'`, which don't start
/// with a dash and are out of scope here). Value messages and actions
/// after the description (`:message:action`) are recognized just enough
/// to be skipped, not interpreted.
fn parse_arg_spec(spec: &str) -> Option<ParsedArgSpec> {
    let mut rest = spec;

    // Strip a leading exclusion list, `(-a -b)...`.
    if let Some(after_paren) = rest.strip_prefix('(') {
        if let Some(end) = after_paren.find(')') {
            rest = &after_paren[end + 1..];
        }
    }
    // Strip a leading repeat marker.
    rest = rest.trim_start_matches('*');

    if !rest.starts_with('-') {
        return None;
    }

    let spelling_end = rest.find(['[', '=', ':']).unwrap_or(rest.len());
    let spelling = &rest[..spelling_end];
    rest = &rest[spelling_end..];

    let takes_value = rest.starts_with('=');
    if takes_value {
        rest = &rest[1..];
    }

    let description = rest.strip_prefix('[').and_then(|after_bracket| {
        after_bracket
            .find(']')
            .map(|end| after_bracket[..end].to_string())
    });

    let (short, long) = parse_spelling(spelling)?;
    Some(ParsedArgSpec {
        short,
        long,
        description,
        takes_value,
    })
}

/// A bare spelling with no description at all (bash's `complete -W`
/// word list).
fn parse_bare_flag_spelling(word: &str) -> Option<ParsedArgSpec> {
    let (short, long) = parse_spelling(word)?;
    Some(ParsedArgSpec {
        short,
        long,
        description: None,
        takes_value: false,
    })
}

/// `--long-name` or `-x` (a single character after one dash) into
/// `(short, long)`. Anything else (a bare word, `-` alone, a multi-char
/// single-dash spelling like find's `-name`) is rejected rather than
/// guessed at — this tier only claims flags it can spell unambiguously.
fn parse_spelling(spelling: &str) -> Option<(Option<char>, Option<String>)> {
    if let Some(long) = spelling.strip_prefix("--") {
        if long.is_empty() {
            return None;
        }
        return Some((None, Some(long.to_string())));
    }
    if let Some(rest) = spelling.strip_prefix('-') {
        let mut chars = rest.chars();
        let c = chars.next()?;
        if chars.next().is_some() {
            return None;
        }
        return Some((Some(c), None));
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn prov() -> Provenance {
        Provenance::single(Source::CompletionScript {
            shell: "zsh".to_string(),
        })
    }

    #[test]
    fn dequote_concat_strips_single_and_double_quotes() {
        assert_eq!(dequote_concat("'hello world'"), "hello world");
        assert_eq!(dequote_concat("\"hello\""), "hello");
        assert_eq!(dequote_concat("'a'{b,c}'d'"), "a{b,c}d");
    }

    #[test]
    fn expand_brace_alternatives_produces_one_string_per_alternative() {
        let alts = expand_brace_alternatives("(-v --verbose){-v,--verbose}[enable verbose output]");
        assert_eq!(
            alts,
            vec![
                "(-v --verbose)-v[enable verbose output]".to_string(),
                "(-v --verbose)--verbose[enable verbose output]".to_string(),
            ]
        );
    }

    #[test]
    fn expand_brace_alternatives_passes_through_when_no_braces() {
        let alts = expand_brace_alternatives("-v[enable verbose output]");
        assert_eq!(alts, vec!["-v[enable verbose output]".to_string()]);
    }

    #[test]
    fn parses_a_simple_long_flag_spec() {
        let parsed = parse_arg_spec("--verbose[enable verbose output]").unwrap();
        assert_eq!(parsed.long.as_deref(), Some("verbose"));
        assert_eq!(parsed.description.as_deref(), Some("enable verbose output"));
        assert!(!parsed.takes_value);
    }

    #[test]
    fn parses_a_short_flag_spec_with_exclusion_list() {
        let parsed = parse_arg_spec("(-v --verbose)-v[enable verbose output]").unwrap();
        assert_eq!(parsed.short, Some('v'));
        assert_eq!(parsed.description.as_deref(), Some("enable verbose output"));
    }

    #[test]
    fn parses_a_value_taking_flag_and_ignores_the_action() {
        let parsed = parse_arg_spec("--format=[format]:format:(json yaml table)").unwrap();
        assert_eq!(parsed.long.as_deref(), Some("format"));
        assert!(parsed.takes_value);
        assert_eq!(parsed.description.as_deref(), Some("format"));
    }

    #[test]
    fn rejects_positional_specs() {
        assert!(parse_arg_spec("*::args:_normal").is_none());
        assert!(parse_arg_spec(":file:_files").is_none());
    }

    /// The end-to-end regression this tier exists for: a real-shaped
    /// `_arguments` call using the brace-pair idiom for a short+long
    /// flag pair, embedded inside a shell function (as real zsh
    /// completions always are), must recover both spellings sharing one
    /// description.
    #[test]
    fn extracts_flags_from_a_realistic_zsh_completion_function() {
        let script = r#"
#compdef mytool
_mytool() {
    _arguments \
        '(-v --verbose)'{-v,--verbose}'[enable verbose output]' \
        '--format=[output format]:format:(json yaml table)' \
        '(-h --help)'{-h,--help}'[show help message]' \
        '*::args:_normal'
}
_mytool "$@"
"#;
        let flags = extract_zsh_flags(script, &prov());
        let by_long: std::collections::HashMap<&str, &Flag> = flags
            .iter()
            .filter_map(|f| f.long.as_deref().map(|l| (l, f)))
            .collect();
        assert!(by_long.contains_key("verbose"), "{flags:?}");
        assert!(by_long.contains_key("format"), "{flags:?}");
        assert!(by_long.contains_key("help"), "{flags:?}");
        assert_eq!(
            by_long["verbose"].description.as_ref().unwrap().as_str(),
            "enable verbose output"
        );
        let shorts: Vec<char> = flags.iter().filter_map(|f| f.short).collect();
        assert!(shorts.contains(&'v'));
        assert!(shorts.contains(&'h'));
    }

    #[test]
    fn extracts_bare_spellings_from_bash_complete_dash_w() {
        let script = r#"complete -W "--verbose --help -v -h" mytool"#;
        let flags = extract_bash_flags(script, &prov());
        assert_eq!(flags.len(), 4);
        assert!(flags.iter().all(|f| f.description.is_none()));
        assert!(flags.iter().any(|f| f.long.as_deref() == Some("verbose")));
        assert!(flags.iter().any(|f| f.short == Some('v')));
    }

    #[test]
    fn no_arguments_calls_yields_no_flags() {
        let script = "echo hello\nfoo() { bar; }\n";
        assert!(extract_zsh_flags(script, &prov()).is_empty());
    }

    #[test]
    fn malformed_script_does_not_panic() {
        let script = "if [[ ( ; then\n";
        let flags = extract_zsh_flags(script, &prov());
        assert!(flags.is_empty());
    }

    #[test]
    fn detect_false_for_a_tool_with_no_completion_subcommand() {
        let tier = CompletionScriptTier::default();
        let tool = ResolvedTool {
            name: "sh".to_string(),
            path: Some(Path::new("/bin/sh").to_path_buf()),
            version: None,
        };
        assert!(!tier.detect(&tool));
    }

    #[test]
    fn extract_node_declines_non_root_paths() {
        let tier = CompletionScriptTier::default();
        let tool = ResolvedTool {
            name: "mytool".to_string(),
            path: Some(Path::new("/bin/sh").to_path_buf()),
            version: None,
        };
        let result = tier.extract_node(&tool, &["mytool".to_string(), "sub".to_string()]);
        assert!(matches!(result, Err(ExtractError::PathNotFound)));
    }

    // --- the replay seam: real-argv tests against a `Transcript` ---

    fn exec_output(stdout: &str) -> crate::exec::ExecOutput {
        crate::exec::ExecOutput {
            stdout: stdout.as_bytes().to_vec(),
            stderr: Vec::new(),
            exit_code: Some(0),
            timed_out: false,
        }
    }

    /// Real argv, replayed: the root probe tries zsh first, which renders
    /// to exactly `["completion", "zsh"]` (`InertArgv::args`). A transcript
    /// keyed on that argv, holding a real-shaped `_arguments` script, must
    /// let `extract_node` recover flags through the tier's actual probe
    /// construction — not by handing `extract_zsh_flags` the script text
    /// directly, which is what the parser-level tests above already cover.
    #[test]
    fn extract_node_replays_flags_from_a_transcript_keyed_on_the_real_argv() {
        let script = r#"
#compdef mytool
_mytool() {
    _arguments \
        '(-v --verbose)'{-v,--verbose}'[enable verbose output]'
}
_mytool "$@"
"#;
        let transcript = crate::exec::Transcript::new([(
            vec!["completion".to_string(), "zsh".to_string()],
            exec_output(script),
        )]);
        let tier = CompletionScriptTier::new(std::sync::Arc::new(transcript));
        let tool = ResolvedTool {
            name: "mytool".to_string(),
            path: Some(std::path::PathBuf::from("/replayed/mytool")),
            version: None,
        };
        let node = tier
            .extract_node(&tool, &["mytool".to_string()])
            .expect("the transcript covers the exact `completion zsh` argv this tier sends");
        assert!(node
            .flags
            .iter()
            .any(|f| f.long.as_deref() == Some("verbose")));
    }

    /// The negative case: a transcript that covers neither `completion
    /// zsh` nor `completion bash` — the only two argvs this tier ever
    /// sends — must not produce a fabricated, silently-empty node. It must
    /// come back as an explicit error.
    #[test]
    fn extract_node_against_a_transcript_missing_both_shell_argvs_is_an_error_not_empty_success() {
        let transcript = crate::exec::Transcript::new([(
            vec!["completion".to_string(), "fish".to_string()],
            exec_output("this key can never be requested by this tier"),
        )]);
        let tier = CompletionScriptTier::new(std::sync::Arc::new(transcript));
        let tool = ResolvedTool {
            name: "mytool".to_string(),
            path: Some(std::path::PathBuf::from("/replayed/mytool")),
            version: None,
        };
        let result = tier.extract_node(&tool, &["mytool".to_string()]);
        assert!(
            matches!(result, Err(ExtractError::Other(_))),
            "expected an explicit error, got {result:?}"
        );
    }
}
