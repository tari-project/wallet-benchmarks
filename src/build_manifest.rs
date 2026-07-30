use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
};

use anyhow::{Context, bail};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::config::Config;

pub const BUILD_MANIFEST_SCHEMA_VERSION: u32 = 2;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BuildManifest {
    pub schema_version: u32,
    pub sources: BTreeMap<String, SourceProvenance>,
    pub artifacts: BTreeMap<String, BuildArtifact>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SourceProvenance {
    pub repository: String,
    pub upstream: UpstreamSource,
    pub patches: Vec<AppliedPatch>,
    pub complete_diff_sha256: String,
    pub result_tree: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UpstreamSource {
    pub revision: String,
    pub commit: String,
    pub tree: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AppliedPatch {
    pub path: String,
    pub sha256: String,
    pub result_tree: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BuildArtifact {
    pub source: String,
    pub source_revision: String,
    pub source_tree: String,
    pub sha256: String,
}

pub(crate) struct ExpectedPatch {
    path: &'static str,
    sha256: &'static str,
    result_tree: &'static str,
}

pub(crate) struct ExpectedSource {
    name: &'static str,
    repository: &'static str,
    upstream_revision: &'static str,
    upstream_commit: &'static str,
    upstream_tree: &'static str,
    patches: &'static [ExpectedPatch],
    complete_diff_sha256: &'static str,
    result_tree: &'static str,
}

pub(crate) struct ExpectedArtifact {
    name: &'static str,
    source: &'static str,
    source_revision: &'static str,
    source_tree: &'static str,
}

include!(concat!(env!("OUT_DIR"), "/build_provenance.rs"));

pub fn verify(config: &Config) -> anyhow::Result<()> {
    let bytes = fs::read(&config.paths.build_manifest).with_context(|| {
        format!(
            "reading build manifest {} (rerun both fetch scripts)",
            config.paths.build_manifest.display()
        )
    })?;
    let manifest: BuildManifest = serde_json::from_slice(&bytes).with_context(|| {
        format!(
            "parsing build manifest {}",
            config.paths.build_manifest.display()
        )
    })?;

    verify_configured_revisions(config)?;
    let artifact_paths = BTreeMap::from([
        ("minotari".to_string(), config.paths.minotari_binary.clone()),
        (
            "minotari_console_wallet".to_string(),
            config.paths.minotari_console_wallet.clone(),
        ),
        (
            "minotari_node".to_string(),
            config.paths.minotari_node.clone(),
        ),
        (
            "minotari_payment_processor".to_string(),
            config.paths.payment_processor_binary.clone(),
        ),
    ]);
    verify_manifest(
        &manifest,
        &artifact_paths,
        Path::new(env!("CARGO_MANIFEST_DIR")),
    )?;
    println!(
        "build manifest PASS: schema v2 upstream bases, ordered patches, result trees, and runtime artifact SHA-256 values match"
    );
    Ok(())
}

fn verify_configured_revisions(config: &Config) -> anyhow::Result<()> {
    let configured = [
        ("minotari", config.versions.minotari_cli_rev.as_str()),
        (
            "minotari_console_wallet",
            config.versions.tari_console_wallet_rev.as_str(),
        ),
        ("minotari_node", config.versions.base_node_rev.as_str()),
        (
            "minotari_payment_processor",
            config.versions.payment_processor_rev.as_str(),
        ),
    ];
    for (name, revision) in configured {
        let expected = EXPECTED_ARTIFACTS
            .iter()
            .find(|artifact| artifact.name == name)
            .expect("embedded artifact provenance is complete");
        if revision != expected.source_revision {
            bail!(
                "configured {name} revision {revision} does not match embedded provenance {}",
                expected.source_revision
            );
        }
    }
    Ok(())
}

fn verify_manifest(
    manifest: &BuildManifest,
    artifact_paths: &BTreeMap<String, PathBuf>,
    source_root: &Path,
) -> anyhow::Result<()> {
    verify_portable_manifest(manifest)?;
    if artifact_paths.len() != EXPECTED_ARTIFACTS.len() {
        bail!("build manifest artifact path set is not exact");
    }
    for expected in EXPECTED_ARTIFACTS {
        let artifact = manifest
            .artifacts
            .get(expected.name)
            .expect("portable manifest verification checked every artifact");
        let path = artifact_paths
            .get(expected.name)
            .with_context(|| format!("runtime path is missing for artifact {}", expected.name))?;
        if sha256_file(path)? != artifact.sha256 {
            bail!(
                "{} SHA-256 does not match the build manifest",
                expected.name
            );
        }
    }
    for expected in EXPECTED_SOURCES {
        for expected_patch in expected.patches {
            let patch_path = source_root.join(expected_patch.path);
            if sha256_file(&patch_path)? != expected_patch.sha256 {
                bail!(
                    "tracked patch {} SHA-256 does not match embedded provenance",
                    expected_patch.path
                );
            }
        }
    }
    Ok(())
}

pub(crate) fn verify_portable_manifest(manifest: &BuildManifest) -> anyhow::Result<()> {
    if manifest.schema_version != BUILD_MANIFEST_SCHEMA_VERSION {
        bail!(
            "unsupported build manifest schema {}; expected {}",
            manifest.schema_version,
            BUILD_MANIFEST_SCHEMA_VERSION
        );
    }
    if manifest.sources.len() != EXPECTED_SOURCES.len() {
        bail!(
            "build manifest source set is not exact: expected {}, found {}",
            EXPECTED_SOURCES.len(),
            manifest.sources.len()
        );
    }
    for expected in EXPECTED_SOURCES {
        let source = manifest
            .sources
            .get(expected.name)
            .with_context(|| format!("build manifest is missing source {}", expected.name))?;
        verify_source_claims(expected, source)?;
    }

    if manifest.artifacts.len() != EXPECTED_ARTIFACTS.len() {
        bail!("build manifest artifact set is not exact");
    }
    for expected in EXPECTED_ARTIFACTS {
        let artifact = manifest
            .artifacts
            .get(expected.name)
            .with_context(|| format!("build manifest is missing artifact {}", expected.name))?;
        if artifact.source != expected.source
            || artifact.source_revision != expected.source_revision
            || artifact.source_tree != expected.source_tree
        {
            bail!(
                "build manifest artifact {} source provenance does not match the embedded expectation",
                expected.name
            );
        }
        require_sha256_hex(&artifact.sha256, &format!("artifact {}", expected.name))?;
    }
    Ok(())
}

fn verify_source_claims(
    expected: &ExpectedSource,
    source: &SourceProvenance,
) -> anyhow::Result<()> {
    require_sha256_hex(
        &source.complete_diff_sha256,
        &format!("source {} complete diff", expected.name),
    )?;
    if source.repository != expected.repository
        || source.upstream.revision != expected.upstream_revision
        || source.upstream.commit != expected.upstream_commit
        || source.upstream.tree != expected.upstream_tree
        || source.complete_diff_sha256 != expected.complete_diff_sha256
        || source.result_tree != expected.result_tree
        || source.patches.len() != expected.patches.len()
    {
        bail!(
            "build manifest source {} does not match the embedded upstream/tree provenance",
            expected.name
        );
    }
    for (index, (patch, expected_patch)) in source
        .patches
        .iter()
        .zip(expected.patches.iter())
        .enumerate()
    {
        require_sha256_hex(
            &patch.sha256,
            &format!("source {} patch {}", expected.name, index + 1),
        )?;
        if patch.path != expected_patch.path
            || patch.sha256 != expected_patch.sha256
            || patch.result_tree != expected_patch.result_tree
        {
            bail!(
                "build manifest source {} patch {} is not the expected ordered patch",
                expected.name,
                index + 1
            );
        }
    }
    Ok(())
}

fn require_sha256_hex(value: &str, label: &str) -> anyhow::Result<()> {
    if value.len() != 64
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        bail!("{label} SHA-256 is not lowercase 64-character hexadecimal");
    }
    Ok(())
}

pub(crate) fn sha256_file(path: &Path) -> anyhow::Result<String> {
    let bytes =
        fs::read(path).with_context(|| format!("reading {} for SHA-256", path.display()))?;
    Ok(hex::encode(Sha256::digest(bytes)))
}

#[cfg(test)]
pub(crate) fn portable_manifest_fixture() -> BuildManifest {
    let sources = EXPECTED_SOURCES
        .iter()
        .map(|source| {
            (
                source.name.to_string(),
                SourceProvenance {
                    repository: source.repository.to_string(),
                    upstream: UpstreamSource {
                        revision: source.upstream_revision.to_string(),
                        commit: source.upstream_commit.to_string(),
                        tree: source.upstream_tree.to_string(),
                    },
                    patches: source
                        .patches
                        .iter()
                        .map(|patch| AppliedPatch {
                            path: patch.path.to_string(),
                            sha256: patch.sha256.to_string(),
                            result_tree: patch.result_tree.to_string(),
                        })
                        .collect(),
                    complete_diff_sha256: source.complete_diff_sha256.to_string(),
                    result_tree: source.result_tree.to_string(),
                },
            )
        })
        .collect();
    let artifacts = EXPECTED_ARTIFACTS
        .iter()
        .map(|artifact| {
            (
                artifact.name.to_string(),
                BuildArtifact {
                    source: artifact.source.to_string(),
                    source_revision: artifact.source_revision.to_string(),
                    source_tree: artifact.source_tree.to_string(),
                    sha256: "00".repeat(32),
                },
            )
        })
        .collect();
    BuildManifest {
        schema_version: BUILD_MANIFEST_SCHEMA_VERSION,
        sources,
        artifacts,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn schema_v2_manifest_verifies_exact_embedded_provenance() {
        let dir = tempfile::tempdir().unwrap();
        let (manifest, paths) = manifest_fixture(dir.path());
        verify_manifest(&manifest, &paths, Path::new(env!("CARGO_MANIFEST_DIR"))).unwrap();
    }

    #[test]
    fn base_node_manifest_pin_is_tari_v5_5_0() {
        let source = EXPECTED_SOURCES
            .iter()
            .find(|source| source.name == "minotari_node")
            .unwrap();
        assert_eq!(source.upstream_revision, "v5.5.0");
        assert_eq!(
            source.upstream_commit,
            "b32dba96aa0d4ffdd715a0bb724f69c567d47d9f"
        );
        assert_eq!(
            source.upstream_tree,
            "449b76bc4121400265927561a661c5c381d55bcd"
        );
        assert_eq!(source.result_tree, source.upstream_tree);

        let artifact = EXPECTED_ARTIFACTS
            .iter()
            .find(|artifact| artifact.name == "minotari_node")
            .unwrap();
        assert_eq!(artifact.source_revision, source.upstream_revision);
        assert_eq!(artifact.source_tree, source.upstream_tree);
    }

    #[test]
    fn manifest_rejects_changed_result_tree() {
        let dir = tempfile::tempdir().unwrap();
        let (mut manifest, paths) = manifest_fixture(dir.path());
        manifest
            .sources
            .get_mut("minotari_cli")
            .unwrap()
            .result_tree = "0000000000000000000000000000000000000000".to_string();
        let error = verify_manifest(&manifest, &paths, Path::new(env!("CARGO_MANIFEST_DIR")))
            .unwrap_err()
            .to_string();
        assert!(error.contains("upstream/tree provenance"));
    }

    #[test]
    fn manifest_rejects_unknown_fields() {
        let json = r#"{
            "schema_version": 2,
            "sources": {},
            "artifacts": {},
            "untracked_claim": true
        }"#;
        assert!(serde_json::from_str::<BuildManifest>(json).is_err());
    }

    #[test]
    fn portable_manifest_rejects_non_lowercase_sha256() {
        let mut manifest = portable_manifest_fixture();
        manifest.artifacts.get_mut("minotari").unwrap().sha256 = "AA".repeat(32);
        let error = verify_portable_manifest(&manifest).unwrap_err().to_string();
        assert!(error.contains("lowercase 64-character hexadecimal"));
    }

    #[cfg(unix)]
    #[test]
    fn emitted_manifest_claims_match_embedded_provenance() {
        let dir = tempfile::tempdir().unwrap();
        let output = dir.path().join("build-manifest.json");
        let zero_sha256 = "00".repeat(32);
        let script = Path::new(env!("CARGO_MANIFEST_DIR")).join("scripts/build-provenance.sh");
        let status = std::process::Command::new("bash")
            .arg(script)
            .arg("--emit-manifest")
            .arg(&output)
            .args([&zero_sha256, &zero_sha256, &zero_sha256, &zero_sha256])
            .status()
            .unwrap();
        assert!(status.success());

        let manifest: BuildManifest = serde_json::from_slice(&fs::read(output).unwrap()).unwrap();
        assert_eq!(manifest, portable_manifest_fixture());
        verify_portable_manifest(&manifest).unwrap();
    }

    fn manifest_fixture(root: &Path) -> (BuildManifest, BTreeMap<String, PathBuf>) {
        let mut manifest = portable_manifest_fixture();
        let mut paths = BTreeMap::new();
        for expected in EXPECTED_ARTIFACTS {
            let path = root.join(expected.name);
            fs::write(&path, format!("test artifact {}", expected.name)).unwrap();
            manifest
                .artifacts
                .get_mut(expected.name)
                .expect("portable fixture has every expected artifact")
                .sha256 = sha256_file(&path).unwrap();
            paths.insert(expected.name.to_string(), path);
        }
        (manifest, paths)
    }
}
