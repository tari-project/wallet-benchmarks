use std::{fs, path::Path, process::Command};

use sha2::{Digest, Sha256};

const PATCHES: &[(&str, &str)] = &[
    (
        "patches/minotari-wallet-password-env.patch",
        "53c7e54c2ef211547196a6cbcc0adb0ce6715390da5188afc3a0150888b9ca7b",
    ),
    (
        "patches/payment-processor-fee-rate.patch",
        "69c3001b4474d478822651810dc5f25cae5c8bfede2f9bc756de6ded37dc89fe",
    ),
];

const GENERATED_PROVENANCE: &str = r#"
pub(crate) const EXPECTED_SOURCES: &[ExpectedSource] = &[
    ExpectedSource {
        name: "minotari_cli",
        repository: "https://github.com/tari-project/minotari-cli.git",
        upstream_revision: "360c4848a54d65fd710266233cc9277b0f785e74",
        upstream_commit: "360c4848a54d65fd710266233cc9277b0f785e74",
        upstream_tree: "e9bbd1fb7b538e213e17c2986b85940435adce26",
        patches: &[ExpectedPatch {
            path: "patches/minotari-wallet-password-env.patch",
            sha256: "53c7e54c2ef211547196a6cbcc0adb0ce6715390da5188afc3a0150888b9ca7b",
            result_tree: "8ffaba109f7d413a68852a98939ec2a002105cc6",
        }],
        complete_diff_sha256: "08b5ff524839091db7c110708fac4538261b94e5c3cdda6961ecdbc42c9c0bcf",
        result_tree: "8ffaba109f7d413a68852a98939ec2a002105cc6",
    },
    ExpectedSource {
        name: "tari_console_wallet",
        repository: "https://github.com/tari-project/tari.git",
        upstream_revision: "9f5adb7183dc2ec285f5c8fae05f4be9735d9749",
        upstream_commit: "9f5adb7183dc2ec285f5c8fae05f4be9735d9749",
        upstream_tree: "be2020d2eb904507fa20442448ef76b6e8f0d502",
        patches: &[],
        complete_diff_sha256: "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855",
        result_tree: "be2020d2eb904507fa20442448ef76b6e8f0d502",
    },
    ExpectedSource {
        name: "minotari_node",
        repository: "https://github.com/tari-project/tari.git",
        upstream_revision: "v5.5.0",
        upstream_commit: "b32dba96aa0d4ffdd715a0bb724f69c567d47d9f",
        upstream_tree: "449b76bc4121400265927561a661c5c381d55bcd",
        patches: &[],
        complete_diff_sha256: "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855",
        result_tree: "449b76bc4121400265927561a661c5c381d55bcd",
    },
    ExpectedSource {
        name: "payment_processor",
        repository: "https://github.com/tari-project/minotari_payment_processor.git",
        upstream_revision: "f0572c98cbfac7377412dc6d4094c7d7dfc5de2c",
        upstream_commit: "f0572c98cbfac7377412dc6d4094c7d7dfc5de2c",
        upstream_tree: "add06a544f950f724caa13b972cfc13e5d666c90",
        patches: &[ExpectedPatch {
            path: "patches/payment-processor-fee-rate.patch",
            sha256: "69c3001b4474d478822651810dc5f25cae5c8bfede2f9bc756de6ded37dc89fe",
            result_tree: "8f15669442f3da67fc4636de00b80c666d890c5c",
        }],
        complete_diff_sha256: "8b467bf65003de81ea752092ea3b4f2914e28b284590425d155fda4ad13287d8",
        result_tree: "8f15669442f3da67fc4636de00b80c666d890c5c",
    },
];

pub(crate) const EXPECTED_ARTIFACTS: &[ExpectedArtifact] = &[
    ExpectedArtifact {
        name: "minotari",
        source: "minotari_cli",
        source_revision: "360c4848a54d65fd710266233cc9277b0f785e74",
        source_tree: "8ffaba109f7d413a68852a98939ec2a002105cc6",
    },
    ExpectedArtifact {
        name: "minotari_console_wallet",
        source: "tari_console_wallet",
        source_revision: "9f5adb7183dc2ec285f5c8fae05f4be9735d9749",
        source_tree: "be2020d2eb904507fa20442448ef76b6e8f0d502",
    },
    ExpectedArtifact {
        name: "minotari_node",
        source: "minotari_node",
        source_revision: "v5.5.0",
        source_tree: "449b76bc4121400265927561a661c5c381d55bcd",
    },
    ExpectedArtifact {
        name: "minotari_payment_processor",
        source: "payment_processor",
        source_revision: "f0572c98cbfac7377412dc6d4094c7d7dfc5de2c",
        source_tree: "8f15669442f3da67fc4636de00b80c666d890c5c",
    },
];
"#;

fn main() {
    track_git_revision_inputs();
    println!("cargo:rerun-if-env-changed=GIT_COMMIT");

    for (path, expected) in PATCHES {
        println!("cargo:rerun-if-changed={path}");
        verify_patch_hash(Path::new(path), expected);
    }
    let out_dir = std::env::var_os("OUT_DIR").expect("OUT_DIR is set by Cargo");
    fs::write(
        Path::new(&out_dir).join("build_provenance.rs"),
        GENERATED_PROVENANCE,
    )
    .expect("writing embedded build provenance");

    println!("cargo:rustc-env=GIT_COMMIT={}", resolve_git_commit());
    println!(
        "cargo:rustc-env=GIT_WORKTREE_CLEAN_AT_BUILD={}",
        resolve_git_worktree_clean()
    );
}

fn verify_patch_hash(path: &Path, expected: &str) {
    let bytes =
        fs::read(path).unwrap_or_else(|error| panic!("reading {}: {error}", path.display()));
    let actual = hex::encode(Sha256::digest(bytes));
    assert_eq!(
        actual,
        expected,
        "tracked patch {} does not match its immutable expected SHA-256",
        path.display()
    );
}

fn track_git_revision_inputs() {
    for pathspec in ["HEAD", "packed-refs", "index"] {
        if let Some(path) = git_output(&["rev-parse", "--git-path", pathspec]) {
            println!("cargo:rerun-if-changed={path}");
        }
    }
    if let Some(reference) = git_output(&["symbolic-ref", "-q", "HEAD"])
        && let Some(path) = git_output(&["rev-parse", "--git-path", &reference])
    {
        println!("cargo:rerun-if-changed={path}");
    }

    // Once a build script emits any rerun-if-changed directive, Cargo stops
    // using its default package-wide change detection. Track every versioned
    // file so an unstaged source edit cannot reuse a prior clean-at-build
    // attestation.
    let Some(paths) = git_output_bytes(&["ls-files", "-z"]) else {
        // A non-Git source archive can still build, but
        // resolve_git_worktree_clean() will make its attestation fail closed.
        return;
    };
    for path in paths.split(|byte| *byte == b'\0') {
        if path.is_empty() {
            continue;
        }
        let path = std::str::from_utf8(path).expect("tracked build provenance paths must be UTF-8");
        assert!(
            !path.contains('\r') && !path.contains('\n'),
            "tracked build provenance paths must not contain newlines"
        );
        println!("cargo:rerun-if-changed={path}");
    }
}

fn resolve_git_worktree_clean() -> bool {
    Command::new("git")
        .args(["status", "--porcelain", "--untracked-files=all"])
        .current_dir(
            std::env::var_os("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR is set by Cargo"),
        )
        .output()
        .ok()
        .filter(|output| output.status.success())
        .is_some_and(|output| output.stdout.is_empty())
}

fn resolve_git_commit() -> String {
    let supplied = std::env::var("GIT_COMMIT")
        .ok()
        .filter(|value| !value.is_empty());
    if let Some(actual) = git_output(&["rev-parse", "--verify", "HEAD"]) {
        assert!(
            is_lowercase_sha1(&actual),
            "git rev-parse HEAD did not return a lowercase 40-character object ID"
        );
        if let Some(supplied) = supplied {
            assert!(
                is_lowercase_sha1(&supplied),
                "GIT_COMMIT must be lowercase 40-character hexadecimal"
            );
            assert_eq!(
                supplied, actual,
                "GIT_COMMIT does not match the checked-out git HEAD"
            );
        }
        return actual;
    }
    match supplied {
        Some(supplied) => {
            assert!(
                is_lowercase_sha1(&supplied),
                "GIT_COMMIT must be lowercase 40-character hexadecimal"
            );
            supplied
        }
        None => "unknown".to_string(),
    }
}

fn git_output(args: &[&str]) -> Option<String> {
    String::from_utf8(git_output_bytes(args)?)
        .ok()
        .map(|output| output.trim().to_string())
        .filter(|output| !output.is_empty())
}

fn git_output_bytes(args: &[&str]) -> Option<Vec<u8>> {
    Command::new("git")
        .args(args)
        .current_dir(std::env::var_os("CARGO_MANIFEST_DIR")?)
        .output()
        .ok()
        .filter(|output| output.status.success())
        .map(|output| output.stdout)
}

fn is_lowercase_sha1(value: &str) -> bool {
    value.len() == 40
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}
