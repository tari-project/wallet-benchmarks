//! S2, S3, S6 and S7 — rescans of a populated wallet.
//!
//! All three have the same shape: snapshot what the wallet holds, wipe its data directory,
//! bring it back up from a birthday and check that the scan rediscovers exactly what was
//! there. They differ only in when they run and where they start scanning from.

use std::{sync::Arc, time::Instant};

use anyhow::Result;
use async_trait::async_trait;

use crate::{
    metrics::ScenarioReport,
    modes::{Birthday, WalletMode},
    scenarios::{RunContext, Scenario},
};

pub struct ScanScenario {
    id: &'static str,
    description: &'static str,
    birthday: Birthday,
}

impl ScanScenario {
    /// Scan from genesis, checkpoint 1 (after S1).
    pub fn s2() -> Self {
        Self {
            id: "S2",
            description: "Rescan from genesis after the UTXO build-up",
            birthday: Birthday::Genesis,
        }
    }

    /// Scan from the seed birthday, checkpoint 1.
    pub fn s3() -> Self {
        Self {
            id: "S3",
            description: "Rescan from the wallet birthday after the UTXO build-up",
            birthday: Birthday::FromSeed,
        }
    }

    /// Scan from genesis, checkpoint 2 (after S4 and S5).
    pub fn s6() -> Self {
        Self {
            id: "S6",
            description: "Rescan from genesis after the concurrency and throughput scenarios",
            birthday: Birthday::Genesis,
        }
    }

    /// Scan from the seed birthday, checkpoint 2.
    ///
    /// S6 is to S7 what S2 is to S3: the same wallet state rescanned from genesis and from the
    /// birthday, so the cost of the blocks before the wallet existed can be separated from the
    /// cost of its own history. Pairing it with S3 also shows what S4 and S5 added to a
    /// birthday scan, which is the delta `T_scan(S7) - T_scan(S3)`.
    pub fn s7() -> Self {
        Self {
            id: "S7",
            description: "Rescan from the wallet birthday after the concurrency and throughput \
                          scenarios",
            birthday: Birthday::FromSeed,
        }
    }
}

#[async_trait]
impl Scenario for ScanScenario {
    fn id(&self) -> &'static str {
        self.id
    }

    fn description(&self) -> &'static str {
        self.description
    }

    async fn run(
        &self,
        mode: &Arc<dyn WalletMode>,
        ctx: &mut RunContext,
    ) -> Result<ScenarioReport> {
        let params = ctx.cfg.params.clone();
        let mut report = ScenarioReport::new(self.id, mode.name());
        let seed = ctx.funded_seed()?;
        report.tip_height_start = ctx.chain.tip_height().await.unwrap_or(0);

        // Bring the wallet up to the tip before snapshotting it. Without this the comparison
        // is against whatever the previous scenario left behind: a wallet that has not caught
        // up under-reports its UTXOs, and the rescan then looks like it invented some.
        if let Err(e) = mode.sync_to_tip(params.scan_timeout()).await {
            report.note("pre_wipe_sync_error", e.to_string());
        }

        // Snapshot the live wallet before wiping it: that is what the rescan has to reproduce.
        // The height it was taken at matters as much as the numbers — the rescan will finish
        // several blocks later, and the two readings have to describe the same chain.
        let snapshot_height = mode.scanned_height().await.unwrap_or(0);
        let before_balance = mode.balance().await.ok();
        let before_utxos = mode.unspent_amounts().await.ok().map(|u| u.len() as u64);
        report.balance_start = before_balance.map_or(0, |b| b.total());
        report.utxos_start = before_utxos.unwrap_or(0);

        let started = Instant::now();
        mode.reset(&seed, self.birthday).await?;
        let scan = mode.sync_to_tip(params.scan_timeout()).await?;
        report.wall_clock_secs = started.elapsed().as_secs_f64();
        report.tip_height_end = scan.tip_height_end;

        let balance = mode.balance().await?;
        let utxos = mode.unspent_amounts().await?;
        report.balance_end = balance.total();
        report.utxos_end = utxos.len() as u64;
        report.note("birthday_policy", format!("{:?}", self.birthday));
        report.note("outputs_found", scan.outputs_found);
        report.note("blocks_scanned", scan.blocks_scanned);
        if let Some(birth_height) = ctx.state.birth_height {
            report.note("h_birth", birth_height);
        }
        report.scan = Some(scan);

        // Compare the two readings at the same height where the wallet can do it. The chain
        // moves during a rescan, and an output whose maturity falls inside that window is
        // missing from the earlier reading and present in the later one for no reason but the
        // passage of blocks. A wallet that cannot answer as of a past height is compared as-is
        // and says so.
        let (rediscovered, comparison) = match mode.unspent_amounts_at(snapshot_height).await {
            Ok(Some(at_snapshot)) if snapshot_height > 0 => (
                at_snapshot.len() as u64,
                format!("as of height {snapshot_height}"),
            ),
            _ => (
                report.utxos_end,
                "at the rescan's own height; the chain moved in between".to_string(),
            ),
        };
        report.note("snapshot_height", snapshot_height);
        report.note("utxos_rediscovered", rediscovered);
        report.note("utxo_comparison", comparison.clone());

        match before_utxos {
            Some(expected) if expected != rediscovered => report.fail(format!(
                "the rescan rediscovered {rediscovered} UTXOs {comparison}, the wallet held \
                 {expected} before the wipe"
            )),
            Some(_) => {},
            None => report.fail(
                "the wallet state before the wipe could not be read, so the rescan cannot be \
                 verified",
            ),
        }
        if let Some(before) = before_balance {
            // Totals, not spendable balances: a fresh scan has to mark every rediscovered
            // output confirmed again, so `available` can legitimately differ on either side
            // of the wipe while the wallet holds exactly the same money.
            if before.total() != balance.total() {
                report.fail(format!(
                    "the rescan produced a balance of {} µT, the wallet held {} µT before the wipe",
                    balance.total(),
                    before.total()
                ));
            }
            report.note("available_before", before.available);
            report.note("available_after", balance.available);
            report.reconcile(before.total());
        }
        Ok(report)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_checkpoint_pairs_a_genesis_scan_with_a_birthday_scan() {
        // Checkpoint 1 is S2/S3, checkpoint 2 is S6/S7. Without S7 the second checkpoint has
        // only a genesis scan, and there is nothing to attribute the pre-birthday blocks to.
        assert_eq!(ScanScenario::s2().birthday, Birthday::Genesis);
        assert_eq!(ScanScenario::s3().birthday, Birthday::FromSeed);
        assert_eq!(ScanScenario::s6().birthday, Birthday::Genesis);
        assert_eq!(ScanScenario::s7().birthday, Birthday::FromSeed);
    }

    #[test]
    fn s7_is_the_second_birthday_rescan_and_says_so() {
        let s7 = ScanScenario::s7();
        assert_eq!(s7.id(), "S7");
        assert!(s7.description().contains("birthday"));
        assert!(
            s7.description().contains("concurrency and throughput"),
            "S7 has to name the checkpoint it runs at: {}",
            s7.description()
        );
    }
}
