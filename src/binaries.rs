//! Version pinning for the binaries the harness spawns.
//!
//! "A third party can clone, build, fund and reproduce this" is only true if the binaries a
//! run was taken against are identifiable. A committed 46 MB blob is not a guard: it says
//! nothing about which revision produced it and it cannot be rebuilt from the repository. A
//! pinned revision plus an assertion at start-up is — `tools/fetch.sh` builds each binary at
//! its pinned commit, and this module refuses to run when what is on disk does not report the
//! version the profile is about to claim.
//!
//! Every version observed is recorded in [`crate::metrics::Versions`], including the ones
//! that could only be read after the process was spawned (the payment processor answers
//! `GET /health/version` and takes no `--version` flag at all).

use std::{
    path::{Path, PathBuf},
    process::Command,
};

use anyhow::{anyhow, Context, Result};
use log::warn;
use serde::{Deserialize, Serialize};

/// What one binary reported when it was asked for its version.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BinaryVersion {
    /// Stable label, e.g. `minotari_console_wallet`.
    pub label: String,
    /// Where the binary was found, or how it was asked (`GET /health/version`).
    pub source: String,
    /// The version string it reported, verbatim.
    pub reported: String,
    /// The version string the configuration required, or empty when the pin is not enforced.
    pub expected: String,
    /// False when `expected` was empty, so the run is not reproducible from this binary alone.
    pub pinned: bool,
}

/// A binary the harness spawns, and the version it has to report.
#[derive(Debug, Clone)]
pub struct PinnedBinary {
    pub label: &'static str,
    pub path: PathBuf,
    /// Arguments that make it print its version.
    pub version_args: &'static [&'static str],
    /// Required version string. Empty means the operator has switched the pin off.
    pub expected: String,
}

impl PinnedBinary {
    pub fn new(label: &'static str, path: impl Into<PathBuf>, expected: &str) -> Self {
        Self {
            label,
            path: path.into(),
            version_args: &["--version"],
            expected: expected.to_string(),
        }
    }

    /// Runs the binary, reads its version and checks it against the pin.
    pub fn verify(&self) -> Result<BinaryVersion> {
        let reported = probe_version(&self.path, self.version_args).with_context(|| {
            format!(
                "Could not read the version of `{}` ({}). Build it with `tools/fetch.sh` and \
                 point the configuration at it.",
                self.label,
                self.path.display()
            )
        })?;
        check(
            self.label,
            &self.path.display().to_string(),
            &reported,
            &self.expected,
        )
    }
}

/// Runs `path <args>` and returns the first non-empty line it printed.
///
/// Both streams are read: `--version` goes to stdout for every binary here, but a binary that
/// fails to start says why on stderr, and that message is far more useful in the error than
/// "no output".
pub fn probe_version(path: &Path, args: &[&str]) -> Result<String> {
    let output = Command::new(path)
        .args(args)
        .output()
        .map_err(|e| anyhow!("Could not run `{} {}`: {e}", path.display(), args.join(" ")))?;
    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();
    let line = first_line(&stdout).or_else(|| first_line(&stderr));
    match line {
        Some(line) => Ok(line),
        None => Err(anyhow!(
            "`{} {}` exited with {} and printed nothing",
            path.display(),
            args.join(" "),
            output.status
        )),
    }
}

/// First line with anything on it, trimmed.
fn first_line(text: &str) -> Option<String> {
    text.lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .map(ToString::to_string)
}

/// Checks a reported version against a pin.
///
/// The match is a substring test, not equality: every one of these binaries prints its name
/// alongside the version (`minotari_console_wallet 6.2.0-pre.1`), and two of them disagree
/// with themselves about what that name is — `minotari --version` prints `tari 0.3.0` because
/// the clap command is named `tari`, while its own `GET /version` reports the name
/// `minotari`. Pinning the version and ignoring the name is the part that is stable.
///
/// An empty pin is accepted with a warning rather than refused. Turning the assertion off is
/// a legitimate thing to do while developing against an unreleased build; producing a profile
/// that silently claims a pin it never checked is not, so the profile records `pinned: false`
/// against that binary.
pub fn check(label: &str, source: &str, reported: &str, expected: &str) -> Result<BinaryVersion> {
    let expected = expected.trim();
    let reported = reported.trim();
    if expected.is_empty() {
        warn!(
            "No version pin is configured for `{label}` ({source}, reporting `{reported}`). The \
             profile will record that this binary was not pinned."
        );
        return Ok(BinaryVersion {
            label: label.to_string(),
            source: source.to_string(),
            reported: reported.to_string(),
            expected: String::new(),
            pinned: false,
        });
    }
    if !reported.contains(expected) {
        return Err(anyhow!(
            "`{label}` at {source} reports `{reported}`, but this harness is pinned to \
             `{expected}`. Rebuild it with `tools/fetch.sh`, or change the pin in the \
             configuration's [pins] section and accept that the profile describes a different \
             binary."
        ));
    }
    Ok(BinaryVersion {
        label: label.to_string(),
        source: source.to_string(),
        reported: reported.to_string(),
        expected: expected.to_string(),
        pinned: true,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_matching_version_passes_and_is_recorded_verbatim() {
        let observed = check(
            "minotari_console_wallet",
            "tools/minotari_console_wallet",
            "minotari_console_wallet 6.2.0-pre.1",
            "6.2.0-pre.1",
        )
        .unwrap();
        assert!(observed.pinned);
        assert_eq!(observed.reported, "minotari_console_wallet 6.2.0-pre.1");
        assert_eq!(observed.expected, "6.2.0-pre.1");
    }

    #[test]
    fn the_binarys_own_name_is_not_part_of_the_pin() {
        // `minotari --version` prints `tari 0.3.0`: the clap command is named `tari` even
        // though the executable and the crate are named `minotari`. Pinning on the name would
        // reject the right binary.
        assert!(check("minotari", "tools/minotari", "tari 0.3.0", "0.3.0").is_ok());
    }

    #[test]
    fn a_mismatched_version_refuses_the_run_and_says_both_versions() {
        let err = check("minotari", "tools/minotari", "tari 0.2.0", "0.3.0")
            .unwrap_err()
            .to_string();
        assert!(err.contains("tari 0.2.0"), "unexpected error: {err}");
        assert!(err.contains("0.3.0"), "unexpected error: {err}");
        assert!(err.contains("tools/fetch.sh"), "unexpected error: {err}");
    }

    #[test]
    fn an_unpinned_binary_is_allowed_but_the_profile_says_so() {
        let observed = check("minotari", "tools/minotari", "tari 0.9.0-dev", "  ").unwrap();
        assert!(
            !observed.pinned,
            "a profile may not claim a pin that was never checked"
        );
        assert_eq!(observed.reported, "tari 0.9.0-dev");
        assert!(observed.expected.is_empty());
    }

    #[test]
    fn a_prefix_of_the_version_is_not_a_match_in_the_wrong_direction() {
        // `6.2.0-pre.1` must not be satisfied by a binary reporting `6.2.0`.
        assert!(check("w", "p", "w 6.2.0", "6.2.0-pre.1").is_err());
        // The other way round is a match, and deliberately so: pinning `6.2.0` accepts every
        // pre-release of it, which is what an operator asking for that pin means.
        assert!(check("w", "p", "w 6.2.0-pre.1", "6.2.0").is_ok());
    }

    #[test]
    fn a_binary_that_cannot_be_run_says_which_one() {
        let binary = PinnedBinary::new("missing", "/nonexistent/minotari", "0.3.0");
        let err = binary.verify().unwrap_err();
        let text = format!("{err:#}");
        assert!(text.contains("missing"), "unexpected error: {text}");
        assert!(text.contains("tools/fetch.sh"), "unexpected error: {text}");
    }

    #[test]
    fn the_first_non_empty_line_is_the_version() {
        assert_eq!(
            first_line("\n\n  tari 0.3.0  \nmore\n").unwrap(),
            "tari 0.3.0"
        );
        assert_eq!(first_line("   \n  \n"), None);
    }

    #[test]
    fn a_real_binary_can_be_probed() {
        // `echo` is not a wallet, but it proves the plumbing reads a child's stdout rather
        // than guessing.
        let version = probe_version(Path::new("/bin/echo"), &["harness 1.2.3"]).unwrap();
        assert_eq!(version, "harness 1.2.3");
    }
}
