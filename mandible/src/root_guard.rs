//! Refuses to run as root before any probe spawns (spec §6 rule 10).
//! Checked once in `main`, before any tool resolves.

use nix::unistd::Uid;

/// One-line refusal `main` prints and exits on; names `--allow-root`.
pub const REFUSAL: &str = "mandible refuses to run as root (uid 0); pass --allow-root to proceed.";

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
