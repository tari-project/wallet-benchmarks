//! S1 — UTXO build-up: six doubling rounds followed by one fan-out round.
//!
//! Each round spends every UTXO the wallet holds exactly once, splitting it into
//! `outputs_per_tx` pieces (the last piece being the change output). That produces the
//! on-chain history the scan scenarios need and measures per-transaction cost on the way.
//!
//! A round that does not land in full halts the scenario, as the protocol requires: doubling
//! cannot continue from an unknown UTXO set.

use std::{ops::Range, sync::Arc, time::Instant};

use anyhow::Result;
use async_trait::async_trait;
use serde::Serialize;

use crate::{
    metrics::ScenarioReport,
    modes::{Balance, OutputSpec, SendRequest, WalletMode},
    scenarios::{
        support::{
            await_confirmations,
            distinct_errors,
            fee_source,
            fees_of,
            send_recorded,
            split_value,
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

pub struct UtxoBuildUp {
    /// The rounds as they will be reported.
    ///
    /// Held rather than written straight into the report because for a mode that cannot
    /// report a fee at send time the fees — and the value-conservation check that reads them —
    /// are only knowable after enrichment has run.
    rounds: Vec<RoundNote>,
    /// Whether the mode this ran against priced its own transactions as it sent them.
    fees_known_at_send: bool,
}

impl Default for UtxoBuildUp {
    fn default() -> Self {
        Self {
            rounds: Vec::new(),
            // Until a run says otherwise: every mode but the payment processor does.
            fees_known_at_send: true,
        }
    }
}

/// One round's line in the `rounds` note.
#[derive(Debug, Clone, Serialize)]
struct RoundNote {
    round: u32,
    txs: u64,
    outputs_per_tx: u32,
    expected_utxos_after: u64,
    observed_utxos_after: u64,
    wall_clock_secs: f64,
    /// How long the wallet took, after the previous round confirmed, to be willing to spend
    /// again.
    settle_secs: f64,
    spendable_utxos_at_start: u64,
    available_at_start: u64,
    fees: u64,
    /// One of [`fee_source`]: which of the two ways this round's fee figure was arrived at.
    fees_source: &'static str,
    balance_before: u64,
    balance_after: u64,
    balance_delta: i128,
    /// The spendable portion trails the total by a scan pass, so it is recorded rather than
    /// checked: a round whose change has not been marked confirmed yet shows up here as a low
    /// `available` and a correct `balance_after`.
    balance_after_breakdown: Balance,
    tally: TxTally,
    errors: Vec<String>,
    /// Half-open range of this round's transactions in the report, so a reader can find them
    /// and so the deferred fee check knows which ones to add up.
    tx_index_start: usize,
    tx_index_end: usize,
}

impl RoundNote {
    fn range(&self) -> Range<usize> {
        self.tx_index_start..self.tx_index_end
    }

    /// Applies a fee figure to the round and recomputes what is derived from it.
    fn with_fees(&mut self, fees: u64, source: &'static str) {
        self.fees = fees;
        self.fees_source = source;
        self.balance_delta =
            i128::from(self.balance_before) - i128::from(self.balance_after) - i128::from(fees);
    }
}

/// The round conserved value, or the reason it looks like it did not.
///
/// Every S1 round is self-sends, so the wallet's total moves by exactly the fees it burned.
/// That only holds as a check while the fee figure is the real one: a mode that cannot price a
/// transaction until it is on chain reports zero, and asserting against zero would turn "no
/// fee yet" into "the wallet lost money". So a round whose fees are not fully known is
/// reported as unchecked, with one thing still asserted — a self-send cannot leave the wallet
/// holding *more* than it started with, whatever the fee turns out to be.
fn conservation_failure(round: &RoundNote, fees_known: bool) -> Option<String> {
    if round.balance_after > round.balance_before {
        return Some(format!(
            "round {}: the wallet holds {} µT, more than the {} µT it started the round with,              and every transaction in it was a self-send",
            round.round, round.balance_after, round.balance_before
        ));
    }
    if !fees_known {
        return None;
    }
    if round.balance_before != round.balance_after + round.fees {
        return Some(format!(
            "round {}: the wallet holds {} µT, expected {} µT less {} µT of fees",
            round.round, round.balance_after, round.balance_before, round.fees
        ));
    }
    None
}

#[async_trait]
impl Scenario for UtxoBuildUp {
    fn id(&self) -> &'static str {
        "S1"
    }

    fn description(&self) -> &'static str {
        "Doubling rounds plus a fan-out round, building the wallet up to volume_target UTXOs"
    }

    #[allow(clippy::too_many_lines)]
    async fn run(
        &mut self,
        mode: &Arc<dyn WalletMode>,
        ctx: &mut RunContext,
    ) -> Result<ScenarioReport> {
        let params = ctx.cfg.params.clone();
        let mut report = ScenarioReport::new(self.id(), mode.name());
        report.tip_height_start = ctx.chain.tip_height().await.unwrap_or(0);

        let started = Instant::now();
        self.fees_known_at_send = mode.reports_fee_at_send();
        mode.sync_to_tip(params.scan_timeout()).await?;
        let address = mode.address().await?;
        report.balance_start = mode.balance().await?.total();
        report.utxos_start = mode.unspent_amounts().await?.len() as u64;

        // The protocol is written for a wallet funded with one UTXO, but the plan only needs
        // to know how many it is starting from: each round still spends every UTXO once.
        let start_utxos = report.utxos_start.max(1);
        let expected_utxos_end = params.expected_utxos_from(start_utxos);
        let mut plan = params.doubling_plan_from(start_utxos);
        plan.push(params.fanout_plan_from(start_utxos));
        if start_utxos != 1 {
            report.note(
                "scaled_plan",
                format!(
                    "the wallet was funded with {start_utxos} UTXOs, so the build-up runs to \
                     {expected_utxos_end} instead of the reference {}",
                    params.expected_utxos_after_s1()
                ),
            );
        }

        let mut halt: Option<String> = None;

        for round in plan {
            let round_started = Instant::now();
            let balance_before = mode.balance().await?.total();

            // The previous round's outputs are confirmed, but a wallet does not become
            // willing to spend them at the same instant. Wait for it to say so, and record
            // how long that took: the wait belongs to the round that needs it.
            let settled = wait_until_spendable(
                mode,
                SpendRequirement {
                    utxos: round.txs,
                    amount: 0,
                },
                params.confirm_timeout(),
                params.poll_interval(),
            )
            .await;
            if !settled.ready {
                halt = Some(format!(
                    "round {} needs {} spendable UTXOs; after {:.0}s the wallet offered {} \
                     (available {} µT)",
                    round.round,
                    round.txs,
                    settled.secs,
                    settled.spendable_utxos,
                    settled.available
                ));
                break;
            }

            let mut amounts = mode.spendable_amounts().await?;
            amounts.sort_unstable_by(|a, b| b.cmp(a));

            if (amounts.len() as u64) < round.txs {
                halt = Some(format!(
                    "round {} needs {} spendable UTXOs but the wallet offers {}",
                    round.round,
                    round.txs,
                    amounts.len()
                ));
                break;
            }

            // Spend the largest UTXOs first, one transaction per UTXO. Asking for half (or an
            // eighth) of a specific UTXO is the closest the harness can steer a wallet that
            // owns its own input selection; whatever the wallet actually picks is measured.
            let first_index = report.transactions.len();
            let mut pending = Vec::new();
            for (i, value) in amounts.iter().take(round.txs as usize).enumerate() {
                let label = format!("s1-r{}-tx{}", round.round, i);
                let Some(each) = split_value(*value, round.outputs_per_tx) else {
                    let mut metric = crate::metrics::TxMetric {
                        label,
                        outcome: crate::metrics::TxOutcome::Failed,
                        ..crate::metrics::TxMetric::default()
                    };
                    metric.error = Some(format!(
                        "UTXO of {value} µT cannot be split into {} outputs",
                        round.outputs_per_tx
                    ));
                    report.transactions.push(metric);
                    continue;
                };
                // The change output is the final piece of the split, so only
                // `outputs_per_tx - 1` payment outputs are requested.
                let outputs = (0..round.outputs_per_tx - 1)
                    .map(|_| OutputSpec {
                        address: address.clone(),
                        amount: each,
                    })
                    .collect();
                let request = SendRequest {
                    label,
                    outputs,
                    fee_per_gram: params.fee_rate,
                };
                let (metric, tracker) = send_recorded(mode, request, round_started).await;
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
                round_started,
            )
            .await;

            // Tip refresh, not a full rescan: the wallet only needs to see this round land.
            if let Err(e) = mode.sync_to_tip(params.scan_timeout()).await {
                halt = Some(format!(
                    "round {} could not refresh wallet state: {e}",
                    round.round
                ));
                break;
            }

            let tx_index_end = report.transactions.len();
            let (tally, round_fees, errors) = {
                let round_metrics = &report.transactions[first_index..];
                (
                    TxTally::of(round_metrics),
                    fees_of(round_metrics),
                    distinct_errors(round_metrics, 3),
                )
            };
            let balance = mode.balance().await?;
            let balance_after = balance.total();
            let utxos_after = mode.unspent_amounts().await?.len() as u64;

            let mut note = RoundNote {
                round: round.round,
                txs: round.txs,
                outputs_per_tx: round.outputs_per_tx,
                expected_utxos_after: round.utxos_after,
                observed_utxos_after: utxos_after,
                wall_clock_secs: round_started.elapsed().as_secs_f64(),
                settle_secs: settled.secs,
                spendable_utxos_at_start: settled.spendable_utxos,
                available_at_start: settled.available,
                fees: 0,
                fees_source: fee_source::PENDING,
                balance_before,
                balance_after,
                balance_delta: 0,
                balance_after_breakdown: balance,
                tally,
                errors,
                tx_index_start: first_index,
                tx_index_end,
            };
            if self.fees_known_at_send {
                note.with_fees(round_fees.total, fee_source::AT_SEND);
                // Self-sends, so the round conserves value apart from the fees it burned.
                if let Some(reason) = conservation_failure(&note, round_fees.complete) {
                    report.fail(reason);
                }
            }
            self.rounds.push(note);

            if tally.unsuccessful() > 0 {
                halt = Some(format!(
                    "round {} did not land in full: {} of {} transactions confirmed",
                    round.round, tally.confirmed, round.txs
                ));
                break;
            }
            if utxos_after != round.utxos_after {
                halt = Some(format!(
                    "round {} ended on {} UTXOs, expected {}",
                    round.round, utxos_after, round.utxos_after
                ));
                break;
            }
        }

        let tally = TxTally::of(&report.transactions);
        report.wall_clock_secs = started.elapsed().as_secs_f64();
        report.tip_height_end = ctx.chain.tip_height().await.unwrap_or(0);
        report.fees_paid = report.total_recorded_fees();
        report.balance_end = mode.balance().await?.total();
        report.utxos_end = mode.unspent_amounts().await?.len() as u64;
        report.successes = tally.confirmed;
        report.failures = tally.unsuccessful();
        report.note("rounds", &self.rounds);
        if !self.fees_known_at_send {
            report.note(
                "fees_deferred",
                "this mode reports no fee at send time, so the per-round fees and the \
                 value-conservation check are computed once enrichment has read the transactions \
                 back off the chain",
            );
        }
        report.note("tally", tally);
        report.note("start_utxos", start_utxos);
        report.note("expected_utxos", expected_utxos_end);
        report.note("reference_expected_utxos", params.expected_utxos_after_s1());
        report.reconcile(report.balance_start.saturating_sub(report.fees_paid));

        if let Some(reason) = halt {
            report.fail(reason);
        } else if report.utxos_end != expected_utxos_end {
            report.fail(format!(
                "S1 ended on {} UTXOs, expected {expected_utxos_end}",
                report.utxos_end
            ));
        }
        Ok(report)
    }

    fn finalise(&mut self, report: &mut ScenarioReport) {
        if self.fees_known_at_send || self.rounds.is_empty() {
            return;
        }

        // Read every round's fees first: the checks below need the report mutably.
        let fees: Vec<Fees> = self
            .rounds
            .iter()
            .map(|round| fees_of(report.transactions.get(round.range()).unwrap_or_default()))
            .collect();

        let mut unchecked = Vec::new();
        for (round, fees) in self.rounds.iter_mut().zip(fees) {
            let source = if fees.complete {
                fee_source::ENRICHMENT
            } else {
                fee_source::ENRICHMENT_PARTIAL
            };
            round.with_fees(fees.total, source);
            if !fees.complete {
                unchecked.push(round.round);
            }
        }

        for round in &self.rounds {
            let fees_known = round.fees_source == fee_source::ENRICHMENT;
            if let Some(reason) = conservation_failure(round, fees_known) {
                report.fail(reason);
            }
        }

        report.note("rounds", &self.rounds);
        if !unchecked.is_empty() {
            // Said out loud rather than failed: enrichment recovers a fee through the payref a
            // payment only gets once it has confirmed, so a round holding a transaction that
            // never confirmed has no fee to check against and never will.
            report.note(
                "value_conservation_unchecked",
                serde_json::json!({
                    "rounds": unchecked,
                    "reason": "not every transaction in these rounds has a fee, so the balance \
                               they should have ended on is unknown; only the direction of the \
                               balance change was checked",
                }),
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        metrics::{Outcome, TxMetric, TxOutcome},
        modes::PAYMENT_PROCESSOR,
    };

    /// A round of two self-sends that started on 1,000,000 µT and ended on `balance_after`.
    fn round(balance_after: u64) -> RoundNote {
        RoundNote {
            round: 1,
            txs: 2,
            outputs_per_tx: 2,
            expected_utxos_after: 4,
            observed_utxos_after: 4,
            wall_clock_secs: 12.0,
            settle_secs: 1.0,
            spendable_utxos_at_start: 2,
            available_at_start: 1_000_000,
            fees: 0,
            fees_source: fee_source::PENDING,
            balance_before: 1_000_000,
            balance_after,
            balance_delta: 0,
            balance_after_breakdown: Balance::default(),
            tally: TxTally {
                confirmed: 2,
                ..TxTally::default()
            },
            errors: Vec::new(),
            tx_index_start: 0,
            tx_index_end: 2,
        }
    }

    fn confirmed(fee: u64) -> TxMetric {
        TxMetric {
            label: "s1-r1-tx0".to_string(),
            fee,
            outcome: TxOutcome::Confirmed,
            ..TxMetric::default()
        }
    }

    /// S1 as it stands after a run against a mode that could not price its own sends, with
    /// `fees` the fee each of the round's two transactions turned out to have paid.
    fn after_a_deferred_run(balance_after: u64, fees: [u64; 2]) -> (UtxoBuildUp, ScenarioReport) {
        let scenario = UtxoBuildUp {
            rounds: vec![round(balance_after)],
            fees_known_at_send: false,
        };
        let mut report = ScenarioReport::new(scenario.id(), PAYMENT_PROCESSOR);
        report.transactions = fees.iter().copied().map(confirmed).collect();
        // What `run` wrote: no fee was knowable, so the round is reported with none.
        report.note("rounds", &scenario.rounds);
        (scenario, report)
    }

    fn round_note(report: &ScenarioReport) -> serde_json::Value {
        report.notes["rounds"][0].clone()
    }

    #[test]
    fn a_round_conserves_value_apart_from_the_fees_it_burned() {
        let mut note = round(999_900);
        note.with_fees(100, fee_source::AT_SEND);
        assert_eq!(conservation_failure(&note, true), None);
        assert_eq!(note.balance_delta, 0);
    }

    #[test]
    fn a_round_that_lost_more_than_its_fees_is_a_failure() {
        let mut note = round(999_000);
        note.with_fees(100, fee_source::AT_SEND);
        let reason = conservation_failure(&note, true).expect("900 µT went missing");
        assert!(reason.contains("999000"), "unexpected reason: {reason}");
        assert_eq!(note.balance_delta, 900);
    }

    #[test]
    fn a_mode_that_cannot_price_its_sends_is_not_failed_for_reporting_no_fee() {
        // The bug this replaces: `payment_processor` returns fee 0 from every send, so the
        // round always looked like it had lost its whole fee spend, S1 failed, and every
        // scenario after it on that mode was skipped.
        let (mut scenario, mut report) = after_a_deferred_run(999_900, [0, 0]);
        assert_eq!(round_note(&report)["fees"], 0);
        assert_eq!(
            report.outcome,
            Outcome::Ok,
            "run() must not check this itself"
        );

        // Enrichment finds the two fees the processor could not report.
        report.transactions[0].fee = 60;
        report.transactions[1].fee = 40;
        scenario.finalise(&mut report);

        assert_eq!(report.outcome, Outcome::Ok, "{:?}", report.failure_reasons);
        assert_eq!(round_note(&report)["fees"], 100);
        assert_eq!(round_note(&report)["fees_source"], fee_source::ENRICHMENT);
        assert_eq!(round_note(&report)["balance_delta"], 0);
        assert!(!report.notes.contains_key("value_conservation_unchecked"));
    }

    #[test]
    fn the_deferred_check_still_catches_a_real_mismatch() {
        // Same mode, same deferral, but the wallet is 900 µT short of what its fees explain.
        let (mut scenario, mut report) = after_a_deferred_run(999_000, [60, 40]);
        scenario.finalise(&mut report);

        assert_eq!(report.outcome, Outcome::Failed);
        let reason = report.error.as_deref().unwrap_or_default();
        assert!(reason.contains("999000"), "unexpected reason: {reason}");
        assert!(
            reason.contains("100 µT of fees"),
            "unexpected reason: {reason}"
        );
    }

    #[test]
    fn a_round_whose_fees_never_arrived_is_reported_unchecked_rather_than_failed() {
        // A confirmed transaction with no fee against it is one enrichment could not reach —
        // no payref, or the daemon had no stored copy. The balance it should have ended on is
        // then unknown, and asserting on it would invent a failure.
        let (mut scenario, mut report) = after_a_deferred_run(999_000, [60, 0]);
        scenario.finalise(&mut report);

        assert_eq!(report.outcome, Outcome::Ok, "{:?}", report.failure_reasons);
        assert_eq!(
            round_note(&report)["fees_source"],
            fee_source::ENRICHMENT_PARTIAL
        );
        assert_eq!(
            report.notes["value_conservation_unchecked"]["rounds"],
            serde_json::json!([1])
        );
    }

    #[test]
    fn a_round_that_gained_value_fails_whether_or_not_its_fees_are_known() {
        // The one thing that holds without a fee figure: paying yourself cannot leave you
        // richer, so this is checked on both paths.
        let mut note = round(1_000_001);
        assert!(conservation_failure(&note, false).is_some());
        note.with_fees(100, fee_source::AT_SEND);
        assert!(conservation_failure(&note, true).is_some());

        let (mut scenario, mut report) = after_a_deferred_run(1_000_001, [60, 40]);
        scenario.finalise(&mut report);
        assert_eq!(report.outcome, Outcome::Failed);
        assert!(
            report.error.as_deref().unwrap_or_default().contains("more"),
            "unexpected reason: {:?}",
            report.error
        );
    }

    #[test]
    fn a_mode_that_prices_its_sends_has_nothing_left_to_do_afterwards() {
        let mut scenario = UtxoBuildUp {
            rounds: vec![round(999_000)],
            fees_known_at_send: true,
        };
        let mut report = ScenarioReport::new(scenario.id(), "new_wallet");
        report.transactions = vec![confirmed(60), confirmed(40)];
        // A mismatched round on this path was already failed by `run`; `finalise` must not
        // rewrite the notes or fail it a second time.
        scenario.finalise(&mut report);
        assert_eq!(report.outcome, Outcome::Ok);
        assert!(report.notes.is_empty());
    }
}
