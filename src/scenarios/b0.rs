//! B0 — baseline scan of an empty wallet.
//!
//! The floor cost of walking the chain and checking every output against a view key with
//! nothing to store. Runs against a seed that is never funded, from birthday 0.

use std::{sync::Arc, time::Instant};

use anyhow::Result;
use async_trait::async_trait;

use crate::{
    metrics::ScenarioReport,
    modes::{Birthday, WalletMode},
    scenarios::{RunContext, Scenario},
};

pub struct BaselineScan;

#[async_trait]
impl Scenario for BaselineScan {
    fn id(&self) -> &'static str {
        "B0"
    }

    fn description(&self) -> &'static str {
        "Baseline scan from genesis on a fresh, never-funded wallet"
    }

    async fn run(&self, mode: &Arc<dyn WalletMode>, ctx: &mut RunContext) -> Result<ScenarioReport> {
        let mut report = ScenarioReport::new(self.id(), mode.name());
        let seed = ctx.baseline_seed()?;
        report.tip_height_start = ctx.chain.tip_height().await.unwrap_or(0);

        let started = Instant::now();
        mode.reset(&seed, Birthday::Genesis).await?;
        let scan = mode.sync_to_tip(ctx.cfg.params.scan_timeout()).await?;
        report.wall_clock_secs = started.elapsed().as_secs_f64();
        report.tip_height_end = scan.tip_height_end;

        let balance = mode.balance().await?;
        let utxos = mode.unspent_amounts().await?;
        report.balance_end = balance.total();
        report.utxos_end = utxos.len() as u64;
        report.note("wallet_address", mode.address().await.unwrap_or_default());
        report.note("outputs_found", scan.outputs_found);
        report.scan = Some(scan);

        // The whole point of B0 is that this wallet has nothing in it.
        if !utxos.is_empty() {
            report.fail(format!(
                "the baseline wallet found {} UTXOs; its seed must never be funded",
                utxos.len()
            ));
        }
        if balance.total() != 0 {
            report.fail(format!(
                "the baseline wallet has a balance of {} µT; its seed must never be funded",
                balance.total()
            ));
        }
        report.reconcile(0);
        Ok(report)
    }
}
