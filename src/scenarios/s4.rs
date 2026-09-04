//! S4 — concurrent construction.
//!
//! For each level of the ramp, `N` construct-and-broadcast calls are fired at once and
//! nothing is done to help them succeed: no retries, no backoff, no UTXO pre-partitioning.
//! Locking, selection contention, stalls and rejections are the measurement.

use std::{ops::Range, sync::Arc, time::Instant};

use anyhow::Result;
use async_trait::async_trait;
use serde::Serialize;

use crate::{
    metrics::{count_infrastructure_failures, ScenarioReport, TxMetric, TxOutcome},
    modes::{SendRequest, WalletMode},
    scenarios::{
        support::{
            await_confirmations,
            distinct_errors,
            fee_source,
            fees_of,
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

pub struct ConcurrentConstruction {
    /// The ramp levels as they will be reported.
    ///
    /// Held rather than written straight into the report: a mode that cannot price its own
    /// transaction until it is on chain has no per-level fee to give until enrichment has run.
    levels: Vec<LevelNote>,
    /// Whether the mode this ran against priced its own transactions as it sent them.
    fees_known_at_send: bool,
}

impl Default for ConcurrentConstruction {
    fn default() -> Self {
        Self {
            levels: Vec::new(),
            // Until a run says otherwise: every mode but the payment processor does.
            fees_known_at_send: true,
        }
    }
}

/// One ramp level's line in the `levels` note.
#[derive(Debug, Clone, Serialize)]
struct LevelNote {
    n_concurrent: u32,
    settle_secs: f64,
    settled_before_start: bool,
    spendable_utxos_at_start: u64,
    utxos_before: u64,
    utxos_after: u64,
    batch_wall_clock_secs: f64,
    construction_wall_clock_secs: f64,
    max_serialization_gap_secs: f64,
    max_serialization_gap_source: &'static str,
    max_construction_gap_secs: f64,
    max_terminal_state_gap_secs: f64,
    success_rate: f64,
    /// Failures that are the harness's plumbing rather than the wallet's behaviour. Anything
    /// above zero makes this level's numbers suspect.
    infrastructure_failures: u32,
    fees: u64,
    /// `send` when the wallet priced its transactions as it built them, `enrichment` when the
    /// figure had to be recovered from the chain afterwards, and `enrichment_incomplete` when
    /// that recovery came back short — `fees` is then a floor for the level, not its cost.
    fees_source: &'static str,
    budget_secs: u64,
    /// True when construction alone used the level's whole budget, so nothing was left to wait
    /// for confirmations and the outcomes below are truncated.
    budget_exhausted: bool,
    tally: TxTally,
    errors: Vec<String>,
    sync_error: Option<String>,
    /// Half-open range of this level's transactions in the report.
    tx_index_start: usize,
    tx_index_end: usize,
}

impl LevelNote {
    fn range(&self) -> Range<usize> {
        self.tx_index_start..self.tx_index_end
    }
}

#[async_trait]
impl Scenario for ConcurrentConstruction {
    fn id(&self) -> &'static str {
        "S4"
    }

    fn description(&self) -> &'static str {
        "Fire N concurrent construct-and-broadcast calls per ramp level, with no retries"
    }

    async fn run(
        &mut self,
        mode: &Arc<dyn WalletMode>,
        ctx: &mut RunContext,
    ) -> Result<ScenarioReport> {
        let params = ctx.cfg.params.clone();
        let mut report = ScenarioReport::new(self.id(), mode.name());
        report.tip_height_start = ctx.chain.tip_height().await.unwrap_or(0);

        let started = Instant::now();
        // Whether the gaps between construction-complete events mean anything for this mode.
        let observes_construction = mode.observes_construction();
        self.fees_known_at_send = mode.reports_fee_at_send();
        mode.sync_to_tip(params.scan_timeout()).await?;
        let address = mode.address().await?;
        report.balance_start = mode.balance().await?.total();
        report.utxos_start = mode.unspent_amounts().await?.len() as u64;

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

            let tx_index_end = report.transactions.len();
            let (tally, construction_gap, terminal_gap, fees, errors, infrastructure_failures) = {
                let metrics = &report.transactions[first_index..];
                (
                    TxTally::of(metrics),
                    max_serialization_gap(metrics),
                    max_terminal_state_gap(metrics),
                    fees_of(metrics).total,
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

            self.levels.push(LevelNote {
                n_concurrent: level,
                settle_secs: settled.secs,
                settled_before_start: settled.ready,
                spendable_utxos_at_start: settled.spendable_utxos,
                utxos_before,
                utxos_after,
                batch_wall_clock_secs: batch_started.elapsed().as_secs_f64(),
                construction_wall_clock_secs: construction_wall_clock,
                max_serialization_gap_secs: gap,
                max_serialization_gap_source: gap_source,
                max_construction_gap_secs: construction_gap,
                max_terminal_state_gap_secs: terminal_gap,
                success_rate: tally.success_rate(),
                infrastructure_failures,
                fees,
                fees_source: if self.fees_known_at_send {
                    fee_source::AT_SEND
                } else {
                    fee_source::PENDING
                },
                budget_secs: params.s4_budget_secs,
                budget_exhausted,
                tally,
                errors,
                sync_error,
                tx_index_start: first_index,
                tx_index_end,
            });
        }

        let tally = TxTally::of(&report.transactions);
        report.wall_clock_secs = started.elapsed().as_secs_f64();
        report.tip_height_end = ctx.chain.tip_height().await.unwrap_or(0);
        report.fees_paid = report.total_recorded_fees();
        report.balance_end = mode.balance().await?.total();
        report.utxos_end = mode.unspent_amounts().await?.len() as u64;
        report.successes = tally.confirmed;
        report.failures = tally.unsuccessful();
        report.note("levels", &self.levels);
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

    fn finalise(&mut self, report: &mut ScenarioReport) {
        if self.fees_known_at_send || self.levels.is_empty() {
            return;
        }
        // The per-level cost of the ramp, which the sends themselves could not report: for
        // this mode the fee belongs to a transaction a worker built after the 202 came back.
        let mut incomplete = Vec::new();
        for level in &mut self.levels {
            let fees = fees_of(report.transactions.get(level.range()).unwrap_or_default());
            level.fees = fees.total;
            level.fees_source = if fees.complete {
                fee_source::ENRICHMENT
            } else {
                fee_source::ENRICHMENT_PARTIAL
            };
            if !fees.complete {
                incomplete.push(level.n_concurrent);
            }
        }
        report.note("levels", &self.levels);
        if !incomplete.is_empty() {
            // Expected at the top of the ramp rather than a fault: enrichment prices a
            // transaction through a payref it only gets once the payment confirmed, and S4
            // runs a fixed budget with no retries precisely so that some sends do not. Said
            // out loud because a level's cost is otherwise indistinguishable from a level
            // that happened to be cheap.
            report.note(
                "fees_incomplete",
                serde_json::json!({
                    "levels": incomplete,
                    "reason": "not every transaction at these levels has a fee against it, so \
                               their fees are a floor rather than what the level cost",
                }),
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::modes::PAYMENT_PROCESSOR;

    fn level(tx_index_end: usize) -> LevelNote {
        LevelNote {
            n_concurrent: 8,
            settle_secs: 1.0,
            settled_before_start: true,
            spendable_utxos_at_start: 8,
            utxos_before: 8,
            utxos_after: 8,
            batch_wall_clock_secs: 30.0,
            construction_wall_clock_secs: 2.0,
            max_serialization_gap_secs: 0.5,
            max_serialization_gap_source: "terminal_state",
            max_construction_gap_secs: 0.1,
            max_terminal_state_gap_secs: 0.5,
            success_rate: 1.0,
            infrastructure_failures: 0,
            fees: 0,
            fees_source: fee_source::PENDING,
            budget_secs: 900,
            budget_exhausted: false,
            tally: TxTally {
                confirmed: 2,
                ..TxTally::default()
            },
            errors: Vec::new(),
            sync_error: None,
            tx_index_start: 0,
            tx_index_end,
        }
    }

    fn confirmed(fee: u64) -> TxMetric {
        TxMetric {
            label: "s4-n8-tx0".to_string(),
            fee,
            outcome: TxOutcome::Confirmed,
            ..TxMetric::default()
        }
    }

    #[test]
    fn a_levels_cost_is_filled_in_from_enrichment() {
        // `payment_processor` answers every send with fee 0, so without this the ramp reports
        // that concurrency at every level was free.
        let mut scenario = ConcurrentConstruction {
            levels: vec![level(2)],
            fees_known_at_send: false,
        };
        let mut report = ScenarioReport::new(scenario.id(), PAYMENT_PROCESSOR);
        report.transactions = vec![confirmed(0), confirmed(0)];
        report.note("levels", &scenario.levels);
        assert_eq!(report.notes["levels"][0]["fees"], 0);

        report.transactions[0].fee = 70;
        report.transactions[1].fee = 30;
        scenario.finalise(&mut report);

        assert_eq!(report.notes["levels"][0]["fees"], 100);
        assert_eq!(
            report.notes["levels"][0]["fees_source"],
            fee_source::ENRICHMENT
        );
    }

    #[test]
    fn a_level_enrichment_could_not_fully_price_says_so() {
        // One of the two transactions came back without a fee — an unconfirmed send at the top
        // of the ramp has no payref to price it through. 70 µT is then the least this level
        // cost, and calling it `enrichment` would publish it as the whole bill.
        let mut timed_out = level(2);
        timed_out.tally = TxTally {
            confirmed: 1,
            timed_out: 1,
            ..TxTally::default()
        };
        let mut scenario = ConcurrentConstruction {
            levels: vec![timed_out],
            fees_known_at_send: false,
        };
        let mut report = ScenarioReport::new(scenario.id(), PAYMENT_PROCESSOR);
        report.transactions = vec![confirmed(70), TxMetric {
            label: "s4-n8-tx1".to_string(),
            outcome: TxOutcome::TimedOut,
            ..TxMetric::default()
        }];
        scenario.finalise(&mut report);

        assert_eq!(report.notes["levels"][0]["fees"], 70);
        assert_eq!(
            report.notes["levels"][0]["fees_source"],
            fee_source::ENRICHMENT_PARTIAL
        );
        assert_eq!(report.notes["fees_incomplete"]["levels"][0], 8);
    }

    #[test]
    fn a_send_that_never_reached_the_network_is_not_a_missing_fee() {
        // A failed send paid nothing, so the level is fully priced despite its zero.
        let mut failed_one = level(2);
        failed_one.tally = TxTally {
            confirmed: 1,
            failed: 1,
            ..TxTally::default()
        };
        let mut scenario = ConcurrentConstruction {
            levels: vec![failed_one],
            fees_known_at_send: false,
        };
        let mut report = ScenarioReport::new(scenario.id(), PAYMENT_PROCESSOR);
        report.transactions = vec![confirmed(70), TxMetric {
            label: "s4-n8-tx1".to_string(),
            outcome: TxOutcome::Failed,
            ..TxMetric::default()
        }];
        scenario.finalise(&mut report);

        assert_eq!(report.notes["levels"][0]["fees"], 70);
        assert_eq!(
            report.notes["levels"][0]["fees_source"],
            fee_source::ENRICHMENT
        );
        assert!(!report.notes.contains_key("fees_incomplete"));
    }

    #[test]
    fn a_mode_that_prices_its_sends_keeps_the_figures_it_already_published() {
        let mut scenario = ConcurrentConstruction {
            levels: vec![level(2)],
            fees_known_at_send: true,
        };
        let mut report = ScenarioReport::new(scenario.id(), "new_wallet");
        report.transactions = vec![confirmed(70), confirmed(30)];
        scenario.finalise(&mut report);
        assert!(report.notes.is_empty(), "there is nothing to recompute");
    }
}
