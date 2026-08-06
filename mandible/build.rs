//! Generates shell completion scripts (bash, zsh, fish, PowerShell, Elvish)
//! into `OUT_DIR` at build time, from the same `clap` command definition
//! the binary itself parses against — so completions can never drift from
//! the real CLI surface the way a hand-maintained completion script would.
//!
//! `include!`s `src/cli.rs` directly rather than depending on a `mandible`
//! library target (this crate is binary-only, spec §8): a build script is
//! its own separate compilation and has no other way to reach the `Cli`
//! type. This is the same pattern ripgrep and fd use for the same reason.
//!
//! Packaging (`[package.metadata.deb]`/`[package.metadata.generate-rpm]`
//! in `Cargo.toml`, spec §15) references the generated files via a glob
//! into `target/<profile>/build/mandible-*/out/`, since the hashed
//! directory component isn't known ahead of time — the same convention
//! those tools use.

use clap::{CommandFactory, ValueEnum};
use clap_complete::{generate_to, Shell};
use std::env;

include!("src/cli.rs");

fn main() {
    println!("cargo:rerun-if-changed=src/cli.rs");

    let Some(out_dir) = env::var_os("OUT_DIR") else {
        // Not building under cargo (e.g. an IDE's standalone rustc
        // invocation) — completions are a packaging nicety, not something
        // worth failing the build over.
        return;
    };

    let mut cmd = Cli::command();
    let bin_name = "mandible";
    for shell in Shell::value_variants() {
        // Best-effort: a completion-generation failure must never break
        // an ordinary `cargo build`.
        let _ = generate_to(*shell, &mut cmd, bin_name, &out_dir);
    }
}
