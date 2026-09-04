//! The wallet abstraction the scenarios are written against.
//!
//! A scenario never knows whether it is driving a wallet process over gRPC or a wallet
//! library in this process. Everything mode specific lives behind [`WalletMode`]:
//!
//! * [`old_wallet::OldWallet`] spawns `minotari_console_wallet` and speaks gRPC to it.
//! * [`new_wallet::NewWallet`] uses the `minotari` crate directly and signs offline.

pub mod new_wallet;
pub mod old_wallet;
pub mod payment_processor;

use std::time::Duration;

use anyhow::Result;
use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use crate::{
    metrics::{ScanMetrics, ScenarioReport},
    seeds::SeedEntry,
};

/// Label of the mode backed by `minotari_console_wallet`.
pub const OLD_WALLET: &str = "old_wallet";
/// Label of the mode backed by the `minotari-cli` library.
pub const NEW_WALLET: &str = "new_wallet";
/// Label of the mode backed by the real `minotari_payment_processor` service.
pub const PAYMENT_PROCESSOR: &str = "payment_processor";

/// Where a wallet starts scanning from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Birthday {
    /// Scan the whole chain: the seed is re-encoded with birthday 0.
    Genesis,
    /// Scan from the birthday encoded in the seed.
    FromSeed,
}

/// Balance snapshot, in µT.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Balance {
    /// Every µT the wallet holds, whether or not it can spend it yet.
    ///
    /// This is what the scenarios reconcile against. `available` is not: an output is only
    /// spendable once the wallet has *marked* it confirmed, which lags the block depth by a
    /// scan pass, so a wallet that has just received change can hold its whole balance with
    /// `available == 0`. Value conservation is about what the wallet owns, not about what it
    /// is currently willing to spend.
    pub total: u64,
    /// The portion the wallet will spend right now, at confirmation depth `C_min`.
    pub available: u64,
    pub pending_incoming: u64,
    pub pending_outgoing: u64,
    pub timelocked: u64,
}

impl Balance {
    /// Every µT the wallet holds. See [`Balance::total`].
    pub fn total(&self) -> u64 {
        self.total
    }
}

/// Trims a list of unspent output values to the prefix the wallet can actually pay from.
///
/// A wallet will happily list outputs it is not yet willing to spend, and its available
/// balance is the only number that says how much of that list is real. Taking the largest
/// outputs until their total exceeds `available` gives the set a send can actually draw on;
/// anything past that point is a transaction the wallet would refuse.
pub fn cap_to_available(mut amounts: Vec<u64>, available: u64) -> Vec<u64> {
    amounts.sort_unstable_by(|a, b| b.cmp(a));
    let mut running = 0u64;
    amounts.retain(|value| {
        if running.saturating_add(*value) > available {
            return false;
        }
        running += *value;
        true
    });
    amounts
}

/// One payment output.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OutputSpec {
    pub address: String,
    pub amount: u64,
}

/// A transaction to construct and broadcast. More than one output means a single batch
/// transaction paying several recipients, not several transactions.
#[derive(Debug, Clone)]
pub struct SendRequest {
    pub label: String,
    pub outputs: Vec<OutputSpec>,
    pub fee_per_gram: u64,
}

impl SendRequest {
    pub fn single(label: impl Into<String>, address: &str, amount: u64, fee_per_gram: u64) -> Self {
        Self {
            label: label.into(),
            outputs: vec![OutputSpec {
                address: address.to_string(),
                amount,
            }],
            fee_per_gram,
        }
    }

    pub fn total_amount(&self) -> u64 {
        self.outputs.iter().map(|o| o.amount).sum()
    }
}

/// Handle used to follow a broadcast transaction to confirmation.
///
/// Whether a transaction has reached `C_min` is a question about what the *wallet* believes,
/// not about what the base node knows: a wallet that will not spend against a transaction it
/// has not finished processing has not confirmed it, whatever the chain says. So every mode
/// tracks in its own terms, and the shape of "its own terms" differs:
///
/// * `old_wallet` and `new_wallet` own the transaction and answer for its `TxId`, a `u64`.
/// * `payment_processor` never exposes a transaction id at all. It hands back a batch id and one
///   uuid `payment_id` per requested output, and on-chain identity only becomes reachable (via
///   `payref`) once the payment has confirmed. One harness send is therefore one batch, and its
///   state is the aggregate of the payments in it.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum TxTracker {
    /// The wallet's own transaction id.
    WalletTxId(u64),
    /// One payment processor batch: the id of the batch and the payments it created.
    PaymentBatch {
        batch_id: String,
        payment_ids: Vec<String>,
    },
}

impl TxTracker {
    /// Tracker for a mode that owns the transaction and knows its id.
    pub fn wallet_tx_id(tx_id: u64) -> Self {
        Self::WalletTxId(tx_id)
    }

    /// Tracker for one payment processor batch.
    pub fn payment_batch(batch_id: impl Into<String>, payment_ids: Vec<String>) -> Self {
        Self::PaymentBatch {
            batch_id: batch_id.into(),
            payment_ids,
        }
    }

    /// The wallet transaction id, for a mode that has one.
    ///
    /// Returns an error rather than `None` so a mode handed a tracker it did not issue fails
    /// with a message that says which mode it belongs to instead of silently reporting the
    /// transaction as unknown.
    pub fn expect_wallet_tx_id(&self) -> Result<u64> {
        match self {
            Self::WalletTxId(id) => Ok(*id),
            Self::PaymentBatch { batch_id, .. } => Err(anyhow::anyhow!(
                "tracker for payment processor batch {batch_id} was handed to a mode that tracks \
                 by wallet transaction id"
            )),
        }
    }

    /// The payment ids of a payment processor batch.
    pub fn expect_payment_ids(&self) -> Result<&[String]> {
        match self {
            Self::PaymentBatch { payment_ids, .. } => Ok(payment_ids),
            Self::WalletTxId(id) => Err(anyhow::anyhow!(
                "tracker for wallet transaction {id} was handed to the payment processor mode"
            )),
        }
    }
}

impl std::fmt::Display for TxTracker {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::WalletTxId(id) => write!(f, "tx {id}"),
            Self::PaymentBatch {
                batch_id,
                payment_ids,
            } => write!(f, "batch {batch_id} ({} payments)", payment_ids.len()),
        }
    }
}

/// Result of a successful construct-and-broadcast.
#[derive(Debug, Clone)]
pub struct SendReceipt {
    /// The wallet's transaction id, when the mode has one. `payment_processor` does not: it
    /// exposes no transaction id, only a `payment_id` that reaches the chain via `payref`
    /// after confirmation.
    pub tx_id: Option<u64>,
    pub fee: u64,
    /// Time spent selecting inputs, building and signing.
    pub construction: Duration,
    /// Time from the end of construction to the network accepting the transaction, when the
    /// mode can observe it separately from construction.
    ///
    /// `None` is a disclosure, not a zero: it means the wallet gives the harness no way to
    /// see the broadcast, and [`SendReceipt::broadcast_unavailable`] says why. Reporting zero
    /// would be a claim that the broadcast was instantaneous.
    pub broadcast: Option<Duration>,
    /// Why `broadcast` is absent. Ignored when `broadcast` is `Some`.
    pub broadcast_unavailable: Option<String>,
    pub tracker: TxTracker,
}

/// Where a broadcast transaction currently is.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TxState {
    /// Neither the wallet nor the node has it. Either lost or not propagated yet.
    Unknown,
    /// In a mempool, not mined.
    Mempool,
    Mined {
        height: u64,
        /// Depth, counted from the height the *wallet* has scanned to.
        confirmations: u64,
        /// Whether the wallet itself calls this transaction confirmed — the console wallet's
        /// `MINED_CONFIRMED` status, the library wallet's `confirmed_height` being set.
        ///
        /// Kept separate from `confirmations` because they are different claims: one is the
        /// wallet's verdict under its own rules, the other is arithmetic on the wallet's
        /// heights. A round waits for both.
        wallet_confirmed: bool,
    },
    Rejected {
        reason: String,
    },
}

impl TxState {
    /// Confirmed by the wallet *and* at least `depth` blocks deep.
    ///
    /// Both, deliberately. The wallet's verdict alone would ignore `C_min` whenever the
    /// wallet's own threshold is the lower of the two; the depth alone would call a
    /// transaction settled while the wallet still refuses to spend against it, which is what
    /// made S1 fire rounds into a wallet that answered "Funds are still pending".
    pub fn is_confirmed_at(&self, depth: u64) -> bool {
        matches!(
            self,
            TxState::Mined {
                confirmations,
                wallet_confirmed: true,
                ..
            } if *confirmations >= depth
        )
    }

    /// Nothing more will happen to this transaction: either deep enough, or rejected.
    pub fn is_terminal(&self, depth: u64) -> bool {
        self.is_confirmed_at(depth) || matches!(self, TxState::Rejected { .. })
    }
}

/// A wallet implementation the harness can benchmark.
///
/// Implementations take `&self` throughout so S4 can fire constructions concurrently through
/// an `Arc<dyn WalletMode>`; any locking a wallet needs is the wallet's own, which is
/// precisely what S4 sets out to measure.
#[async_trait]
pub trait WalletMode: Send + Sync {
    /// Stable label used in the result profile.
    fn name(&self) -> &'static str;

    /// Version of the wallet under test, if it can report one.
    async fn version(&self) -> Result<Option<String>>;

    /// Versions that could only be read from a process while it was running.
    ///
    /// Most binaries answer `--version` and are checked before the run starts
    /// ([`crate::binaries`]). The payment processor takes no arguments at all, so its version
    /// is only reachable over HTTP once it is up, and it is collected here instead. Returns
    /// an empty list for a mode with nothing extra to add.
    async fn runtime_versions(&self) -> Vec<crate::binaries::BinaryVersion> {
        Vec::new()
    }

    /// Wipes the wallet data directory and starts the wallet on `seed` from `birthday`.
    ///
    /// This does not wait for the scan to finish; [`WalletMode::sync_to_tip`] does. The old
    /// wallet scans as part of start-up, so splitting the two is what lets both modes report
    /// a comparable `T_scan`.
    async fn reset(&self, seed: &SeedEntry, birthday: Birthday) -> Result<()>;

    /// Stops the wallet and releases its resources.
    async fn shutdown(&self) -> Result<()>;

    /// Waits until the wallet has scanned to the chain tip and reports what that cost.
    async fn sync_to_tip(&self, timeout: Duration) -> Result<ScanMetrics>;

    /// Advances the wallet's own view of the chain by one step, cheaply.
    ///
    /// The scenarios poll a wallet while waiting for it to confirm or settle. A wallet with
    /// its own background services needs nothing here; a library wallet only moves when it is
    /// asked to scan, and polling it without this would wait forever on a view that never
    /// changes.
    async fn refresh(&self) -> Result<()>;

    /// Height the wallet has scanned to.
    async fn scanned_height(&self) -> Result<u64>;

    /// The wallet's own one-sided address, used for self-sends.
    async fn address(&self) -> Result<String>;

    async fn balance(&self) -> Result<Balance>;

    /// Values of the unspent outputs the wallet holds, spendable or not. The length is the
    /// UTXO count the scenarios verify a round against — that is a question about what
    /// landed on chain.
    async fn unspent_amounts(&self) -> Result<Vec<u64>>;

    /// The same list as [`WalletMode::unspent_amounts`], but as the wallet would have
    /// reported it at `height`, or `None` when the wallet cannot answer as of a past height.
    ///
    /// Rescans need this to compare like with like. The set is read once before the wallet is
    /// wiped and again after it has scanned back up, and by then the chain has moved — a scan
    /// takes minutes. An output whose maturity falls between the two heights is absent from
    /// the first reading and present in the second, which looks exactly like a rescan
    /// inventing an output when nothing of the sort happened.
    async fn unspent_amounts_at(&self, _height: u64) -> Result<Option<Vec<u64>>> {
        Ok(None)
    }

    /// Values of the outputs the wallet's own input selection would consider right now.
    ///
    /// Deliberately separate from [`WalletMode::unspent_amounts`]: an output can be mined,
    /// buried and counted, and still be one the wallet refuses to spend. Scenarios that are
    /// about to send use this; scenarios verifying what a round produced use the other.
    async fn spendable_amounts(&self) -> Result<Vec<u64>>;

    /// Constructs, signs and broadcasts one transaction. No retries: a failure here is a
    /// measurement.
    async fn send(&self, request: SendRequest) -> Result<SendReceipt>;

    /// Current state of a previously broadcast transaction.
    async fn tx_state(&self, tracker: &TxTracker) -> Result<TxState>;

    /// Largest number of payment outputs this mode can put in one transaction.
    fn max_outputs_per_tx(&self) -> usize {
        usize::MAX
    }

    /// Whether this mode can see the moment a transaction finished being constructed.
    ///
    /// Both wallet modes can: they build the transaction themselves and the call returns when
    /// it is built. `payment_processor` cannot — its API answers 202 in a few milliseconds and
    /// a background worker builds the transaction later, recording no timestamp for it. S4
    /// uses this to decide whether the gaps between construction-complete events say anything
    /// about internal serialisation, or whether it has to look at terminal states instead.
    fn observes_construction(&self) -> bool {
        true
    }

    /// Whether a completed [`WalletMode::send`] already knows the fee its transaction pays.
    ///
    /// Both wallet modes build and sign inside the call, so the fee is settled by the time it
    /// returns. `payment_processor` builds nothing during a send: it acknowledges the request
    /// and a worker constructs the transaction later, so the fee does not exist yet and only
    /// becomes readable once [`WalletMode::enrich`] has decoded the transaction off the chain.
    ///
    /// Scenarios use this to decide whether a fee-derived figure can be computed while they
    /// run or has to wait. Reporting zero and checking against it would turn "this mode cannot
    /// tell me the fee yet" into "this mode lost money", which is a different claim.
    fn reports_fee_at_send(&self) -> bool {
        true
    }

    /// Results about this mode that belong in the profile whatever a scenario measured.
    ///
    /// These are findings, not defects to fix: things the wallet does not expose, or does by
    /// design, that bound what any measurement of it can mean.
    fn known_findings(&self) -> Vec<&'static str> {
        Vec::new()
    }

    /// Fills in what only a lookup after the scenario can supply.
    ///
    /// Every mode that knows its transaction's fee and id at send time leaves this alone.
    /// `payment_processor` does not: it reports no fee and no transaction id, and the
    /// `payref` that reaches the chain only exists once a payment has confirmed, so the
    /// on-chain half of every transaction's record has to be recovered afterwards.
    async fn enrich(&self, _report: &mut ScenarioReport) -> Result<()> {
        Ok(())
    }

    /// The processes this mode's resource samples are attributed to, headline first.
    ///
    /// Replaces the old `samples_include_harness()` flag, which could say nothing useful
    /// about a mode that spans three processes: `payment_processor` runs a wallet daemon, a
    /// payment processor and a signer subprocess, and "does this number include the harness"
    /// is not the question a reader of that profile has. The headline entry is the one the
    /// scan table's `peak RSS` and `peak CPU` columns describe; the rest are reported
    /// alongside it.
    fn resource_processes(&self) -> Vec<ProcessRole> {
        vec![ProcessRole::external(self.name())]
    }
}

/// One process a mode attributes resource measurements to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProcessRole {
    /// Stable label used in the profile, e.g. `console_wallet`, `harness` or `daemon`.
    pub label: &'static str,
    /// True when the sampled process also runs the harness, so its figures include it.
    pub includes_harness: bool,
}

impl ProcessRole {
    /// A separate process, sampled on its own.
    pub const fn external(label: &'static str) -> Self {
        Self {
            label,
            includes_harness: false,
        }
    }

    /// A process that is also the harness, so its figures include the harness.
    pub const fn with_harness(label: &'static str) -> Self {
        Self {
            label,
            includes_harness: true,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_wallet_holding_unconfirmed_change_still_reports_its_whole_balance() {
        // What a wallet looks like moments after a self-send confirms: the change is on
        // chain and owned, but not yet marked confirmed, so none of it is spendable.
        let balance = Balance {
            total: 3_999_852,
            available: 0,
            pending_incoming: 3_999_852,
            pending_outgoing: 0,
            timelocked: 0,
        };
        assert_eq!(
            balance.total(),
            3_999_852,
            "value conservation is checked against the total, so a lagging `available` is not a \
             missing balance"
        );
    }

    #[test]
    fn confirmation_needs_the_wallets_verdict_as_well_as_the_depth() {
        // Deep enough by the wallet's heights, but the wallet has not called it confirmed:
        // this is the state that had S1 firing the next round into "Funds are still pending".
        let deep_but_unconfirmed = TxState::Mined {
            height: 100,
            confirmations: 10,
            wallet_confirmed: false,
        };
        assert!(!deep_but_unconfirmed.is_confirmed_at(3));
        assert!(!deep_but_unconfirmed.is_terminal(3));

        // Confirmed by the wallet under its own rule, but shallower than the configured
        // C_min: the harness parameter still applies.
        let confirmed_but_shallow = TxState::Mined {
            height: 100,
            confirmations: 2,
            wallet_confirmed: true,
        };
        assert!(!confirmed_but_shallow.is_confirmed_at(3));
    }

    #[test]
    fn confirmation_depth_gates_on_the_configured_minimum() {
        let mined = TxState::Mined {
            height: 100,
            confirmations: 3,
            wallet_confirmed: true,
        };
        assert!(mined.is_confirmed_at(3));
        assert!(!mined.is_confirmed_at(4));
        assert!(mined.is_terminal(3));
        assert!(!mined.is_terminal(4));
        assert!(!TxState::Mempool.is_terminal(3));
        assert!(!TxState::Unknown.is_terminal(1));
        assert!(TxState::Rejected {
            reason: "no".to_string()
        }
        .is_terminal(3));
    }

    #[test]
    fn the_spendable_set_is_capped_by_what_the_wallet_will_pay_out() {
        // A console wallet that lists 64 outputs of 62,500 µT while only 1,123,332 µT is
        // available will pay from 17 of them and reject the rest with "Funds are still
        // pending", which is exactly what it did in an earlier run.
        let listed = vec![62_500; 64];
        let spendable = cap_to_available(listed, 1_123_332);
        assert_eq!(spendable.len(), 17);
        assert_eq!(spendable.iter().sum::<u64>(), 1_062_500);
    }

    #[test]
    fn capping_handles_the_easy_cases() {
        assert_eq!(cap_to_available(vec![10, 20, 30], 60), vec![30, 20, 10]);
        assert!(cap_to_available(vec![10, 20], 0).is_empty());
        assert!(cap_to_available(vec![100], 99).is_empty());
        assert!(cap_to_available(Vec::new(), 500).is_empty());
    }

    #[test]
    fn a_wallet_tracker_answers_for_its_transaction_id_and_nothing_else() {
        let tracker = TxTracker::wallet_tx_id(42);
        assert_eq!(tracker.expect_wallet_tx_id().unwrap(), 42);

        let err = tracker.expect_payment_ids().unwrap_err().to_string();
        assert!(err.contains("42"), "unexpected error: {err}");
        assert!(
            err.contains("payment processor"),
            "the error has to name the mode that was handed the wrong tracker: {err}"
        );
    }

    #[test]
    fn a_payment_batch_tracker_carries_every_payment_in_the_batch() {
        // One harness send is one `POST /v1/payment-batches`, and the processor answers with
        // one payment id per requested output. All of them have to be followed, because the
        // batch is only settled when every payment in it is.
        let tracker = TxTracker::payment_batch("batch-1", vec![
            "11111111-1111-4111-8111-111111111111".to_string(),
            "22222222-2222-4222-8222-222222222222".to_string(),
        ]);
        assert_eq!(tracker.expect_payment_ids().unwrap().len(), 2);
        assert_eq!(
            tracker.expect_payment_ids().unwrap()[0],
            "11111111-1111-4111-8111-111111111111"
        );

        let err = tracker.expect_wallet_tx_id().unwrap_err().to_string();
        assert!(err.contains("batch-1"), "unexpected error: {err}");
        assert_eq!(tracker.to_string(), "batch batch-1 (2 payments)");
        assert_eq!(TxTracker::wallet_tx_id(7).to_string(), "tx 7");
    }

    #[test]
    fn trackers_are_comparable_so_a_scenario_can_key_on_them() {
        let a = TxTracker::payment_batch("b", vec!["p".to_string()]);
        assert_eq!(a.clone(), a);
        assert_ne!(a, TxTracker::payment_batch("b", Vec::new()));
        assert_ne!(TxTracker::wallet_tx_id(1), TxTracker::wallet_tx_id(2));
    }

    #[test]
    fn a_process_role_says_whether_the_harness_is_in_the_numbers() {
        assert!(!ProcessRole::external("daemon").includes_harness);
        assert!(ProcessRole::with_harness("harness").includes_harness);
        assert_eq!(ProcessRole::external("daemon").label, "daemon");
    }

    #[test]
    fn a_send_request_sums_its_outputs() {
        let request = SendRequest {
            label: "fanout".to_string(),
            outputs: vec![
                OutputSpec {
                    address: "a".to_string(),
                    amount: 10,
                },
                OutputSpec {
                    address: "b".to_string(),
                    amount: 32,
                },
            ],
            fee_per_gram: 5,
        };
        assert_eq!(request.total_amount(), 42);
        assert_eq!(SendRequest::single("x", "addr", 7, 5).total_amount(), 7);
    }
}
