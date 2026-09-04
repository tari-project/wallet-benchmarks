//! Shared scenario machinery: sending with measurement, waiting for confirmations and the
//! small amount of arithmetic the scenarios do on their own results.
//!
//! Nothing in here retries, backs off or throttles. A send that fails is recorded as a failed
//! send, and a transaction that never confirms inside the scenario's budget is recorded as a
//! timeout.

use std::{
    collections::HashMap,
    sync::Arc,
    time::{Duration, Instant},
};

use crate::{
    metrics::{TxMetric, TxOutcome},
    modes::{SendRequest, TxState, TxTracker, WalletMode},
};

/// A broadcast transaction waiting for confirmation, tied back to its [`TxMetric`].
#[derive(Debug, Clone)]
pub struct Pending {
    /// Index into the scenario's transaction list.
    pub index: usize,
    pub tracker: TxTracker,
}

/// Sends one transaction and turns the outcome into a metric.
///
/// `origin` is the batch start, so offsets from different transactions are comparable.
pub async fn send_recorded(
    mode: &Arc<dyn WalletMode>,
    request: SendRequest,
    origin: Instant,
) -> (TxMetric, Option<TxTracker>) {
    let label = request.label.clone();
    let outputs = request.outputs.len() as u32;
    let amount = request.total_amount();
    let submitted_at = origin.elapsed().as_secs_f64();

    match mode.send(request).await {
        Ok(receipt) => {
            let construction_secs = receipt.construction.as_secs_f64();
            let metric = TxMetric {
                label,
                tx_id: Some(receipt.tx_id),
                outputs,
                amount,
                fee: receipt.fee,
                construction_secs,
                broadcast_secs: receipt.broadcast.as_secs_f64(),
                submitted_at_secs: submitted_at,
                construction_done_secs: submitted_at + construction_secs,
                confirmed_at_secs: None,
                outcome: TxOutcome::Broadcast,
                error: None,
            };
            (metric, Some(receipt.tracker))
        },
        Err(e) => {
            let metric = TxMetric {
                label,
                tx_id: None,
                outputs,
                amount,
                fee: 0,
                construction_secs: 0.0,
                broadcast_secs: 0.0,
                submitted_at_secs: submitted_at,
                construction_done_secs: origin.elapsed().as_secs_f64(),
                confirmed_at_secs: None,
                outcome: TxOutcome::Failed,
                error: Some(e.to_string()),
            };
            (metric, None)
        },
    }
}

/// What a wallet has to be able to do before a round of sends can start.
#[derive(Debug, Clone, Copy)]
pub struct SpendRequirement {
    /// How many separate outputs the round needs to spend, one per transaction.
    pub utxos: u64,
    /// How much of the balance the round is about to move.
    pub amount: u64,
}

/// Outcome of waiting for a wallet to make a round's proceeds spendable again.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Settled {
    /// How long the wallet took, from the moment the previous round was confirmed.
    pub secs: f64,
    /// Whether it got there at all.
    pub ready: bool,
    /// Spendable outputs at the end of the wait.
    pub spendable_utxos: u64,
    /// Spendable balance at the end of the wait.
    pub available: u64,
}

/// Waits until the wallet itself says it can spend what the next round needs.
///
/// Confirmation depth is not the same question. An output can be mined, buried `C_min` deep
/// and counted by the wallet, and the wallet will still refuse to spend it until its own
/// bookkeeping has caught up — the console wallet answers "Funds are still pending", the
/// library wallet's input selector answers "Funds are pending. Available: 0". Firing the next
/// round before then measures that lag as a failed send instead of as what it is.
///
/// This is not a retry: no failed send is repeated, and the wait is reported as `settle_secs`
/// so a wallet that takes a long time to settle is measured taking a long time to settle.
pub async fn wait_until_spendable(
    mode: &Arc<dyn WalletMode>,
    requirement: SpendRequirement,
    budget: Duration,
    poll: Duration,
) -> Settled {
    let started = Instant::now();
    let deadline = started + budget;
    let mut spendable_utxos = 0;
    let mut available = 0;

    loop {
        // Drive the wallet's own view forward; a library wallet does not move on its own.
        let _ = mode.refresh().await;
        spendable_utxos = mode
            .spendable_amounts()
            .await
            .map_or(spendable_utxos, |a| a.len() as u64);
        available = mode.balance().await.map_or(available, |b| b.available);

        if spendable_utxos >= requirement.utxos && available >= requirement.amount {
            return Settled {
                secs: started.elapsed().as_secs_f64(),
                ready: true,
                spendable_utxos,
                available,
            };
        }
        if Instant::now() >= deadline {
            return Settled {
                secs: started.elapsed().as_secs_f64(),
                ready: false,
                spendable_utxos,
                available,
            };
        }
        tokio::time::sleep(poll).await;
    }
}

/// Waits for every pending transaction to reach `depth` confirmations, stopping at `budget`.
///
/// Updates the matching metrics in place. Transactions that are still unconfirmed when the
/// budget runs out are marked [`TxOutcome::TimedOut`] — that is a result, not an error.
pub async fn await_confirmations(
    mode: &Arc<dyn WalletMode>,
    pending: &[Pending],
    metrics: &mut [TxMetric],
    depth: u64,
    budget: Duration,
    poll: Duration,
    origin: Instant,
) {
    let mut outstanding: HashMap<usize, TxTracker> =
        pending.iter().map(|p| (p.index, p.tracker)).collect();
    let deadline = Instant::now() + budget;

    while !outstanding.is_empty() {
        // The wallet answers the confirmation question, so its view has to be moving.
        let _ = mode.refresh().await;
        let mut settled = Vec::new();
        for (index, tracker) in &outstanding {
            match mode.tx_state(tracker).await {
                Ok(state) if state.is_confirmed_at(depth) => {
                    if let Some(metric) = metrics.get_mut(*index) {
                        metric.confirmed_at_secs = Some(origin.elapsed().as_secs_f64());
                        metric.outcome = TxOutcome::Confirmed;
                    }
                    settled.push(*index);
                },
                Ok(TxState::Rejected { reason }) => {
                    if let Some(metric) = metrics.get_mut(*index) {
                        metric.outcome = TxOutcome::Failed;
                        metric.error = Some(reason);
                    }
                    settled.push(*index);
                },
                Ok(_) => {},
                Err(e) => {
                    // A query failure is noted but does not settle the transaction; the next
                    // poll tries again.
                    if let Some(metric) = metrics.get_mut(*index) {
                        metric.error = Some(e.to_string());
                    }
                },
            }
        }
        for index in settled {
            outstanding.remove(&index);
        }
        if outstanding.is_empty() {
            break;
        }
        if Instant::now() >= deadline {
            for index in outstanding.keys() {
                if let Some(metric) = metrics.get_mut(*index) {
                    metric.outcome = TxOutcome::TimedOut;
                }
            }
            break;
        }
        tokio::time::sleep(poll).await;
    }
}

/// Largest gap between consecutive construction-complete events.
///
/// A wallet that serialises transaction construction internally shows up here as a gap the
/// size of one construction, no matter how many were fired at once.
pub fn max_serialization_gap(metrics: &[TxMetric]) -> f64 {
    let mut done: Vec<f64> = metrics
        .iter()
        .filter(|m| m.outcome != TxOutcome::Failed)
        .map(|m| m.construction_done_secs)
        .collect();
    done.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    done.windows(2).map(|w| w[1] - w[0]).fold(0.0, f64::max)
}

/// Counts of how the transactions ended.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, serde::Serialize)]
pub struct TxTally {
    pub confirmed: u32,
    pub broadcast: u32,
    pub failed: u32,
    pub timed_out: u32,
}

impl TxTally {
    pub fn of(metrics: &[TxMetric]) -> Self {
        let mut tally = Self::default();
        for metric in metrics {
            match metric.outcome {
                TxOutcome::Confirmed => tally.confirmed += 1,
                TxOutcome::Broadcast => tally.broadcast += 1,
                TxOutcome::Failed => tally.failed += 1,
                TxOutcome::TimedOut => tally.timed_out += 1,
            }
        }
        tally
    }

    /// Everything that did not confirm.
    pub fn unsuccessful(&self) -> u32 {
        self.broadcast + self.failed + self.timed_out
    }

    pub fn success_rate(&self) -> f64 {
        let total = self.confirmed + self.unsuccessful();
        if total == 0 {
            return 0.0;
        }
        f64::from(self.confirmed) / f64::from(total)
    }
}

/// Splits `value` into `parts` equal payment amounts, leaving the remainder (and the fee) to
/// the change output. Returns `None` when the split would produce zero-value outputs.
pub fn split_value(value: u64, parts: u32) -> Option<u64> {
    if parts < 2 {
        return None;
    }
    let each = value / u64::from(parts);
    if each == 0 {
        None
    } else {
        Some(each)
    }
}

/// Reasons the first `n` distinct error strings, for a compact report.
pub fn distinct_errors(metrics: &[TxMetric], n: usize) -> Vec<String> {
    let mut seen: Vec<String> = Vec::new();
    for metric in metrics {
        if let Some(error) = &metric.error {
            if !seen.iter().any(|e| e == error) {
                seen.push(error.clone());
            }
            if seen.len() == n {
                break;
            }
        }
    }
    seen
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicU64, Ordering};

    use anyhow::{anyhow, Result};
    use async_trait::async_trait;

    use super::*;
    use crate::{
        metrics::{count_infrastructure_failures, ScanMetrics},
        modes::{Balance, Birthday, SendReceipt, WalletMode},
        seeds::SeedEntry,
    };

    /// A wallet whose view only moves when it is refreshed, and which will not spend or admit
    /// a confirmation until it has moved far enough. This is the shape of both real wallets:
    /// a transaction is confirmed and money is spendable when the *wallet* says so, and it
    /// only reconsiders when it scans.
    struct SlowWallet {
        refreshes: AtomicU64,
        ready_after: u64,
    }

    impl SlowWallet {
        fn ready_after(ready_after: u64) -> Arc<dyn WalletMode> {
            Arc::new(Self {
                refreshes: AtomicU64::new(0),
                ready_after,
            })
        }

        fn ready(&self) -> bool {
            self.refreshes.load(Ordering::SeqCst) >= self.ready_after
        }
    }

    #[async_trait]
    impl WalletMode for SlowWallet {
        fn name(&self) -> &'static str {
            "slow_wallet"
        }

        async fn version(&self) -> Result<Option<String>> {
            Ok(None)
        }

        async fn reset(&self, _seed: &SeedEntry, _birthday: Birthday) -> Result<()> {
            Ok(())
        }

        async fn shutdown(&self) -> Result<()> {
            Ok(())
        }

        async fn sync_to_tip(&self, _timeout: Duration) -> Result<ScanMetrics> {
            Ok(ScanMetrics::default())
        }

        async fn refresh(&self) -> Result<()> {
            self.refreshes.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }

        async fn scanned_height(&self) -> Result<u64> {
            Ok(100)
        }

        async fn address(&self) -> Result<String> {
            Ok("address".to_string())
        }

        async fn balance(&self) -> Result<Balance> {
            Ok(Balance {
                total: 2_000,
                available: if self.ready() { 2_000 } else { 0 },
                pending_incoming: if self.ready() { 0 } else { 2_000 },
                pending_outgoing: 0,
                timelocked: 0,
            })
        }

        async fn unspent_amounts(&self) -> Result<Vec<u64>> {
            // On chain either way; spendability is the other question.
            Ok(vec![1_000, 1_000])
        }

        async fn spendable_amounts(&self) -> Result<Vec<u64>> {
            Ok(if self.ready() {
                vec![1_000, 1_000]
            } else {
                Vec::new()
            })
        }

        async fn send(&self, _request: SendRequest) -> Result<SendReceipt> {
            Err(anyhow!("the mock wallet does not send"))
        }

        async fn tx_state(&self, _tracker: &TxTracker) -> Result<TxState> {
            Ok(if self.ready() {
                TxState::Mined {
                    height: 97,
                    confirmations: 3,
                    wallet_confirmed: true,
                }
            } else {
                TxState::Mempool
            })
        }
    }

    const FAST_POLL: Duration = Duration::from_millis(1);

    #[tokio::test]
    async fn waiting_drives_the_wallet_until_it_will_spend() {
        let mode = SlowWallet::ready_after(3);
        let settled = wait_until_spendable(
            &mode,
            SpendRequirement {
                utxos: 2,
                amount: 1_500,
            },
            Duration::from_secs(5),
            FAST_POLL,
        )
        .await;
        assert!(settled.ready);
        assert_eq!(settled.spendable_utxos, 2);
        assert_eq!(settled.available, 2_000);
    }

    #[tokio::test]
    async fn a_wallet_that_never_settles_is_reported_not_ready() {
        let mode = SlowWallet::ready_after(u64::MAX);
        let settled = wait_until_spendable(
            &mode,
            SpendRequirement {
                utxos: 1,
                amount: 1,
            },
            Duration::from_millis(20),
            FAST_POLL,
        )
        .await;
        assert!(!settled.ready, "a stall is a result, not a hang");
        assert_eq!(settled.spendable_utxos, 0);
        assert_eq!(settled.available, 0);
    }

    #[tokio::test]
    async fn confirmations_come_from_the_wallet_and_wait_for_it() {
        let mode = SlowWallet::ready_after(3);
        let mut metrics = vec![metric(0.0, TxOutcome::Broadcast)];
        await_confirmations(
            &mode,
            &[Pending {
                index: 0,
                tracker: TxTracker(7),
            }],
            &mut metrics,
            3,
            Duration::from_secs(5),
            FAST_POLL,
            Instant::now(),
        )
        .await;
        assert_eq!(metrics[0].outcome, TxOutcome::Confirmed);
        assert!(metrics[0].confirmed_at_secs.is_some());
    }

    #[tokio::test]
    async fn a_transaction_the_wallet_never_confirms_times_out() {
        let mode = SlowWallet::ready_after(u64::MAX);
        let mut metrics = vec![metric(0.0, TxOutcome::Broadcast)];
        await_confirmations(
            &mode,
            &[Pending {
                index: 0,
                tracker: TxTracker(7),
            }],
            &mut metrics,
            3,
            Duration::from_millis(20),
            FAST_POLL,
            Instant::now(),
        )
        .await;
        assert_eq!(metrics[0].outcome, TxOutcome::TimedOut);
    }

    fn metric(done: f64, outcome: TxOutcome) -> TxMetric {
        TxMetric {
            construction_done_secs: done,
            outcome,
            ..TxMetric::default()
        }
    }

    #[test]
    fn serialization_gap_is_the_largest_step_between_completions() {
        let metrics = vec![
            metric(0.5, TxOutcome::Confirmed),
            metric(4.0, TxOutcome::Confirmed),
            metric(1.0, TxOutcome::Broadcast),
        ];
        assert!((max_serialization_gap(&metrics) - 3.0).abs() < 1e-9);
    }

    #[test]
    fn failed_constructions_do_not_create_phantom_gaps() {
        let metrics = vec![
            metric(0.5, TxOutcome::Confirmed),
            metric(90.0, TxOutcome::Failed),
            metric(1.0, TxOutcome::Confirmed),
        ];
        assert!((max_serialization_gap(&metrics) - 0.5).abs() < 1e-9);
    }

    #[test]
    fn a_single_transaction_has_no_gap() {
        assert_eq!(
            max_serialization_gap(&[metric(1.0, TxOutcome::Confirmed)]),
            0.0
        );
        assert_eq!(max_serialization_gap(&[]), 0.0);
    }

    #[test]
    fn tally_counts_every_outcome() {
        let metrics = vec![
            metric(0.0, TxOutcome::Confirmed),
            metric(0.0, TxOutcome::Confirmed),
            metric(0.0, TxOutcome::Failed),
            metric(0.0, TxOutcome::TimedOut),
        ];
        let tally = TxTally::of(&metrics);
        assert_eq!(tally.confirmed, 2);
        assert_eq!(tally.failed, 1);
        assert_eq!(tally.timed_out, 1);
        assert_eq!(tally.unsuccessful(), 2);
        assert!((tally.success_rate() - 0.5).abs() < 1e-9);
        assert_eq!(TxTally::default().success_rate(), 0.0);
    }

    #[test]
    fn splitting_a_utxo_leaves_the_fee_to_change() {
        assert_eq!(split_value(1_000, 2), Some(500));
        assert_eq!(split_value(1_001, 8), Some(125));
        assert_eq!(split_value(4, 8), None, "a split into dust is refused");
        assert_eq!(split_value(1_000, 1), None);
    }

    #[test]
    fn pool_failures_are_counted_apart_from_wallet_failures() {
        let failed = |error: &str| TxMetric {
            outcome: TxOutcome::Failed,
            error: Some(error.to_string()),
            ..TxMetric::default()
        };
        let metrics = vec![
            failed("timed out waiting for connection"),
            failed("Failed to acquire database connection: timed out waiting for connection"),
            failed("Funds are pending. Available: 0 µT, Pending: 7.999440 T, Required: 200 µT"),
            metric(0.0, TxOutcome::Confirmed),
        ];
        // Two of the three failures were the plumbing; only the third says anything about the
        // wallet under load.
        assert_eq!(count_infrastructure_failures(&metrics), 2);
        assert_eq!(TxTally::of(&metrics).failed, 3);
    }

    #[test]
    fn distinct_errors_are_deduplicated_and_capped() {
        let mut a = metric(0.0, TxOutcome::Failed);
        a.error = Some("locked".to_string());
        let mut b = metric(0.0, TxOutcome::Failed);
        b.error = Some("locked".to_string());
        let mut c = metric(0.0, TxOutcome::Failed);
        c.error = Some("no funds".to_string());
        let errors = distinct_errors(&[a, b, c], 5);
        assert_eq!(errors, vec!["locked".to_string(), "no funds".to_string()]);
    }
}
