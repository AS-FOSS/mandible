//! Tier C: completion script structural parsing (spec §7 Tier C).
//!
//! Generates `<tool> completion zsh` (falling back to `bash`), never
//! executes the result, and walks it as a real shell AST via
//! `brush-parser` — not `conch-parser` (unmaintained [M-9]) and not
//! `yash-syntax` (GPLv3).
//!
//! **zsh first.** A zsh completion function's `_arguments` calls carry a
//! flag's spelling and description in one structure, e.g. the brace-pair
//! idiom `'(-v --verbose)'{-v,--verbose}'[enable verbose output]'`.
//! `brush_parser` keeps a token's raw source text rather than performing
//! shell expansion, so this module does its own small, targeted expansion
//! of that idiom before a hand-written spec-string grammar. Value
//! messages, actions, and nested exclusion lists past the first are read
//! past, not interpreted — the payload here is spelling + description,
//! not a completion engine.
//!
//! **bash as a fallback, spellings only.** Bash completion functions
//! mostly compute candidates at runtime rather than declaring them
//! statically, so this tier only recognizes `complete -W "word1 word2
//! ..." <tool>` and recovers bare spellings with no descriptions.
//!
//! **Root-only contribution.** Attributing a nested `_arguments` call to
//! the right subcommand path would require simulating the script's
//! dispatch logic, not just parsing it. Rather than guess, this tier is
//! not incremental (spec §5.2): it contributes once, to the root node
//! only.
//!
//! **Authority** (spec §4.4): structural 150, prose 30 — existence of a
//! flag is fairly trustworthy, but its prose is usually terser than real
//! `--help`.
//!
//! **Gated on prior evidence, never speculative.** This tier used to send
//! `completion zsh`/`bash` to every tool speculatively; measured leaving
//! 437 daemons running, tripping the sweep's PTY canary (`docker-proxy`
//! binding `0.0.0.0:-1`), and costing two-thirds of extraction time on
//! tools with no such subcommand (spec §6 rule 4, [M-9]).
//!
//! [`names_a_completion_subcommand`] is the gate: asks the tool for its
//! own root `--help` (a shape Tier B already sends every tool, so no new
//! argv or exposure) and only constructs a `completion <shell>` argv when
//! the tool's own text names `completion`/`completions` as a command.
//! Evidence read from the tool's own output, never from its name (spec
//! §1); the gate can only stop a probe, never authorise one not already
//! permitted.

use crate::errors::ExtractError;
use crate::exec::{InertArgv, LiveProbe, Probe};
use crate::resolve::ResolvedTool;
use crate::tier::{ExtractionTier, NodeHints};
use brush_parser::ast;
use mandible_core::{Authority, CommandNode, Entity, Provenance, Source, Text, ValueKind};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// Wall-clock cap for a `detect`/`extract_node` probe (spec §6 rule 4).
/// Generating a completion script is a single spawn with no further
/// per-node cost (this tier isn't incremental), so both use the more
/// generous extract cap rather than detect's tighter one.
const PROBE_TIMEOUT: Duration = Duration::from_secs(10);

/// Wall-clock cap for the *evidence* probe — the tool's own root `--help`
/// (spec §6 rule 4's `detect` cap, not the generous `extract_node` one).
///
/// Tighter than [`PROBE_TIMEOUT`] on purpose, and safe to be: this probe
/// asks a question rather than collecting a payload, so a tool slow enough
/// to blow through it loses nothing but a tier that was speculative to
/// begin with. It is also the one shape Tier B already sends to every tool
/// unconditionally, so a tool that would hang here is already hanging in
/// Tier B and this changes nothing about that.
const EVIDENCE_TIMEOUT: Duration = Duration::from_secs(2);

/// Tier C: parses a generated zsh (preferred) or bash completion script
/// for flag spellings and descriptions.
pub struct CompletionScriptTier {
    /// The source of a `completion <shell>` probe's output — [`LiveProbe`]
    /// in production ([`Self::default`]), or a [`crate::exec::Transcript`]
    /// to replay frozen bytes with zero subprocesses.
    probe: Arc<dyn Probe>,
    /// Whether each binary offers a `completion` subcommand at all, keyed
    /// by resolved path and answered once. Memoized since it's a property
    /// of the binary, not of a node or moment. Deliberately does not
    /// memoize the completion script itself — a refresh (`r`) must
    /// re-fetch it.
    offers_completion: Mutex<HashMap<PathBuf, bool>>,
}

impl Default for CompletionScriptTier {
    fn default() -> Self {
        Self::new(Arc::new(LiveProbe))
    }
}

impl CompletionScriptTier {
    /// Build this tier against an explicit probe.
    pub fn new(probe: Arc<dyn Probe>) -> Self {
        Self {
            probe,
            offers_completion: Mutex::new(HashMap::new()),
        }
    }

    /// The gate (see this module's doc comment): is there evidence, from
    /// the tool itself, that a `completion <shell>` argv is a subcommand
    /// invocation?
    ///
    /// Two independent sources, both about the binary, neither about its
    /// name: the tool's own root `--help` names it (the common case), or
    /// Tier A′ identified the binary as cobra (which registers `completion`
    /// itself from v1.2 on, possibly hidden from help text). Reading the
    /// artifact marker is free — a memoized file read, no subprocess.
    fn offers_completion_subcommand(&self, tool: &ResolvedTool, tool_path: &Path) -> bool {
        if let Ok(memo) = self.offers_completion.lock() {
            if let Some(hit) = memo.get(tool_path) {
                return *hit;
            }
        }

        let answer = crate::framework::identify_from_artifact(tool)
            == Some(crate::framework::Framework::Cobra)
            || self.help_text_names_a_completion_subcommand(tool_path);

        if let Ok(mut memo) = self.offers_completion.lock() {
            memo.insert(tool_path.to_path_buf(), answer);
        }
        answer
    }

    /// Ask the tool for its own root `--help` and look for a `completion`
    /// command in it.
    ///
    /// Both streams are searched, unlike the parsing path
    /// (`help_text::pick_stream`): here the answer is a yes/no about a
    /// word's presence, not a document to parse, so a tool printing its
    /// command table to stderr should not lose a tier over it.
    fn help_text_names_a_completion_subcommand(&self, tool_path: &Path) -> bool {
        let Ok(out) = self
            .probe
            .run(tool_path, &InertArgv::HelpLong, EVIDENCE_TIMEOUT)
        else {
            return false;
        };
        names_a_completion_subcommand(&String::from_utf8_lossy(&out.stdout))
            || names_a_completion_subcommand(&String::from_utf8_lossy(&out.stderr))
    }
}

/// True if `help` contains a line that *begins* with `completion` or
/// `completions` as a whole word — the shape of a command-table row.
///
/// Anchored to the first token deliberately, and this is the whole
/// grammar:
///
/// - `  completion   Generate the autocompletion script` — a command. ✓
/// - `      --completion <shell>` — a flag, token starts with a dash. ✗
/// - `Generate the shell completion script` — prose about a flag
///   elsewhere in the document. ✗
/// - `Usage: tool completion <shell>` — the word is real but not first. ✗
///
/// A trailing `,` or `:` is stripped so an aliased row still matches. The
/// plural is accepted as the other common spelling.
///
/// Deliberately narrow: a tool whose completion command exists but is
/// named something else, or is hidden and not cobra, loses this tier and
/// keeps every other (spec §7's degradation ladder).
fn names_a_completion_subcommand(help: &str) -> bool {
    help.lines().any(|line| {
        let Some(first) = line.split_whitespace().next() else {
            return false;
        };
        let word = first.trim_end_matches([',', ':']);
        word == "completion" || word == "completions"
    })
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
        // The gate (spec §6 rule 1, this module's doc comment): `completion`
        // and `zsh` are framework-protocol words, not universal ones. A
        // daemon that ignores argv and starts anyway falsifies rule 2's
        // premise that these tools parse argv. No evidence, no argv.
        if !self.offers_completion_subcommand(tool, tool_path) {
            return false;
        }
        probe_and_extract_flags(self.probe.as_ref(), tool_path).is_some()
    }

    fn extract_node(
        &self,
        tool: &ResolvedTool,
        path: &[String],
        _hints: NodeHints,
    ) -> Result<CommandNode, ExtractError> {
        let tool_path = tool.path.as_ref().ok_or(ExtractError::ToolNotFound)?;
        // Root-only contribution (see module doc). `is_incremental() ==
        // false` already keeps the lazy-fill path from calling this for a
        // deeper node; this guard is cheap insurance regardless.
        if path.len() > 1 {
            return Err(ExtractError::PathNotFound);
        }
        // Re-checked here, not left to `detect()` having run first: the
        // gate is a safety property and must hold for any caller reaching
        // this method by any route.
        if !self.offers_completion_subcommand(tool, tool_path) {
            return Err(ExtractError::Other(
                "no evidence of a `completion` subcommand: refusing to send a completion-protocol \
                 argv to a tool that may not parse it (spec §6 rule 1)"
                    .to_string(),
            ));
        }
        let (shell, flags) = probe_and_extract_flags(self.probe.as_ref(), tool_path)
            .ok_or_else(|| ExtractError::Other("no usable completion script found".to_string()))?;
        let name = path.last().cloned().unwrap_or_else(|| tool.name.clone());
        let mut node =
            CommandNode::new(name, Provenance::single(Source::CompletionScript { shell }));
        node.set_flags(flags);
        Ok(node)
    }

    fn is_incremental(&self) -> bool {
        // A single probe already returns everything this tier will ever
        // know (root-only contribution, see module doc).
        false
    }
}

/// Request zsh first, bash as fallback (spec §7 Tier C); return the shell
/// name used and the flags recovered, or `None` if neither produced a
/// script with anything this tier recognizes.
fn probe_and_extract_flags(probe: &dyn Probe, tool_path: &Path) -> Option<(String, Vec<Entity>)> {
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
/// anywhere in it (regardless of nesting). This tier only looks for
/// specific command names, never simulates control flow, so flattening
/// suffices.
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
/// argument words as zsh `_arguments` spec strings into flag entities.
fn extract_zsh_flags(script: &str, provenance: &Provenance) -> Vec<Entity> {
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
fn extract_bash_flags(script: &str, provenance: &Provenance) -> Vec<Entity> {
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
/// leaving unquoted runs as-is (an unquoted `{...}` brace expression
/// survives, ready for [`expand_brace_alternatives`]). Not general shell
/// quote-removal — `_arguments` spec strings are simple literal
/// single-quoted text in practice.
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
    fn into_flag(self, provenance: Provenance) -> Entity {
        let mut flag = Entity::flag_spelled(self.short, self.long, false, false, provenance);
        flag.value_kind = if self.takes_value {
            ValueKind::Required
        } else {
            ValueKind::None
        };
        flag.description = self.description.as_deref().map(Text::sanitize);
        flag
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

    /// A real-shaped `_arguments` call using the brace-pair idiom for a
    /// short+long flag pair, inside a shell function, must recover both
    /// spellings sharing one description.
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
        let by_long: std::collections::HashMap<&str, &Entity> = flags
            .iter()
            .filter_map(|f| f.long().map(|l| (l, f)))
            .collect();
        assert!(by_long.contains_key("verbose"), "{flags:?}");
        assert!(by_long.contains_key("format"), "{flags:?}");
        assert!(by_long.contains_key("help"), "{flags:?}");
        assert_eq!(
            by_long["verbose"].description.as_ref().unwrap().as_str(),
            "enable verbose output"
        );
        let shorts: Vec<char> = flags.iter().filter_map(|f| f.short()).collect();
        assert!(shorts.contains(&'v'));
        assert!(shorts.contains(&'h'));
    }

    #[test]
    fn extracts_bare_spellings_from_bash_complete_dash_w() {
        let script = r#"complete -W "--verbose --help -v -h" mytool"#;
        let flags = extract_bash_flags(script, &prov());
        assert_eq!(flags.len(), 4);
        assert!(flags.iter().all(|f| f.description.is_none()));
        assert!(flags.iter().any(|f| f.long() == Some("verbose")));
        assert!(flags.iter().any(|f| f.short() == Some('v')));
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
        let result = tier.extract_node(
            &tool,
            &["mytool".to_string(), "sub".to_string()],
            NodeHints {
                heading_attested: true,
            },
        );
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

    /// A root `--help` that names a `completion` command — what the gate
    /// requires before this tier constructs any completion-protocol argv.
    const HELP_NAMING_COMPLETION: &str = "\
Usage: mytool [OPTIONS] <COMMAND>

Commands:
  completion  Generate a completion script for your shell
  help        Print this message
";

    /// Real argv, replayed: the root probe tries zsh first, rendering to
    /// exactly `["completion", "zsh"]`. A transcript keyed on that argv
    /// must let `extract_node` recover flags through the tier's actual
    /// probe construction. Also carries the `["--help"]` recording the
    /// gate needs — a tier that stopped asking, or asked differently,
    /// would miss and fail this test.
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
        let transcript = crate::exec::Transcript::new([
            (
                vec!["--help".to_string()],
                exec_output(HELP_NAMING_COMPLETION),
            ),
            (
                vec!["completion".to_string(), "zsh".to_string()],
                exec_output(script),
            ),
        ]);
        let tier = CompletionScriptTier::new(std::sync::Arc::new(transcript));
        let tool = ResolvedTool {
            name: "mytool".to_string(),
            path: Some(std::path::PathBuf::from("/replayed/mytool")),
            version: None,
        };
        let node = tier
            .extract_node(
                &tool,
                &["mytool".to_string()],
                NodeHints {
                    heading_attested: true,
                },
            )
            .expect("the transcript covers the exact `completion zsh` argv this tier sends");
        assert!(node.flags().any(|f| f.long() == Some("verbose")));
    }

    /// A transcript covering neither `completion zsh` nor `completion
    /// bash` must not produce a fabricated, silently-empty node.
    #[test]
    fn extract_node_against_a_transcript_missing_both_shell_argvs_is_an_error_not_empty_success() {
        let transcript = crate::exec::Transcript::new([
            (
                vec!["--help".to_string()],
                exec_output(HELP_NAMING_COMPLETION),
            ),
            (
                vec!["completion".to_string(), "fish".to_string()],
                exec_output("this key can never be requested by this tier"),
            ),
        ]);
        let tier = CompletionScriptTier::new(std::sync::Arc::new(transcript));
        let tool = ResolvedTool {
            name: "mytool".to_string(),
            path: Some(std::path::PathBuf::from("/replayed/mytool")),
            version: None,
        };
        let result = tier.extract_node(
            &tool,
            &["mytool".to_string()],
            NodeHints {
                heading_attested: true,
            },
        );
        assert!(
            matches!(result, Err(ExtractError::Other(_))),
            "expected an explicit error, got {result:?}"
        );
    }

    // --- the gate: no evidence, no completion-protocol argv ---

    #[test]
    fn a_command_table_row_is_evidence() {
        assert!(names_a_completion_subcommand(HELP_NAMING_COMPLETION));
        // The plural spelling, and an aliased row.
        assert!(names_a_completion_subcommand(
            "  completions  Generate them\n"
        ));
        assert!(names_a_completion_subcommand(
            "  completion, comp   Generate them\n"
        ));
    }

    /// Three places the word appears in a help document without naming a
    /// command; each firing would put a completion-protocol argv in front
    /// of a tool with no such subcommand.
    #[test]
    fn the_word_elsewhere_in_a_document_is_not_evidence() {
        // A flag, not a command.
        assert!(!names_a_completion_subcommand(
            "  --completion <SHELL>   emit a completion script\n"
        ));
        // Prose about a flag elsewhere.
        assert!(!names_a_completion_subcommand(
            "Generate the shell completion script with --emit=completion\n"
        ));
        // A usage synopsis: the word is real, but not the first token.
        assert!(!names_a_completion_subcommand(
            "Usage: mytool completion <shell>\n"
        ));
        assert!(!names_a_completion_subcommand(
            "Usage: blkmapd [-h] [-d level] [-f] [-n]\n"
        ));
    }

    /// A tool whose own `--help` says nothing about a `completion`
    /// command must never be sent `completion zsh`/`bash` — proven by a
    /// transcript that does hold a working `completion zsh` recording, so
    /// only the gate itself stops the tier from succeeding.
    #[test]
    fn a_tool_whose_help_names_no_completion_command_is_never_sent_one() {
        let script = r#"
#compdef mytool
_mytool() {
    _arguments '(-v --verbose)'{-v,--verbose}'[enable verbose output]'
}
"#;
        let transcript = crate::exec::Transcript::new([
            (
                vec!["--help".to_string()],
                // A real tool's shape: flags only, no command table.
                exec_output("Usage: mytool [-h] [-d level] [-f]\n  -h  help\n"),
            ),
            (
                vec!["completion".to_string(), "zsh".to_string()],
                exec_output(script),
            ),
        ]);
        let tier = CompletionScriptTier::new(std::sync::Arc::new(transcript));
        let tool = ResolvedTool {
            name: "mytool".to_string(),
            path: Some(std::path::PathBuf::from("/replayed/mytool")),
            version: None,
        };

        assert!(
            !tier.detect(&tool),
            "no evidence of a `completion` command, so the tier must decline before \
             constructing any completion-protocol argv — even though the transcript \
             would have answered one"
        );
        let result = tier.extract_node(
            &tool,
            &["mytool".to_string()],
            NodeHints {
                heading_attested: true,
            },
        );
        assert!(
            matches!(result, Err(ExtractError::Other(_))),
            "the gate must hold at `extract_node` too, for any caller that reaches it \
             by another route, got {result:?}"
        );
    }

    /// A tool that does advertise the command still gets probed and still
    /// yields its flags — a gate that silently refused everything would
    /// look identical on the safety test above.
    #[test]
    fn a_tool_whose_help_names_the_command_is_still_probed_and_still_yields_flags() {
        let script = r#"
#compdef mytool
_mytool() {
    _arguments '(-v --verbose)'{-v,--verbose}'[enable verbose output]'
}
"#;
        let transcript = crate::exec::Transcript::new([
            (
                vec!["--help".to_string()],
                exec_output(HELP_NAMING_COMPLETION),
            ),
            (
                vec!["completion".to_string(), "zsh".to_string()],
                exec_output(script),
            ),
        ]);
        let tier = CompletionScriptTier::new(std::sync::Arc::new(transcript));
        let tool = ResolvedTool {
            name: "mytool".to_string(),
            path: Some(std::path::PathBuf::from("/replayed/mytool")),
            version: None,
        };
        assert!(tier.detect(&tool));
        let node = tier
            .extract_node(
                &tool,
                &["mytool".to_string()],
                NodeHints {
                    heading_attested: true,
                },
            )
            .expect("evidence is present, so the tier must extract as it always did");
        assert!(node.flags().any(|f| f.long() == Some("verbose")));
    }
}
