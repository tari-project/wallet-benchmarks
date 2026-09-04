//! Filesystem permissions for the files the harness writes that hold key material.
//!
//! A run has to be reproducible by hand, so the things that make a wallet spend end up on
//! disk: the seed book, the payment processor's `.env` with the offline signer's keystore
//! passphrase and the benchmark account's view key, and the working directory that holds both
//! databases. The default mode a file is created with lets every other local user on the host
//! read all of it, so each one is narrowed to its owner the moment it is written.
//!
//! On a platform without POSIX mode bits there is nothing equivalent to do, and the calls are
//! no-ops rather than errors: refusing to run there would be a harness that cannot benchmark
//! a wallet because of a file permission it never needed.

use std::path::Path;

#[cfg(unix)]
use anyhow::Context;
use anyhow::Result;

/// Narrows a just-written file to its owner.
pub fn restrict_file(path: &Path) -> Result<()> {
    set_mode(path, 0o600)
}

/// Narrows a directory to its owner, so its contents cannot be listed or opened either.
///
/// Applied to a directory the harness owns outright — a wallet's working directory — and not
/// to anything shared: the executable bit goes with it, which is what makes the directory
/// traversable at all.
pub fn restrict_dir(path: &Path) -> Result<()> {
    set_mode(path, 0o700)
}

#[cfg(unix)]
fn set_mode(path: &Path, mode: u32) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode))
        .with_context(|| format!("Could not restrict permissions on {}", path.display()))
}

#[cfg(not(unix))]
fn set_mode(_path: &Path, _mode: u32) -> Result<()> {
    Ok(())
}

#[cfg(all(test, unix))]
mod tests {
    use std::os::unix::fs::PermissionsExt;

    use super::*;

    fn mode_of(path: &Path) -> u32 {
        std::fs::metadata(path).unwrap().permissions().mode() & 0o777
    }

    #[test]
    fn a_file_holding_secrets_is_readable_only_by_its_owner() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("secret");
        std::fs::write(&path, "passphrase").unwrap();
        // Whatever the umask left behind, group and other must end up with nothing.
        restrict_file(&path).unwrap();
        assert_eq!(mode_of(&path), 0o600);
    }

    #[test]
    fn a_directory_keeps_the_bit_that_makes_it_traversable() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("work");
        std::fs::create_dir(&path).unwrap();
        restrict_dir(&path).unwrap();
        assert_eq!(mode_of(&path), 0o700);
        // 0o600 on a directory would lock the harness out of its own files.
        std::fs::write(path.join("inside"), "x").unwrap();
    }

    #[test]
    fn a_missing_path_says_which_one() {
        let dir = tempfile::tempdir().unwrap();
        let missing = dir.path().join("not-here");
        let err = restrict_file(&missing).unwrap_err().to_string();
        assert!(err.contains("not-here"), "unexpected error: {err}");
    }
}
