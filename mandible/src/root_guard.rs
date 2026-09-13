//! Refuses to run as root before any probe spawns (spec §6 rule 10).
//!
//! Checked once in `main`, before the `--doctor`/`--report`/`--review`
//! branches and before the TUI starts, so no probe reaches the exec
//! chokepoint under uid 0 unless `--allow-root` is given.

use nix::unistd::Uid;

/// The one line `main` prints and exits on.
pub const REFUSAL: &str = "mandible refuses to run as root (uid 0); pass --allow-root to proceed.";

/// `Some(REFUSAL)` when `uid` is root and `allow_root` was not given.
pub fn refusal(uid: Uid, allow_root: bool) -> Option<&'static str> {
    if uid.is_root() && !allow_root {
        Some(REFUSAL)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn root_without_the_flag_is_refused() {
        assert_eq!(refusal(Uid::from_raw(0), false), Some(REFUSAL));
    }

    #[test]
    fn root_with_the_flag_proceeds() {
        assert_eq!(refusal(Uid::from_raw(0), true), None);
    }

    #[test]
    fn a_normal_uid_proceeds_without_the_flag() {
        assert_eq!(refusal(Uid::from_raw(1000), false), None);
        assert_eq!(refusal(Uid::from_raw(1000), true), None);
    }
}
