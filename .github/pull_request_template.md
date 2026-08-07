<!--
Before opening: does this change add per-tool knowledge? If it makes one
named tool render better without improving a whole class, it will be asked
to become a framework-level fix instead. See CONTRIBUTING.md.
-->

**What this changes**

**Why**

**How it was verified**
<!--
Green tests are necessary, not sufficient — a tier once shipped completely
dead because its tests mocked the subprocess instead of running it
(AGENTS.md §3.1). If this touches extraction, say what real tool output it
was checked against. If it touches the TUI, say what you actually looked
at; scripts/pty_screenshot.py renders real frames headlessly.
-->

- [ ] `cargo fmt --all -- --check`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo test --workspace`
- [ ] `cargo xtask coverage --check --out coverage-scoreboard.ci.txt --tools git,curl,tar,ip,openssl,sed,find,less,dd,gzip,docker,gh` (if extraction changed)
- [ ] No tool-name-keyed logic added
