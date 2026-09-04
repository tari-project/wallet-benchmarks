//! S4 — concurrent construction.
//!
//! For each level of the ramp, `N` construct-and-broadcast calls are fired at once and
//! nothing is done to help them succeed: no retries, no backoff, no UTXO pre-partitioning.
//! Locking, selection contention, stalls and rejections are the measurement.

use std::{sync::Arc, time::Instant};

use anyhow::Result;
use async_trait::async_trait;
use serde_json::json;

use crate::{
    metrics::{count_infrastructure_failures, ScenarioReport, TxMetric, TxOutcome},
    modes::{SendRequest, WalletMode},
    scenarios::{
        support::{
            await_confirmations,
            distinct_errors,
            max_serialization_gap,
            max_terminal_state_gap,
            wait_until_spendable,
            Pending,
            SpendRequirement,
            TxTally,
        },
        RunContext,
        Scenario,
    },
};

pub struct ConcurrentConstruction;

#[async_trait]
impl Scenario for ConcurrentConstruction {
    fn id(&self) -> &'static str {
        "S4"
    }

    fn description(&self) -> &'static str {
        "Fire N concurrent construct-and-broadcast calls per ramp level, with no retries"
    }

    async fn run(
        &self,
        mode: &Arc<dyn WalletMode>,
        ctx: &mut RunContext,
    ) -> Result<ScenarioReport> {
        let params = ctx.cfg.params.clone();
        let mut report = ScenarioReport::new(self.id(), mode.name());
        report.tip_height_start = ctx.chain.tip_height().await.unwrap_or(0);

        let started = Instant::now();
        // Whether the gaps between construction-complete events mean anything for this mode.
        let observes_construction = mode.observes_construction();
        mode.sync_to_tip(params.scan_timeout()).await?;
        let address = mode.address().await?;
        report.balance_start = mode.balance().await?.total();
        report.utxos_start = mode.unspent_amounts().await?.len() as u64;

        let mut level_notes = Vec::new();

        for level in &params.concurrent_batches {
            let level = *level;
            // Enter each level with a wallet that is willing to spend at least once. Without
            // this the first calls of a level fail because the previous level has not settled
            // yet, and that lag would be counted as concurrency contention. Nothing beyond
            // one spendable UTXO is waited for: the contention is the measurement.
            let settled = wait_until_spendable(
                mode,
                SpendRequirement {
                    utxos: 1,
                    amount: params.s4_amount,
                },
                params.confirm_timeout(),
                params.poll_interval(),
            )
            .await;
            let utxos_before = mode.unspent_amounts().await?.len() as u64;
            let batch_started = Instant::now();
            let first_index = report.transactions.len();

            // Every call goes out at once. Distinct recipients are unnecessary here — the
            // spec allows self-sends — but each call is independent.
            let mut handles = Vec::with_capacity(level as usize);
            for i in 0..level {
                let mode = Arc::clone(mode);
                let request = SendRequest::single(
                    format!("s4-n{level}-tx{i}"),
                    &address,
                    params.s4_amount,
                    params.fee_rate,
                );
                handles.push(tokio::spawn(async move {
                    crate::scenarios::support::send_recorded(&mode, request, batch_started).await
                }));
            }

            let mut pending = Vec::new();
            for handle in handles {
                match handle.await {
                    Ok((metric, tracker)) => {
                        let index = report.transactions.len();
                        report.transactions.push(metric);
                        if let Some(tracker) = tracker {
                            pending.push(Pending { index, tracker });
                        }
                    },
                    Err(e) => {
                        report.transactions.push(TxMetric {
                            label: format!("s4-n{level}-join-error"),
                            outcome: TxOutcome::Failed,
                            error: Some(e.to_string()),
                            ..TxMetric::default()
                        });
                    },
                }
            }
            let construction_wall_clock = batch_started.elapsed().as_secs_f64();

            // Construction itself is never cut short — the point of the scenario is how long
            // the wallet takes to push N transactions out, and truncating that would replace
            // the measurement with the budget. What is left of the budget goes to waiting for
            // them to confirm.
            let construction_secs = batch_started.elapsed().as_secs();
            let budget = params.s4_budget_secs.saturating_sub(construction_secs);
            let budget_exhausted = budget == 0;
            await_confirmations(
                mode,
                &pending,
                &mut report.transactions,
                params.c_min,
                std::time::Duration::from_secs(budget),
                params.poll_interval(),
                batch_started,
            )
            .await;

            let (tally, construction_gap, terminal_gap, fees, errors, infrastructure_failures) = {
                let metrics = &report.transactions[first_index..];
                (
                    TxTally::of(metrics),
                    max_serialization_gap(metrics),
                    max_terminal_state_gap(metrics),
                    metrics.iter().map(|m| m.fee).sum::<u64>(),
                    distinct_errors(metrics, 5),
                    count_infrastructure_failures(metrics),
                )
            };
            // A mode that cannot see when construction finished cannot be asked about the gaps
            // between constructions. `payment_processor` returns 202 in milliseconds and builds
            // the transaction later in a worker, so its construction gaps are gaps between
            // axum responses; the stream of terminal states is what is left to read its
            // internal serialisation from. Both are recorded, and so is which one the headline
            // came from.
            let (gap, gap_source) = if observes_construction {
                (construction_gap, "construction_complete")
            } else {
                (terminal_gap, "terminal_state")
            };

            // Let the wallet catch up before the next level, otherwise the ramp measures a
            // shrinking UTXO pool rather than concurrency.
            let sync_error = mode
                .sync_to_tip(params.scan_timeout())
                .await
                .err()
                .map(|e| e.to_string());
            let utxos_after = mode.unspent_amounts().await?.len() as u64;

            level_notes.push(json!({
                "n_concurrent": level,
                "settle_secs": settled.secs,
                "settled_before_start": settled.ready,
                "spendable_utxos_at_start": settled.spendable_utxos,
                "utxos_before": utxos_before,
                "utxos_after": utxos_after,
                "batch_wall_clock_secs": batch_started.elapsed().as_secs_f64(),
                "construction_wall_clock_secs": construction_wall_clock,
                "max_serialization_gap_secs": gap,
                "max_serialization_gap_source": gap_source,
                "max_construction_gap_secs": construction_gap,
                "max_terminal_state_gap_secs": terminal_gap,
                "success_rate": tally.success_rate(),
                // Failures that are the harness's plumbing rather than the wallet's
                // behaviour. Anything above zero makes this level's numbers suspect.
                "infrastructure_failures": infrastructure_failures,
                "fees": fees,
                "budget_secs": params.s4_budget_secs,
                // True when construction alone used the level's whole budget, so nothing was
                // left to wait for confirmations and the outcomes below are truncated.
                "budget_exhausted": budget_exhausted,
                "tally": tally,
                "errors": errors,
                "sync_error": sync_error,
            }));
        }

        let tally = TxTally::of(&report.transactions);
        report.wall_clock_secs = started.elapsed().as_secs_f64();
        report.tip_height_end = ctx.chain.tip_height().await.unwrap_or(0);
        report.fees_paid = report.total_recorded_fees();
        report.balance_end = mode.balance().await?.total();
        report.utxos_end = mode.unspent_amounts().await?.len() as u64;
        report.successes = tally.confirmed;
        report.failures = tally.unsuccessful();
        report.note("levels", level_notes);
        report.note("construction_is_observable", observes_construction);
        if !observes_construction {
            report.note(
                "construction_note",
                "this mode acknowledges a send before it builds anything, so construction_secs is \
                 the time to accept the work rather than the time to construct a transaction, and \
                 the serialisation gap is read from terminal states instead",
            );
        }
        report.note("tally", tally);
        report.note(
            "infrastructure_failures",
            count_infrastructure_failures(&report.transactions),
        );
        report.note(
            "note",
            "no retries, no backoff and no UTXO pre-partitioning: failures under load are the \
             measurement",
        );
        report.reconcile(report.balance_start.saturating_sub(report.fees_paid));
        Ok(report)
    }
}
