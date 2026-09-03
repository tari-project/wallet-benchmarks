//! Run orchestration: the cross product of the requested modes and scenarios.
//!
//! Scenarios run in protocol order against one mode at a time, sharing a [`RunContext`] so
//! later scenarios can use what earlier ones established. A scenario that fails does not stop
//! the run: the matrix is more useful complete, and a failure is itself a result.

use std::{path::PathBuf, sync::Arc};

use anyhow::{anyhow, Context, Result};
use log::{info, warn};

use crate::{
    chain::ChainClient,
    config::HarnessConfig,
    metrics::{Outcome, ResultProfile, ScenarioReport, Versions, MINOTARI_CLI_REV},
    modes::{new_wallet::NewWallet, old_wallet::OldWallet, WalletMode, NEW_WALLET, OLD_WALLET},
    scenarios::{self, RunContext},
    seeds::{baseline_label, SeedBook},
    sysprobe::EnvironmentInfo,
};

/// Builds the wallet mode with the given label.
pub fn build_mode(
    label: &str,
    cfg: Arc<HarnessConfig>,
    chain: ChainClient,
) -> Result<Arc<dyn WalletMode>> {
    match label {
        OLD_WALLET => Ok(Arc::new(OldWallet::new(cfg, chain)?)),
        NEW_WALLET => Ok(Arc::new(NewWallet::new(cfg, chain)?)),
        other => Err(anyhow!(
            "Unknown mode `{other}`; this harness implements `{OLD_WALLET}` and `{NEW_WALLET}`"
        )),
    }
}

/// Creates any missing seeds for the requested modes and returns the book.
pub fn prepare_seeds(cfg: &HarnessConfig, modes: &[String]) -> Result<SeedBook> {
    let network = cfg.tari_network()?;
    let mut book = SeedBook::load_or_new(&cfg.seed_file, &cfg.network)?;
    for mode in modes {
        book.ensure(mode, network, true)?;
        // B0 needs a wallet that has never been funded, so every mode gets a second seed.
        book.ensure(&baseline_label(mode), network, false)?;
    }
    book.save(&cfg.seed_file)?;
    Ok(book)
}

/// Runs the requested scenarios against the requested modes and returns the profile.
pub async fn run(
    cfg: Arc<HarnessConfig>,
    modes: &[String],
    scenario_ids: &[String],
) -> Result<ResultProfile> {
    let chain = ChainClient::new(&cfg.base_node_url)?;
    let tip = chain
        .tip_height()
        .await
        .with_context(|| format!("Base node {} is not reachable", cfg.base_node_url))?;
    let synced = chain.is_synced().await.ok();
    info!("Base node {} is at height {tip} (synced: {synced:?})", cfg.base_node_url);

    let seeds = SeedBook::load(&cfg.seed_file, &cfg.network)?;
    for mode in modes {
        seeds.require(mode)?;
        seeds.require(&baseline_label(mode))?;
    }

    let mut reports = Vec::new();
    let mut console_wallet_version = None;

    for mode_label in modes {
        let mode = build_mode(mode_label, Arc::clone(&cfg), chain.clone())?;
        let mut ctx = RunContext::new(
            Arc::clone(&cfg),
            chain.clone(),
            seeds.clone(),
            mode_label,
        );

        // The scenarios are a chain: S1 builds the UTXO set S2 and S3 rescan, S4 and S5 spend
        // it, S6 rescans what they left. Once one link breaks, everything after it measures a
        // wallet state the protocol never asked for, so the rest of the mode is skipped
        // rather than run and reported as though it meant something.
        let mut halted_by: Option<String> = None;

        for id in scenario_ids {
            let Some(scenario) = scenarios::scenario(id) else {
                reports.push(ScenarioReport::skipped(id, mode_label, "unknown scenario id"));
                continue;
            };
            if let Some(failed) = &halted_by {
                reports.push(ScenarioReport::skipped(
                    id,
                    mode_label,
                    format!("not run: {failed} failed, and this scenario depends on its result"),
                ));
                continue;
            }

            info!("[{mode_label}] {} — {}", scenario.id(), scenario.description());
            let started = std::time::Instant::now();
            match scenario.run(&mode, &mut ctx).await {
                Ok(report) => {
                    if report.outcome == Outcome::Failed {
                        warn!(
                            "[{mode_label}] {} failed: {}. Skipping the rest of this mode.",
                            report.scenario,
                            report.error.as_deref().unwrap_or("unknown reason")
                        );
                        halted_by = Some(report.scenario.clone());
                    }
                    reports.push(report);
                },
                Err(e) => {
                    // The harness could not take the measurement at all.
                    warn!(
                        "[{mode_label}] {} could not run: {e:#}. Skipping the rest of this mode.",
                        scenario.id()
                    );
                    let mut report = ScenarioReport::new(scenario.id(), mode_label);
                    // Keep the time it burned before failing: an hour of scanning that ended
                    // in an error is not a zero-second scenario.
                    report.wall_clock_secs = started.elapsed().as_secs_f64();
                    report.fail(format!("{e:#}"));
                    halted_by = Some(report.scenario.clone());
                    reports.push(report);
                },
            }
        }

        if mode_label == OLD_WALLET {
            console_wallet_version = mode.version().await.ok().flatten();
        }
        if let Err(e) = mode.shutdown().await {
            warn!("[{mode_label}] shutdown reported: {e:#}");
        }
    }

    let versions = Versions {
        harness: env!("CARGO_PKG_VERSION").to_string(),
        console_wallet: console_wallet_version,
        minotari_cli_rev: MINOTARI_CLI_REV.to_string(),
        base_node_url: cfg.base_node_url.clone(),
        base_node_synced: synced,
    };
    let environment = EnvironmentInfo::collect(&cfg.base_node_url);
    Ok(ResultProfile::new(
        &cfg.network,
        environment,
        versions,
        (*cfg).clone(),
        reports,
    ))
}

/// Writes a profile into the results directory and returns the path.
pub fn write_profile(cfg: &HarnessConfig, profile: &ResultProfile) -> Result<PathBuf> {
    std::fs::create_dir_all(&cfg.results_dir)
        .with_context(|| format!("Could not create {}", cfg.results_dir.display()))?;
    let name = format!(
        "profile-{}.json",
        profile.generated_at.format("%Y%m%dT%H%M%SZ")
    );
    let path = cfg.results_dir.join(name);
    std::fs::write(&path, serde_json::to_string_pretty(profile)?)
        .with_context(|| format!("Could not write {}", path.display()))?;
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_config(dir: &std::path::Path) -> HarnessConfig {
        HarnessConfig {
            data_dir: dir.join("data"),
            results_dir: dir.join("results"),
            seed_file: dir.join("seeds.json"),
            ..HarnessConfig::default()
        }
    }

    #[test]
    fn unknown_modes_are_rejected_by_name() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = Arc::new(test_config(dir.path()));
        let chain = ChainClient::new(&cfg.base_node_url).unwrap();
        let err = match build_mode("payment_processor", cfg, chain) {
            Err(e) => e.to_string(),
            Ok(_) => panic!("the payment processor mode is out of scope for this harness"),
        };
        assert!(err.contains("payment_processor"), "unexpected error: {err}");
    }

    #[test]
    fn preparing_seeds_creates_a_funded_and_a_baseline_wallet_per_mode() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = test_config(dir.path());
        let modes = vec![OLD_WALLET.to_string(), NEW_WALLET.to_string()];

        let book = prepare_seeds(&cfg, &modes).unwrap();
        assert_eq!(book.entries.len(), 4);
        assert_eq!(book.funding_targets().len(), 2);
        assert!(!book.require("old_wallet-baseline").unwrap().needs_funding);

        // Re-preparing must not rotate the funded seed, or the operator's money is stranded.
        let again = prepare_seeds(&cfg, &modes).unwrap();
        assert_eq!(book, again);
    }

    #[test]
    fn profiles_are_written_with_a_timestamped_name() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = test_config(dir.path());
        let profile = ResultProfile::new(
            &cfg.network,
            EnvironmentInfo::collect(&cfg.base_node_url),
            Versions {
                harness: "test".to_string(),
                console_wallet: None,
                minotari_cli_rev: MINOTARI_CLI_REV.to_string(),
                base_node_url: cfg.base_node_url.clone(),
                base_node_synced: Some(true),
            },
            cfg.clone(),
            vec![ScenarioReport::new("B0", OLD_WALLET)],
        );
        let path = write_profile(&cfg, &profile).unwrap();
        assert!(path.exists());
        let parsed: ResultProfile =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(parsed.reports.len(), 1);
        assert_eq!(parsed.config.params.volume_target, 512);
    }
}
