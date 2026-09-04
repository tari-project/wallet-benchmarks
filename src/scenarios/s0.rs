//! S0 — funding baseline.
//!
//! Brings the funded wallet up on a fresh data directory, resolves `H_birth` and waits until
//! the operator's funding UTXO is spendable at depth `C_min`. Funding itself is not part of
//! the measurement (the spec says so), so what is measured here is how long each wallet takes
//! to see and to trust money that is already on its way.
//!
//! `A_fund` is treated as a minimum: the wallet has to be able to run the protocol, and a
//! wallet holding more than `A_fund` can. The amount it was actually funded with is recorded
//! and everything downstream measures against that, not against the parameter.

use std::{sync::Arc, time::Instant};

use anyhow::Result;
use async_trait::async_trait;

use crate::{
    metrics::ScenarioReport,
    modes::{Birthday, WalletMode},
    scenarios::{RunContext, Scenario},
};

pub struct FundingBaseline;

#[async_trait]
impl Scenario for FundingBaseline {
    fn id(&self) -> &'static str {
        "S0"
    }

    fn description(&self) -> &'static str {
        "Fresh wallet on the funded seed; wait for one A_fund UTXO at depth C_min"
    }

    async fn run(
        &mut self,
        mode: &Arc<dyn WalletMode>,
        ctx: &mut RunContext,
    ) -> Result<ScenarioReport> {
        let params = ctx.cfg.params.clone();
        let mut report = ScenarioReport::new(self.id(), mode.name());
        let seed = ctx.funded_seed()?;
        report.tip_height_start = ctx.chain.tip_height().await.unwrap_or(0);

        let started = Instant::now();
        mode.reset(&seed, Birthday::FromSeed).await?;
        let scan = mode.sync_to_tip(params.scan_timeout()).await?;

        // `H_birth` is the height the seed's birthday resolves to. It is deliberately not
        // `scan.start_height`: both wallets step a safety offset back from the birthday
        // before they begin scanning, so the two are a couple of days apart and the report
        // carries each of them separately.
        let birth_height = ctx
            .chain
            .height_at_time(seed.birthday_epoch_secs())
            .await
            .unwrap_or(scan.start_height);
        ctx.state.birth_height = Some(birth_height);

        let mut detected_at: Option<f64> = None;
        let mut spendable_at: Option<f64> = None;
        let deadline = Instant::now() + params.fund_timeout();

        loop {
            let balance = mode.balance().await?;
            if detected_at.is_none() && balance.total() > 0 {
                detected_at = Some(started.elapsed().as_secs_f64());
            }
            if balance.available >= params.a_fund {
                spendable_at = Some(started.elapsed().as_secs_f64());
                break;
            }
            if Instant::now() >= deadline {
                break;
            }
            tokio::time::sleep(params.poll_interval()).await;
            // Keep following the chain while waiting; without this the wallet never sees the
            // funding transaction get buried.
            if let Err(e) = mode.sync_to_tip(params.scan_timeout()).await {
                report.fail(format!("scan while waiting for funding failed: {e}"));
                break;
            }
        }

        let balance = mode.balance().await?;
        let utxos = mode.unspent_amounts().await?;
        report.wall_clock_secs = started.elapsed().as_secs_f64();
        report.tip_height_end = ctx.chain.tip_height().await.unwrap_or(0);
        report.balance_end = balance.total();
        report.utxos_end = utxos.len() as u64;
        report.note("h_birth", birth_height);
        report.note("scan_start_height", scan.start_height);
        report.note("seed_birthday_days", seed.birthday);
        report.note("wallet_address", mode.address().await.unwrap_or_default());
        report.note("funding_address", seed.address.clone());
        report.note("time_to_detect_secs", detected_at);
        report.note("time_to_spendable_secs", spendable_at);
        report.note("a_fund", params.a_fund);
        report.note("c_min", params.c_min);
        report.note(
            "note",
            "funding is performed by the operator before the run, so only detection and \
             confirmation are timed here",
        );
        report.scan = Some(scan);

        if spendable_at.is_none() {
            report.fail(format!(
                "the funding UTXO was not spendable after {:.0}s: available {} µT of the {} µT \
                 required",
                params.fund_timeout().as_secs_f64(),
                balance.available,
                params.a_fund
            ));
        }
        for failure in funding_failures(balance.available, params.a_fund, utxos.len()) {
            report.fail(failure);
        }

        report.note("funded_amount", balance.available);
        report.note("total_balance", balance.total());
        if balance.available > params.a_fund {
            report.note("surplus_over_a_fund", balance.available - params.a_fund);
        }
        // Nothing left the wallet during S0, so whatever it holds is what it should hold.
        // Reconciling against `A_fund` instead would report a surplus as a discrepancy, and
        // the scenarios that follow measure against this closing balance, not against the
        // parameter.
        report.reconcile(balance.total());
        Ok(report)
    }
}

/// Checks the funded wallet is in a state the rest of the protocol can start from.
///
/// Both figures are floors, not exact values. `A_fund` is a floor because the operator funds
/// from a faucet, a miner wallet or a sweep-back and none of those pay out to the microTari.
/// The UTXO count is a floor because S1 scales its build-up to whatever the wallet starts
/// with — the reference protocol starts from one UTXO, and a run that starts from four says
/// so in its profile rather than failing. Only an underfunded or empty wallet fails.
fn funding_failures(available: u64, a_fund: u64, utxo_count: usize) -> Vec<String> {
    let mut failures = Vec::new();
    if available < a_fund {
        failures.push(format!(
            "balance {available} µT is short of A_fund {a_fund} µT"
        ));
    }
    if utxo_count == 0 {
        failures.push("the wallet holds no spendable UTXOs to build up from".to_string());
    }
    failures
}

#[cfg(test)]
mod tests {
    use super::funding_failures;

    #[test]
    fn funding_exactly_a_fund_passes() {
        assert!(funding_failures(3_000_000, 3_000_000, 1).is_empty());
    }

    #[test]
    fn funding_above_a_fund_passes() {
        assert!(
            funding_failures(4_000_000, 3_000_000, 1).is_empty(),
            "A_fund is a minimum: over-funding a wallet is not a failed scenario"
        );
    }

    #[test]
    fn funding_below_a_fund_fails() {
        let failures = funding_failures(2_999_999, 3_000_000, 1);
        assert_eq!(failures.len(), 1);
        assert!(failures[0].contains("short of A_fund"), "{}", failures[0]);
    }

    #[test]
    fn several_funding_utxos_pass_because_s1_scales_to_them() {
        assert!(funding_failures(4_000_000, 3_000_000, 4).is_empty());
    }

    #[test]
    fn an_empty_wallet_fails() {
        assert_eq!(funding_failures(4_000_000, 3_000_000, 0).len(), 1);
        assert_eq!(funding_failures(0, 3_000_000, 0).len(), 2);
    }
}
