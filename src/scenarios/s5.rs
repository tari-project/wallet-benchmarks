//! S5 — payment throughput: batch 1-to-N against individual 1-to-1 sends.
//!
//! Both arms pay the same fixed set of `M` recipients. Arm A submits `M / K` batch
//! transactions of `K` outputs each; arm B submits `M` single-output transactions. The
//! wallet state is whatever S4 left behind and is disclosed rather than normalised.

use std::{sync::Arc, time::Instant};

use anyhow::Result;
use async_trait::async_trait;
use serde_json::json;

use crate::{
    metrics::ScenarioReport,
    modes::{OutputSpec, SendRequest, WalletMode},
    scenarios::{
        support::{
            await_confirmations, distinct_errors, send_recorded, wait_until_spendable, Pending,
            SpendRequirement, TxTally,
        },
        RunContext, Scenario,
    },
};

/// What one arm of the comparison cost.
#[derive(Debug, Clone, Copy)]
struct ArmResult {
    duration_secs: f64,
    fees: u64,
    tally: TxTally,
    blocks_consumed: u64,
    txs: usize,
}

impl ArmResult {
    /// Whether this arm actually paid everyone it was supposed to.
    ///
    /// An arm that failed part way still has a duration, but that duration is the time it
    /// took to give up, not the time to pay `M` recipients. Comparing it against the other
    /// arm produces a headline number that means nothing — ten batch calls rejected in 14 ms
    /// against a completed individual arm reads as a 25,000x speed-up — so the headline is
    /// only published when both arms finished.
    fn complete(&self) -> bool {
        self.txs > 0 && self.tally.unsuccessful() == 0
    }
}

pub struct PaymentThroughput;

#[async_trait]
impl Scenario for PaymentThroughput {
    fn id(&self) -> &'static str {
        "S5"
    }

    fn description(&self) -> &'static str {
        "Pay M recipients twice: once as M/K batch transactions, once as M individual sends"
    }

    async fn run(&self, mode: &Arc<dyn WalletMode>, ctx: &mut RunContext) -> Result<ScenarioReport> {
        let params = ctx.cfg.params.clone();
        let network = ctx.cfg.tari_network()?;
        let mut report = ScenarioReport::new(self.id(), mode.name());
        report.tip_height_start = ctx.chain.tip_height().await.unwrap_or(0);

        let started = Instant::now();
        mode.sync_to_tip(params.scan_timeout()).await?;

        // Post-S4 state, disclosed and not normalised.
        report.balance_start = mode.balance().await?.total();
        report.utxos_start = mode.unspent_amounts().await?.len() as u64;

        // One recipient set for both arms, generated once per mode and remembered.
        if ctx.state.recipients.len() != params.s5_m as usize {
            ctx.state.recipients =
                crate::seeds::generate_recipient_addresses(params.s5_m as usize, network)?;
        }
        let recipients = ctx.state.recipients.clone();

        let batch = if mode.max_outputs_per_tx() < params.s5_k as usize {
            report.note(
                "batch_skipped",
                format!(
                    "this mode cannot put {} outputs in one transaction",
                    params.s5_k
                ),
            );
            None
        } else {
            Some(
                run_arm(
                    mode,
                    ctx,
                    &mut report,
                    &recipients,
                    params.s5_k as usize,
                    "s5-batch",
                )
                .await?,
            )
        };

        mode.sync_to_tip(params.scan_timeout()).await?;

        let individual = run_arm(mode, ctx, &mut report, &recipients, 1, "s5-individual").await?;

        let tally = TxTally::of(&report.transactions);
        report.wall_clock_secs = started.elapsed().as_secs_f64();
        report.tip_height_end = ctx.chain.tip_height().await.unwrap_or(0);
        report.fees_paid = report.total_recorded_fees();
        report.balance_end = mode.balance().await?.total();
        report.utxos_end = mode.unspent_amounts().await?.len() as u64;
        report.successes = tally.confirmed;
        report.failures = tally.unsuccessful();

        #[allow(clippy::cast_precision_loss)]
        let per_recipient = |fees: u64| fees as f64 / f64::from(params.s5_m);
        if let Some(batch) = batch {
            if batch.complete() {
                report.note("t_batch_secs", batch.duration_secs);
            }
            report.note(
                "arm_a_batch",
                json!({
                    "txs": batch.txs,
                    "outputs_per_tx": params.s5_k,
                    "duration_secs": batch.duration_secs,
                    "fees": batch.fees,
                    "fee_per_recipient": per_recipient(batch.fees),
                    "blocks_consumed": batch.blocks_consumed,
                    "tally": batch.tally,
                }),
            );
        }
        if individual.complete() {
            report.note("t_individual_secs", individual.duration_secs);
        }
        report.note(
            "arm_b_individual",
            json!({
                "txs": individual.txs,
                "outputs_per_tx": 1,
                "duration_secs": individual.duration_secs,
                "fees": individual.fees,
                "fee_per_recipient": per_recipient(individual.fees),
                "blocks_consumed": individual.blocks_consumed,
                "tally": individual.tally,
            }),
        );
        // Both arms have to have paid every recipient before either headline means anything.
        let comparable = batch.is_some_and(|b| b.complete()) && individual.complete();
        if let Some(batch) = batch {
            if comparable {
                if batch.duration_secs > 0.0 {
                    report.note(
                        "throughput_multiplier",
                        individual.duration_secs / batch.duration_secs,
                    );
                }
                if batch.fees > 0 {
                    report.note(
                        "fee_multiplier",
                        #[allow(clippy::cast_precision_loss)]
                        {
                            individual.fees as f64 / batch.fees as f64
                        },
                    );
                }
            } else {
                report.note(
                    "comparison_withheld",
                    format!(
                        "batch confirmed {} of {} and individual {} of {}; a headline from an \
                         arm that did not pay every recipient would be meaningless",
                        batch.tally.confirmed, batch.txs, individual.tally.confirmed, individual.txs
                    ),
                );
            }
        }
        report.note("recipients", params.s5_m);
        report.note("outputs_per_batch_tx", params.s5_k);
        report.note("tally", tally);

        // Unlike the self-sends elsewhere, S5 pays strangers: both the amounts and the fees
        // leave the wallet, and only transactions that were actually broadcast count.
        let spent: u64 = report
            .transactions
            .iter()
            .filter(|m| m.outcome != crate::metrics::TxOutcome::Failed)
            .map(|m| m.amount + m.fee)
            .sum();
        report.note("amount_sent", spent.saturating_sub(report.fees_paid));
        report.reconcile(report.balance_start.saturating_sub(spent));
        Ok(report)
    }
}

/// Runs one arm: submit back-to-back, then wait for every transaction to confirm.
async fn run_arm(
    mode: &Arc<dyn WalletMode>,
    ctx: &RunContext,
    report: &mut ScenarioReport,
    recipients: &[String],
    outputs_per_tx: usize,
    prefix: &str,
) -> Result<ArmResult> {
    let params = &ctx.cfg.params;

    // Start the arm with a wallet that will spend: an arm timed from before the previous one
    // settled would be timing the wallet catching up, not the payments.
    let settled = wait_until_spendable(
        mode,
        SpendRequirement {
            utxos: 1,
            amount: params.s5_amount * outputs_per_tx as u64,
        },
        params.confirm_timeout(),
        params.poll_interval(),
    )
    .await;
    report.note(&format!("{prefix}_settle_secs"), settled.secs);

    let tip_start = ctx.chain.tip_height().await.unwrap_or(0);
    let arm_started = Instant::now();
    let first_index = report.transactions.len();

    let mut pending = Vec::new();
    for (i, chunk) in recipients.chunks(outputs_per_tx).enumerate() {
        let outputs = chunk
            .iter()
            .map(|address| OutputSpec {
                address: address.clone(),
                amount: params.s5_amount,
            })
            .collect();
        let request = SendRequest {
            label: format!("{prefix}-tx{i}"),
            outputs,
            fee_per_gram: params.fee_rate,
        };
        let (metric, tracker) = send_recorded(mode, request, arm_started).await;
        let index = report.transactions.len();
        report.transactions.push(metric);
        if let Some(tracker) = tracker {
            pending.push(Pending { index, tracker });
        }
    }

    await_confirmations(
        mode,
        &pending,
        &mut report.transactions,
        params.c_min,
        params.confirm_timeout(),
        params.poll_interval(),
        arm_started,
    )
    .await;

    let duration_secs = arm_started.elapsed().as_secs_f64();
    let tip_end = ctx.chain.tip_height().await.unwrap_or(tip_start);
    let metrics = &report.transactions[first_index..];
    let result = ArmResult {
        duration_secs,
        fees: metrics.iter().map(|m| m.fee).sum(),
        tally: TxTally::of(metrics),
        blocks_consumed: tip_end.saturating_sub(tip_start),
        txs: metrics.len(),
    };
    let errors = distinct_errors(metrics, 5);
    if !errors.is_empty() {
        report.note(&format!("{prefix}_errors"), errors);
    }
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::{ArmResult, TxTally};

    fn arm(txs: usize, confirmed: u32, failed: u32) -> ArmResult {
        ArmResult {
            duration_secs: 1.0,
            fees: 0,
            tally: TxTally {
                confirmed,
                failed,
                ..TxTally::default()
            },
            blocks_consumed: 0,
            txs,
        }
    }

    #[test]
    fn an_arm_that_paid_everyone_is_comparable() {
        assert!(arm(10, 10, 0).complete());
    }

    #[test]
    fn an_arm_that_failed_outright_is_not() {
        // The 25,000x multiplier came from exactly this: ten batch calls rejected in 14 ms.
        assert!(!arm(10, 0, 10).complete());
    }

    #[test]
    fn a_partly_completed_arm_is_not_comparable_either() {
        assert!(!arm(100, 6, 94).complete());
        assert!(!arm(0, 0, 0).complete());
    }
}
