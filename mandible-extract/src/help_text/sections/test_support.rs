//! Fixtures and helpers shared by more than one of this module's test
//! suites. The `include_str!` captures live once, as the corpus
//! regression fixtures, rather than a byte-identical second copy.

use super::*;

pub(super) fn flag_named(parsed: &ParsedHelp, long: &str) -> Entity {
    parsed
        .flags
        .iter()
        .find(|f| f.long() == Some(long))
        .unwrap_or_else(|| {
            panic!(
                "no flag long=={long:?} in {:?}",
                parsed
                    .flags
                    .iter()
                    .map(|f| f.spelling())
                    .collect::<Vec<_>>()
            )
        })
        .clone()
}

// These two captures live once, as the corpus regression fixtures
// (`corpus/tar/1.35/help.txt`, `corpus/git/2.43.0/help.txt` — see
// corpus/README.md), rather than a byte-identical second copy under
// this crate's own `tests/fixtures/`.
pub(super) const TAR_HELP: &str = include_str!("../../../../corpus/tar/1.35/help.txt");
pub(super) const GIT_HELP: &str = include_str!("../../../../corpus/git/2.43.0/help.txt");
pub(super) const LSOF_HELP: &str = include_str!("../../../../corpus/lsof/4.95.0/help.stderr.txt");
pub(super) const UNZIP_HELP: &str = include_str!("../../../../corpus/unzip/6.00/help.txt");
pub(super) const ZOXIDE_HELP: &str = include_str!("../../../../corpus/zoxide/0.9.9/help.txt");
pub(super) const JMOD_HELP: &str = include_str!("../../../../corpus/jmod/17.0.20/help.txt");
pub(super) const LLVM_AR_HELP: &str = include_str!("../../../../corpus/llvm-ar-18/18.1.3/help.txt");

pub(super) const OPENSSL_HELP: &str =
    include_str!("../../../tests/fixtures/help_text/openssl_help.stderr");
pub(super) const IP_HELP: &str = include_str!("../../../tests/fixtures/help_text/ip_help.stderr");
pub(super) const DD_HELP: &str = include_str!("../../../tests/fixtures/help_text/dd_help.stdout");
pub(super) const LESS_HELP: &str =
    include_str!("../../../tests/fixtures/help_text/less_help.stdout");
pub(super) const SED_HELP: &str = include_str!("../../../tests/fixtures/help_text/sed_help.stdout");
pub(super) const FIND_HELP: &str =
    include_str!("../../../tests/fixtures/help_text/find_help.stdout");
pub(super) const CURL_HELP: &str =
    include_str!("../../../tests/fixtures/help_text/curl_help.stdout");
pub(super) const APT_GET_HELP: &str =
    include_str!("../../../tests/fixtures/help_text/apt_get_help.stdout");
pub(super) const SIZE_HELP: &str =
    include_str!("../../../tests/fixtures/help_text/size_help.stdout");
