//! S1 — UTXO build-up: six doubling rounds followed by one fan-out round.
//!
//! Each round spends every UTXO the wallet holds exactly once, splitting it into
//! `outputs_per_tx` pieces (the last piece being the change output). That produces the
//! on-chain history the scan scenarios need and measures per-transaction cost on the way.
//!
//! A round that does not land in full halts the scenario, as the protocol requires: doubling
//! cannot continue from an unknown UTXO set.

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
            send_recorded,
            split_value,
            wait_until_spendable,
            Pending,
            SpendRequirement,
            TxTally,
        },
        RunContext,
        Scenario,
    },
};

pub struct UtxoBuildUp;

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
        &self,
        mode: &Arc<dyn WalletMode>,
        ctx: &mut RunContext,
    ) -> Result<ScenarioReport> {
        let params = ctx.cfg.params.clone();
        let mut report = ScenarioReport::new(self.id(), mode.name());
        report.tip_height_start = ctx.chain.tip_height().await.unwrap_or(0);

        let started = Instant::now();
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

        let mut round_notes = Vec::new();
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

            let (tally, round_fees, errors) = {
                let round_metrics = &report.transactions[first_index..];
                (
                    TxTally::of(round_metrics),
                    round_metrics.iter().map(|m| m.fee).sum::<u64>(),
                    distinct_errors(round_metrics, 3),
                )
            };
            let balance = mode.balance().await?;
            let balance_after = balance.total();
            let utxos_after = mode.unspent_amounts().await?.len() as u64;

            round_notes.push(json!({
                "round": round.round,
                "txs": round.txs,
                "outputs_per_tx": round.outputs_per_tx,
                "expected_utxos_after": round.utxos_after,
                "observed_utxos_after": utxos_after,
                "wall_clock_secs": round_started.elapsed().as_secs_f64(),
                // How long the wallet took, after the previous round confirmed, to be willing
                // to spend again.
                "settle_secs": settled.secs,
                "spendable_utxos_at_start": settled.spendable_utxos,
                "available_at_start": settled.available,
                "fees": round_fees,
                "balance_before": balance_before,
                "balance_after": balance_after,
                "balance_delta": i128::from(balance_before) - i128::from(balance_after) - i128::from(round_fees),
                // The spendable portion trails the total by a scan pass, so it is recorded
                // rather than checked: a round whose change has not been marked confirmed yet
                // shows up here as a low `available` and a correct `balance_after`.
                "balance_after_breakdown": balance,
                "tally": tally,
                "errors": errors,
            }));

            // Self-sends, so the round conserves value apart from the fees it burned.
            if balance_before != balance_after + round_fees {
                report.fail(format!(
                    "round {}: the wallet holds {} µT, expected {} µT less {} µT of fees",
                    round.round, balance_after, balance_before, round_fees
                ));
            }
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
        report.note("rounds", round_notes);
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
}
