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
            await_confirmations,
            distinct_errors,
            fee_source,
            fees_of,
            send_recorded,
            wait_until_spendable,
            Fees,
            Pending,
            SpendRequirement,
            TxTally,
        },
        RunContext,
        Scenario,
    },
};

/// What one arm of the comparison cost.
#[derive(Debug, Clone, Copy)]
struct ArmResult {
    duration_secs: f64,
    fees: u64,
    /// False when a transaction this arm broadcast has no fee against it, which makes `fees` a
    /// floor: the arm cost at least that, and nothing may be divided by it.
    fees_complete: bool,
    tally: TxTally,
    blocks_consumed: u64,
    txs: usize,
    /// Half-open range of this arm's transactions in the report, so the fee figure can be
    /// recomputed once enrichment has supplied fees the mode could not report at send time.
    tx_index_start: usize,
    tx_index_end: usize,
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

    fn range(&self) -> std::ops::Range<usize> {
        self.tx_index_start..self.tx_index_end
    }

    /// Takes both halves of a fee figure, so a total can never be stored without what says
    /// whether it is one.
    fn record_fees(&mut self, fees: Fees) {
        self.fees = fees.total;
        self.fees_complete = fees.complete;
    }
}

pub struct PaymentThroughput {
    /// The two arms, kept so the fee comparison can be recomputed once enrichment has supplied
    /// the fees. Without it S5's headline number — whether batching is cheaper — is computed
    /// against zero for the one mode whose whole feature is payment batching.
    arms: Option<Arms>,
    /// Whether the mode this ran against priced its own transactions as it sent them.
    fees_known_at_send: bool,
}

impl Default for PaymentThroughput {
    fn default() -> Self {
        Self {
            arms: None,
            // Until a run says otherwise: every mode but the payment processor does.
            fees_known_at_send: true,
        }
    }
}

/// Both arms, with the parameters the notes are written against.
struct Arms {
    /// `None` when the mode cannot put `K` outputs in one transaction, so there is no
    /// comparison to make.
    batch: Option<ArmResult>,
    individual: ArmResult,
    recipients: u32,
    outputs_per_batch_tx: u32,
}

#[async_trait]
impl Scenario for PaymentThroughput {
    fn id(&self) -> &'static str {
        "S5"
    }

    fn description(&self) -> &'static str {
        "Pay M recipients twice: once as M/K batch transactions, once as M individual sends"
    }

    async fn run(
        &mut self,
        mode: &Arc<dyn WalletMode>,
        ctx: &mut RunContext,
    ) -> Result<ScenarioReport> {
        let params = ctx.cfg.params.clone();
        let network = ctx.cfg.tari_network()?;
        let mut report = ScenarioReport::new(self.id(), mode.name());
        report.tip_height_start = ctx.chain.tip_height().await.unwrap_or(0);

        let started = Instant::now();
        self.fees_known_at_send = mode.reports_fee_at_send();
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

        let arms = Arms {
            batch,
            individual,
            recipients: params.s5_m,
            outputs_per_batch_tx: params.s5_k,
        };
        publish_arm_notes(
            &arms,
            &mut report,
            if self.fees_known_at_send {
                fee_source::AT_SEND
            } else {
                fee_source::PENDING
            },
        );
        self.arms = Some(arms);
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

    fn finalise(&mut self, report: &mut ScenarioReport) {
        if self.fees_known_at_send {
            return;
        }
        let Some(arms) = &mut self.arms else {
            return;
        };
        // The whole point of the scenario for this mode: `fee_multiplier` is the answer to
        // "does batching save fees", and until enrichment has read the fees off the chain
        // both arms cost zero and the comparison is never published.
        if let Some(batch) = &mut arms.batch {
            batch.record_fees(fees_of(
                report.transactions.get(batch.range()).unwrap_or_default(),
            ));
        }
        let individual = fees_of(
            report
                .transactions
                .get(arms.individual.range())
                .unwrap_or_default(),
        );
        arms.individual.record_fees(individual);
        publish_arm_notes(arms, report, fee_source::ENRICHMENT);
    }
}

/// Writes both arms' notes and the two headline multipliers.
///
/// Called once while the scenario runs and, for a mode that could not price its own sends,
/// again once enrichment has: every figure here is derived from the arms' fees, so the second
/// call is what makes them mean anything for that mode.
fn publish_arm_notes(arms: &Arms, report: &mut ScenarioReport, fees_source: &'static str) {
    // Every note below is rewritten from scratch. On the second call the arms cost something
    // they did not cost on the first, so a note the first pass wrote and this one will not —
    // "no fee was recorded against the batch arm" — has to go, or the profile carries both a
    // fee multiplier and the reason there is no fee multiplier.
    for stale in [
        "t_batch_secs",
        "t_individual_secs",
        "throughput_multiplier",
        "fee_multiplier",
        "comparison_withheld",
        "fee_comparison_withheld",
    ] {
        report.notes.remove(stale);
    }

    #[allow(clippy::cast_precision_loss)]
    let per_recipient = |fees: u64| fees as f64 / f64::from(arms.recipients);
    // "Recovered from the chain" is a claim about every transaction in the arm. An arm that
    // enrichment could only price part of gets the label that says so, exactly as S1's rounds
    // do, so `fees` and `fee_per_recipient` next to it read as the floors they are.
    let source_of = |arm: &ArmResult| {
        if fees_source == fee_source::ENRICHMENT && !arm.fees_complete {
            fee_source::ENRICHMENT_PARTIAL
        } else {
            fees_source
        }
    };
    let individual = &arms.individual;
    report.note("recipients", arms.recipients);
    report.note("outputs_per_batch_tx", arms.outputs_per_batch_tx);

    if let Some(batch) = &arms.batch {
        if batch.complete() {
            report.note("t_batch_secs", batch.duration_secs);
        }
        report.note(
            "arm_a_batch",
            json!({
                "txs": batch.txs,
                "outputs_per_tx": arms.outputs_per_batch_tx,
                "duration_secs": batch.duration_secs,
                "fees": batch.fees,
                "fees_source": source_of(batch),
                "fees_complete": batch.fees_complete,
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
            "fees_source": source_of(individual),
            "fees_complete": individual.fees_complete,
            "fee_per_recipient": per_recipient(individual.fees),
            "blocks_consumed": individual.blocks_consumed,
            "tally": individual.tally,
        }),
    );

    // Both arms have to have paid every recipient before either headline means anything.
    let Some(batch) = &arms.batch else {
        return;
    };
    if !(batch.complete() && individual.complete()) {
        report.note(
            "comparison_withheld",
            format!(
                "batch confirmed {} of {} and individual {} of {}; a headline from an arm that \
                 did not pay every recipient would be meaningless",
                batch.tally.confirmed, batch.txs, individual.tally.confirmed, individual.txs
            ),
        );
        return;
    }
    if batch.duration_secs > 0.0 {
        report.note(
            "throughput_multiplier",
            individual.duration_secs / batch.duration_secs,
        );
    }
    if batch.fees == 0 {
        // Zero here is "not known yet" for a mode whose fees arrive with enrichment, and "no
        // transaction was priced" for one whose fees do not. Either way the ratio would be a
        // division by zero, and the reason it is missing belongs in the profile.
        report.note(
            "fee_comparison_withheld",
            format!("the batch arm has no fee recorded against it (fees_source {fees_source})"),
        );
    } else if !(batch.fees_complete && individual.fees_complete) {
        // A floor divided by a floor is not a ratio, and this ratio is the number the whole
        // scenario exists to produce. Which way it would be wrong is not even knowable: a
        // missing batch fee overstates the saving and a missing individual fee understates it,
        // so the ratio is withheld rather than published with a caveat. Checked whatever the
        // fees' source, because a fee the sends themselves failed to report is as much a floor
        // as one enrichment could not recover.
        report.note(
            "fee_comparison_withheld",
            format!(
                "{} has a transaction with no fee against it, so that arm's total is a floor \
                 rather than what it cost; a ratio of floors would not say whether batching is \
                 cheaper (fees_source {fees_source})",
                match (batch.fees_complete, individual.fees_complete) {
                    (false, false) => "neither arm",
                    (false, true) => "the batch arm",
                    _ => "the individual arm",
                }
            ),
        );
    } else {
        report.note(
            "fee_multiplier",
            #[allow(clippy::cast_precision_loss)]
            {
                individual.fees as f64 / batch.fees as f64
            },
        );
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
    let fees = fees_of(metrics);
    let result = ArmResult {
        duration_secs,
        fees: fees.total,
        fees_complete: fees.complete,
        tally: TxTally::of(metrics),
        blocks_consumed: tip_end.saturating_sub(tip_start),
        txs: metrics.len(),
        tx_index_start: first_index,
        tx_index_end: first_index + metrics.len(),
    };
    let errors = distinct_errors(metrics, 5);
    if !errors.is_empty() {
        report.note(&format!("{prefix}_errors"), errors);
    }
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        metrics::{TxMetric, TxOutcome},
        modes::PAYMENT_PROCESSOR,
    };

    fn arm(txs: usize, confirmed: u32, failed: u32) -> ArmResult {
        ArmResult {
            duration_secs: 1.0,
            fees: 0,
            fees_complete: false,
            tally: TxTally {
                confirmed,
                failed,
                ..TxTally::default()
            },
            blocks_consumed: 0,
            txs,
            tx_index_start: 0,
            tx_index_end: txs,
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

    fn confirmed(fee: u64) -> TxMetric {
        TxMetric {
            fee,
            outcome: TxOutcome::Confirmed,
            ..TxMetric::default()
        }
    }

    /// Four recipients paid twice: two batch transactions of two, then four singles.
    fn after_a_deferred_run() -> (PaymentThroughput, ScenarioReport) {
        let mut batch = arm(2, 2, 0);
        batch.tx_index_start = 0;
        batch.tx_index_end = 2;
        let mut individual = arm(4, 4, 0);
        individual.tx_index_start = 2;
        individual.tx_index_end = 6;

        let scenario = PaymentThroughput {
            arms: Some(Arms {
                batch: Some(batch),
                individual,
                recipients: 4,
                outputs_per_batch_tx: 2,
            }),
            fees_known_at_send: false,
        };
        let mut report = ScenarioReport::new("S5", PAYMENT_PROCESSOR);
        report.transactions = vec![confirmed(0); 6];
        publish_arm_notes(
            scenario.arms.as_ref().unwrap(),
            &mut report,
            fee_source::PENDING,
        );
        (scenario, report)
    }

    #[test]
    fn the_fee_comparison_only_exists_once_enrichment_has_supplied_the_fees() {
        let (mut scenario, mut report) = after_a_deferred_run();
        // What the mode looked like before: the headline S5 exists to compute was never
        // published, because both arms cost zero.
        assert_eq!(report.notes["arm_a_batch"]["fees"], 0);
        assert!(!report.notes.contains_key("fee_multiplier"));
        assert!(report.notes.contains_key("fee_comparison_withheld"));

        // Two batch transactions at 50 µT each against four singles at 40 µT each.
        for (index, metric) in report.transactions.iter_mut().enumerate() {
            metric.fee = if index < 2 { 50 } else { 40 };
        }
        scenario.finalise(&mut report);

        assert_eq!(report.notes["arm_a_batch"]["fees"], 100);
        assert_eq!(report.notes["arm_a_batch"]["fee_per_recipient"], 25.0);
        assert_eq!(
            report.notes["arm_a_batch"]["fees_source"],
            fee_source::ENRICHMENT
        );
        assert_eq!(report.notes["arm_b_individual"]["fees"], 160);
        assert_eq!(report.notes["arm_b_individual"]["fee_per_recipient"], 40.0);
        assert_eq!(report.notes["fee_multiplier"], 1.6);
        assert_eq!(report.notes["arm_a_batch"]["fees_complete"], true);
        assert!(!report.notes.contains_key("fee_comparison_withheld"));
    }

    #[test]
    fn a_multiplier_is_not_computed_from_a_fee_enrichment_could_only_partly_recover() {
        let (mut scenario, mut report) = after_a_deferred_run();
        // Everything priced except one of the four individual sends. Taken at face value that
        // reads as batching being 1.2x cheaper; the real ratio, once the missing 40 µT is
        // counted, is 1.6x. The undercount is the whole finding.
        for (index, metric) in report.transactions.iter_mut().enumerate() {
            metric.fee = match index {
                0 | 1 => 50,
                5 => 0,
                _ => 40,
            };
        }
        scenario.finalise(&mut report);

        assert_eq!(report.notes["arm_b_individual"]["fees"], 120);
        assert_eq!(
            report.notes["arm_b_individual"]["fees_source"],
            fee_source::ENRICHMENT_PARTIAL
        );
        assert_eq!(report.notes["arm_b_individual"]["fees_complete"], false);
        // The batch arm was priced in full, so it keeps the label that says so.
        assert_eq!(
            report.notes["arm_a_batch"]["fees_source"],
            fee_source::ENRICHMENT
        );
        assert!(
            !report.notes.contains_key("fee_multiplier"),
            "1.2x is not the answer, and neither is any other number derived from a floor"
        );
        let withheld = report.notes["fee_comparison_withheld"].as_str().unwrap();
        assert!(
            withheld.contains("the individual arm"),
            "the profile has to name the arm that is short: {withheld}"
        );
    }

    #[test]
    fn an_arm_missing_a_fee_is_still_reported_as_the_floor_it_is() {
        // Withholding the ratio is not the same as withholding the arm: what was recovered is
        // still worth having, as long as nothing reads it as the total.
        let (mut scenario, mut report) = after_a_deferred_run();
        report.transactions[0].fee = 50;
        for metric in report.transactions.iter_mut().skip(2) {
            metric.fee = 40;
        }
        scenario.finalise(&mut report);

        assert_eq!(report.notes["arm_a_batch"]["fees"], 50);
        assert_eq!(report.notes["arm_a_batch"]["fee_per_recipient"], 12.5);
        assert_eq!(report.notes["arm_a_batch"]["fees_complete"], false);
        assert_eq!(
            report.notes["arm_a_batch"]["fees_source"],
            fee_source::ENRICHMENT_PARTIAL
        );
        assert!(!report.notes.contains_key("fee_multiplier"));
        assert!(report.notes["fee_comparison_withheld"]
            .as_str()
            .unwrap()
            .contains("the batch arm"));
    }

    #[test]
    fn the_recipient_set_is_reported_even_when_no_headline_can_be() {
        // These describe what was attempted, so they cannot depend on the comparison working.
        let (_, report) = after_a_deferred_run();
        assert_eq!(report.notes["recipients"], 4);
        assert_eq!(report.notes["outputs_per_batch_tx"], 2);
    }

    #[test]
    fn a_mode_that_prices_its_sends_keeps_the_comparison_it_already_published() {
        let mut scenario = PaymentThroughput {
            fees_known_at_send: true,
            ..PaymentThroughput::default()
        };
        let mut report = ScenarioReport::new("S5", "new_wallet");
        scenario.finalise(&mut report);
        assert!(report.notes.is_empty(), "there is nothing to recompute");
    }
}
