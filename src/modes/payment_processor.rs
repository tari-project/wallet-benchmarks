//! Payment processor mode: the real `minotari_payment_processor` service.
//!
//! This mode is three harness-managed processes plus one the harness never invokes:
//!
//! ```text
//! harness
//!   |  spawn + lifecycle                       HTTP
//!   +--> minotari daemon (payment receiver) <-------+
//!   |      holds the wallet, the seed and the scan  |
//!   |      loop; serves lock_funds                  |
//!   |  spawn + lifecycle                            |
//!   +--> minotari_payment_processor  ---------------+
//!   |      5 background workers, SQLite payments.db
//!   |      spawns minotari_offline_signer once per batch
//!   +--> base node (operator supplied, HTTP RPC)
//! ```
//!
//! Four things follow from that shape, and each of them is a measurement rather than a
//! problem to engineer around:
//!
//! * **The wallet is the daemon.** The processor has no scanner, no seed and no balance. The scan
//!   scenarios measure the daemon; the send scenarios measure the processor's pipeline in front of
//!   it.
//! * **The daemon scans continuously while it serves.** Send scenarios therefore run under
//!   concurrent scan load by construction. It cannot be switched off: the processor reads `GET
//!   /accounts/{name}/balance` before every lock and a stale balance fails the lock.
//! * **The pipeline is serial.** One `unsigned_tx_creator`, one signer subprocess at a time. S4
//!   measures that ceiling, and the ceiling is the result.
//! * **The processor exposes no transaction id and no broadcast event.** On-chain identity is
//!   reachable only through `payref`, and only after confirmation, so per-transaction detail is
//!   recovered by [`PaymentProcessor::enrich`] after the scenario rather than at send time.
//!
//! ## Version straddle
//!
//! The processor pins `tari_transaction_components` at `766f80cc` and speaks offline-signing
//! payload format 4.0.0; the daemon and this harness are on 5.6.0-pre.1 and speak 5.0.0. That
//! is survivable because the 4.0.0 payload only ever travels processor -> signer, and the
//! signer is pinned to a matching tari revision (`9f5adb7`). The seam that does cross the
//! generations is `WalletOutput`: the daemon serialises locked UTXOs and the processor
//! deserialises them. At these two pins the two representations were compared field by field
//! and are identical — but that compatibility is incidental, not guaranteed, and must be
//! re-checked whenever either pin moves.

use std::{
    path::{Path, PathBuf},
    process::Stdio,
    sync::Arc,
    time::{Duration, Instant},
};

use anyhow::{anyhow, Context, Result};
use async_trait::async_trait;
use log::{info, warn};
use serde::{Deserialize, Serialize};
use tari_transaction_components::transaction_components::Transaction;
use tari_utilities::ByteArray;
use tokio::{io::AsyncWriteExt, process::Child, sync::Mutex};
use zeroize::Zeroizing;

use crate::{
    binaries::{self, BinaryVersion},
    chain::ChainClient,
    config::{HarnessConfig, PP_MAX_BATCH_SIZE},
    metrics::{ProcessSample, ScanMetrics, ScenarioReport, TxEnrichment, TxOutcome},
    modes::{
        Balance,
        Birthday,
        ProcessRole,
        SendReceipt,
        SendRequest,
        TxState,
        TxTracker,
        WalletMode,
        PAYMENT_PROCESSOR,
    },
    seeds::SeedEntry,
    sysprobe::ResourceSampler,
};

/// Label the daemon is reported under in the profile's resource breakdown. It is the headline
/// process: the daemon is the wallet.
pub const PROCESS_DAEMON: &str = "daemon";
/// Label the payment processor is reported under.
pub const PROCESS_PAYMENT_PROCESSOR: &str = "payment_processor";

/// Why `broadcast` is always `None` for this mode.
///
/// `EventType` declares `TransactionBroadcast`, `TransactionBroadcastFailed` and
/// `TransactionMempoolDetected`, and not one of them is ever emitted: the only six events the
/// processor writes are `PaymentReceived`, `BatchCreated`, `BatchSigned`,
/// `TransactionConfirmed`, `TransactionReorged` and `BatchFailed`. There is no broadcast
/// timestamp to be had.
pub const NO_BROADCAST_REASON: &str = "the payment processor emits no TransactionBroadcast event, \
                                       so broadcast time is not observable for this mode";

/// Results about this mode that belong in the profile, recorded rather than worked around.
pub const KNOWN_FINDINGS: [&str; 6] = [
    "The processor emits no TransactionBroadcast or TransactionMempoolDetected event despite \
     declaring both in EventType, so broadcast-to-mempool time is not observable.",
    "The processor persists no per-state timestamps: payments and batches carry only created_at \
     and updated_at, overwritten on every transition. The events table is the sole transition \
     history.",
    "The processor exposes no transaction id. On-chain identity is reachable only via payref, and \
     only after confirmation, so live polling and on-chain correlation need two different handles.",
    "The processor's pipeline is serial by construction — one unsigned_tx_creator, one signer \
     subprocess at a time — which bounds this mode's throughput independently of how fast the \
     wallet is.",
    "This mode spans two tari generations: the processor speaks offline-signing payload format \
     4.0.0 and the daemon 5.0.0. The payload never crosses the seam, and the WalletOutput \
     representation that does was verified identical at these two pins, but that is incidental \
     and must be re-verified whenever either pin moves.",
    "The payment processor repository carries no LICENSE file and no `license` field in \
     Cargo.toml, which blocks an external contributor from redistributing a patched build.",
];

// --------------------------------------------------------------------------------------
// Wire types
// --------------------------------------------------------------------------------------

/// `GET /health/version` on the processor.
#[derive(Debug, Clone, Deserialize)]
struct ServiceVersion {
    version: String,
}

/// `GET /version` on the daemon.
#[derive(Debug, Clone, Deserialize)]
struct DaemonVersion {
    version: String,
    name: String,
}

/// `GET /accounts/{name}/balance` on the daemon.
#[derive(Debug, Clone, Deserialize)]
struct AccountBalance {
    total: u64,
    available: u64,
    locked: u64,
    unconfirmed: u64,
    immature: u64,
}

/// `GET /accounts/{name}/address` on the daemon.
#[derive(Debug, Clone, Deserialize)]
struct AddressResponse {
    address: String,
}

/// `GET /accounts/{name}/scan_status` on the daemon.
#[derive(Debug, Clone, Default, Deserialize)]
struct ScanStatus {
    last_scanned_height: u64,
}

/// `GET /accounts/{name}/completed_transactions/by_payref/{payref}` on the daemon.
#[derive(Debug, Clone, Deserialize)]
struct CompletedTransactionResponse {
    id: u64,
    kernel_excess_hex: String,
    mined_height: Option<i64>,
    confirmation_height: Option<i64>,
    broadcast_attempts: i32,
}

/// One item of a `POST /v1/payment-batches` request.
#[derive(Debug, Clone, Serialize)]
struct BulkPaymentItem {
    /// Idempotency key. The processor rejects a partial replay of a batch, so this has to be
    /// unique per intended payment and stable across a retry of the same one.
    client_id: String,
    recipient_address: String,
    /// The processor's own column is `i64`; a benchmark amount never approaches the limit,
    /// but sending a `u64` that does not fit would be a silent truncation on its side.
    amount: i64,
    /// A free-form memo, despite the name — *not* the payment's id, which the processor
    /// assigns.
    payment_id: Option<String>,
}

/// `POST /v1/payment-batches` request body.
#[derive(Debug, Clone, Serialize)]
struct BulkPaymentRequest {
    account_name: String,
    items: Vec<BulkPaymentItem>,
}

/// One payment, as the processor reports it.
#[derive(Debug, Clone, Deserialize)]
struct PaymentResponse {
    payment_id: String,
    status: PaymentStatus,
    #[serde(default)]
    payref: Option<String>,
    #[serde(default)]
    failure_reason: Option<String>,
    #[serde(default)]
    mined_height: Option<i64>,
}

/// `POST /v1/payment-batches` response body.
#[derive(Debug, Clone, Deserialize)]
struct BulkPaymentResponse {
    batch_id: String,
    /// The batch's `PaymentBatchStatus`, rendered by its `Display` impl rather than as an
    /// enum, so it arrives as a `SCREAMING_SNAKE_CASE` string.
    status: String,
    payments: Vec<PaymentResponse>,
}

/// The processor's payment state machine.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
enum PaymentStatus {
    Received,
    Batched,
    Confirmed,
    Failed,
    Cancelled,
}

/// One row of `GET /v1/events`.
#[derive(Debug, Clone, Deserialize)]
struct Event {
    /// The `EventType` variant name, verbatim: the column is written from `Display`, which is
    /// `{:?}`, so it is `BatchCreated` rather than `BATCH_CREATED`.
    event_type: String,
    created_at: String,
}

/// `GET /v1/events` response body.
#[derive(Debug, Clone, Deserialize)]
struct EventListResponse {
    events: Vec<Event>,
}

/// Event names the processor actually writes, in pipeline order.
const BATCH_SIGNED: &str = "BatchSigned";

// --------------------------------------------------------------------------------------
// Pure helpers, unit tested without a running daemon
// --------------------------------------------------------------------------------------

/// Maps the processor's payment states onto the harness's transaction states.
///
/// `signed` is whether the batch has emitted `BatchSigned`, which is the closest observable
/// the processor offers to "this is on its way to the network". There is no broadcast event,
/// so a payment that is `BATCHED` and whose batch has been signed is reported as `Mempool`
/// and one whose batch has not is reported as `Unknown` — the honest distinction between
/// "handed to the network" and "still inside the processor".
///
/// `scanned_height` is the daemon's, not the base node's: this mode's confirmation verdict is
/// the wallet's, exactly as it is for the other two.
fn map_payment_state(
    payments: &[PaymentResponse],
    batch_signed: bool,
    scanned_height: u64,
) -> TxState {
    if payments.is_empty() {
        return TxState::Unknown;
    }
    // One rejection sinks the batch: every payment in it shares one transaction, so there is
    // no state in which some of them settle and the rest do not.
    for payment in payments {
        match payment.status {
            PaymentStatus::Failed => {
                return TxState::Rejected {
                    reason: payment
                        .failure_reason
                        .clone()
                        .unwrap_or_else(|| "failed without a reason".to_string()),
                };
            },
            PaymentStatus::Cancelled => {
                return TxState::Rejected {
                    reason: "cancelled".to_string(),
                };
            },
            _ => {},
        }
    }
    if payments
        .iter()
        .all(|p| p.status == PaymentStatus::Confirmed)
    {
        // `mined_height` is only set once the confirmation checker has seen the transaction,
        // and every payment in a batch shares it. A `CONFIRMED` payment with no height is the
        // processor contradicting itself, so it is reported as still in flight rather than
        // as confirmed at height zero.
        let mined = payments
            .iter()
            .filter_map(|p| p.mined_height)
            .filter(|h| *h > 0)
            .max();
        if let Some(mined) = mined {
            #[allow(clippy::cast_sign_loss)]
            let height = mined as u64;
            return TxState::Mined {
                height,
                confirmations: scanned_height.saturating_sub(height) + 1,
                // The processor only moves a payment to `CONFIRMED` once its own
                // `CONFIRMATION_CHECKER_REQUIRED_CONFIRMATIONS` is met, and the harness sets
                // that to `c_min`. So this is the wallet's own verdict at the configured
                // depth, not a restatement of the arithmetic above.
                wallet_confirmed: true,
            };
        }
        return TxState::Mempool;
    }
    if batch_signed {
        TxState::Mempool
    } else {
        TxState::Unknown
    }
}

/// Renders the processor's `.env`.
///
/// Written to disk for inspection and reproducibility *and* set directly on the child
/// process, so a stale `.env` somewhere above the working directory cannot win — `dotenv`
/// searches upwards from the current directory.
fn render_pp_env(cfg: &HarnessConfig, ctx: &PpEnvContext) -> Vec<(String, String)> {
    let pp = &cfg.payment_processor;
    let mut env = vec![
        // `mode=rwc` so sqlx creates the file. Without it the processor refuses to start on a
        // wiped data directory, because `SqliteConnectOptions` defaults to
        // `create_if_missing = false` and the processor's README expects `sqlx database
        // create` to have been run by hand. The harness wipes this database per scan
        // scenario, so a manual step is not available to it.
        (
            "DATABASE_URL".to_string(),
            format!("sqlite://{}?mode=rwc", ctx.payments_db.display()),
        ),
        ("PAYMENT_RECEIVER".to_string(), ctx.daemon_url.clone()),
        ("BASE_NODE".to_string(), cfg.base_node_url.clone()),
        (
            "OFFLINE_SIGNER_PATH".to_string(),
            ctx.offline_signer.display().to_string(),
        ),
        (
            "OFFLINE_SIGNER_PASSPHRASE".to_string(),
            ctx.signer_passphrase.clone(),
        ),
        // Defaults to `MainNet`, which would derive the wrong addresses for every recipient.
        ("TARI_NETWORK".to_string(), tari_network_name(&cfg.network)),
        ("LISTEN_IP".to_string(), "127.0.0.1".to_string()),
        ("LISTEN_PORT".to_string(), pp.pp_port.to_string()),
        // The processor ignores any per-request fee: this is where its fee comes from, which
        // is why `params.fee_rate` has to be one value across all three modes.
        ("FEE_PER_GRAM".to_string(), cfg.params.fee_rate.to_string()),
        (
            "MAX_INPUT_COUNT_PER_TX".to_string(),
            pp.max_input_count_per_tx.to_string(),
        ),
        // The processor's own default is 10. Matching the harness's confirmation depth is
        // what makes this mode's "confirmed" mean the same thing as the other two modes'.
        (
            "CONFIRMATION_CHECKER_REQUIRED_CONFIRMATIONS".to_string(),
            cfg.params.c_min.to_string(),
        ),
        // Without this the processor masks addresses and amounts in its own logs, which makes
        // a failed run unreadable. Nothing secret is logged: these are benchmark wallets.
        ("REVEAL_PII".to_string(), "true".to_string()),
        (
            "BATCH_CREATOR_SLEEP_SECS".to_string(),
            pp.batch_creator_sleep_secs.to_string(),
        ),
        (
            "UNSIGNED_TX_CREATOR_SLEEP_SECS".to_string(),
            pp.unsigned_tx_creator_sleep_secs.to_string(),
        ),
        (
            "TRANSACTION_SIGNER_SLEEP_SECS".to_string(),
            pp.transaction_signer_sleep_secs.to_string(),
        ),
        (
            "BROADCASTER_SLEEP_SECS".to_string(),
            pp.broadcaster_sleep_secs.to_string(),
        ),
        (
            "CONFIRMATION_CHECKER_SLEEP_SECS".to_string(),
            pp.confirmation_checker_sleep_secs.to_string(),
        ),
    ];
    // The `<ID>` in the middle is a grouping key for the `config` crate's nested-map parsing
    // and is discarded; the account is looked up by the lowercased `NAME`.
    env.push(("ACCOUNTS__BENCH__NAME".to_string(), pp.account_name.clone()));
    env.push((
        "ACCOUNTS__BENCH__VIEW_KEY".to_string(),
        ctx.view_key.clone(),
    ));
    env.push((
        "ACCOUNTS__BENCH__PUBLIC_SPEND_KEY".to_string(),
        ctx.public_spend_key.clone(),
    ));
    env
}

/// Everything `render_pp_env` needs that is not in the configuration.
struct PpEnvContext {
    payments_db: PathBuf,
    daemon_url: String,
    offline_signer: PathBuf,
    signer_passphrase: String,
    view_key: String,
    public_spend_key: String,
}

/// Serialises an environment list as a `.env` file.
fn render_env_file(env: &[(String, String)]) -> String {
    let mut out = String::from(
        "# Written by wallet-benchmarks. Every variable here is also set directly on the\n# \
         payment processor's process, so editing this file has no effect on a running\n# harness; \
         it is here so a run can be reproduced by hand.\n",
    );
    for (key, value) in env {
        out.push_str(&format!("{key}=\"{value}\"\n"));
    }
    out
}

/// The network name the processor's `TARI_NETWORK` expects.
///
/// It parses with `tari_common`'s `Network::from_str`, which is case-insensitive, so the
/// harness's lowercase `esmeralda` is accepted as-is. Passed through rather than mapped so an
/// unknown network fails in the processor with a message naming the value, instead of here
/// with a message naming a table this harness invented.
fn tari_network_name(network: &str) -> String {
    network.to_string()
}

/// Renders the daemon's `config.toml`.
///
/// The daemon writes an embedded default at `--config` when the file is missing, and then
/// applies CLI flags on top. The harness writes the file itself for one reason:
/// `confirmation_window` has no `daemon` flag. It is a `[wallet]` key only, so a harness that
/// let the default file be created would silently run the daemon at a confirmation window of
/// 3 whatever `params.c_min` said.
fn render_daemon_config(cfg: &HarnessConfig, db_path: &Path) -> String {
    let pp = &cfg.payment_processor;
    format!(
        "# Written by wallet-benchmarks. The daemon materialises its own default here when the\n# \
         file is missing; it is written explicitly because `confirmation_window` has no\n# \
         command line flag on the `daemon` subcommand and would otherwise stay at 3.\nnetwork = \
         \"{network}\"\n\n[wallet]\nbase_url = \"{base_node}\"\ndatabase_path = \
         \"{db}\"\naccount_name = \"{account}\"\nscan_interval_secs = {scan_interval}\napi_port = \
         {api_port}\napi_bind_address = \"{bind}\"\napi_disable_auth = true\nconfirmation_window \
         = {confirmation_window}\n",
        network = cfg.network,
        base_node = cfg.base_node_url,
        db = db_path.display(),
        account = pp.account_name,
        scan_interval = pp.daemon_scan_interval_secs,
        api_port = pp.daemon_port,
        bind = pp.daemon_bind_address,
        confirmation_window = cfg.params.c_min,
    )
}

/// Pulls the private view key and the public spend key out of `minotari show-keys`.
///
/// The command prints a human banner and has no machine-readable mode, so the two labelled
/// lines are what there is to parse.
fn parse_show_keys(output: &str) -> Result<(String, String)> {
    let field = |label: &str| {
        output.lines().find_map(|line| {
            let line = line.trim();
            let rest = line.strip_prefix(label)?;
            // `Private View Key : 4b51aa` — the separator is padded on both sides.
            let value = rest.trim().strip_prefix(':')?.trim();
            (!value.is_empty()).then(|| value.to_string())
        })
    };
    let view = field("Private View Key").ok_or_else(|| {
        anyhow!("`minotari show-keys` printed no private view key. It said:\n{output}")
    })?;
    let spend = field("Public Spend Key").ok_or_else(|| {
        anyhow!("`minotari show-keys` printed no public spend key. It said:\n{output}")
    })?;
    Ok((view, spend))
}

/// Whether `minotari_offline_signer status` says the keystore holds keys.
///
/// The command exits 0 either way and says so in words, and "not initialized" contains
/// "initialized", so the negative has to be tested first.
fn signer_is_initialised(status_output: &str) -> bool {
    let text = status_output.to_lowercase();
    if text.contains("not initialized") || text.contains("not initialised") {
        return false;
    }
    text.contains("initialized") || text.contains("initialised")
}

/// Splits a transaction's fee across the payments that shared its batch.
///
/// Reporting the whole fee against each payment would multiply one fee by the batch width,
/// which is precisely the comparison S5 exists to make.
fn fee_per_recipient(transaction_fee: u64, recipients: usize) -> Option<f64> {
    if recipients == 0 {
        return None;
    }
    #[allow(clippy::cast_precision_loss)]
    Some(transaction_fee as f64 / recipients as f64)
}

/// What the daemon's stored copy of a transaction says about it.
struct DecodedTransaction {
    /// Fee actually paid, summed over the kernels.
    fee: u64,
    /// Public nonce of the first kernel's excess signature.
    excess_sig_nonce: Vec<u8>,
    /// Signature component of the first kernel's excess signature.
    excess_sig: Vec<u8>,
}

/// Reads a completed transaction back out of the daemon's database.
///
/// The daemon stores the whole signed transaction as JSON in
/// `completed_transactions.serialized_transaction`, and this harness links the same
/// `tari_transaction_components` version the daemon does, so it reads back.
///
/// This is the only place two facts about a mode 3 transaction exist. The fee: the processor
/// never reports one and the daemon's API does not expose the kernel. And the excess
/// *signature*, which is what the base node's transaction query is keyed on — the daemon
/// reports the excess and nothing else, so without decoding this there is no way to ask the
/// chain about the transaction at all.
fn decode_transaction(serialized: &[u8]) -> Result<DecodedTransaction> {
    let transaction: Transaction = serde_json::from_slice(serialized)
        .context("Could not decode the daemon's stored transaction")?;
    let kernels = transaction.body().kernels();
    let first = kernels
        .first()
        .ok_or_else(|| anyhow!("the daemon stored a transaction with no kernels"))?;
    Ok(DecodedTransaction {
        fee: kernels.iter().map(|kernel| kernel.fee.as_u64()).sum(),
        excess_sig_nonce: first.excess_sig.get_compressed_public_nonce().to_vec(),
        excess_sig: first.excess_sig.get_signature().to_vec(),
    })
}

// --------------------------------------------------------------------------------------
// The mode
// --------------------------------------------------------------------------------------

/// A `minotari` daemon and a `minotari_payment_processor`, driven over HTTP.
pub struct PaymentProcessor {
    cfg: Arc<HarnessConfig>,
    chain: ChainClient,
    http: reqwest::Client,
    daemon_url: String,
    pp_url: String,
    /// Password of the daemon's wallet database.
    password: Zeroizing<String>,
    /// Passphrase of the offline signer's keystore.
    signer_passphrase: Zeroizing<String>,
    inner: Mutex<Inner>,
}

#[derive(Default)]
struct Inner {
    daemon: Option<Child>,
    processor: Option<Child>,
    /// Directory holding the daemon database, the processor database, the rendered `.env` and
    /// both logs. Wiped on every reset.
    work_dir: Option<PathBuf>,
    daemon_db: Option<PathBuf>,
    started_at: Option<Instant>,
    scan_start_height: u64,
    tip_at_start: u64,
    daemon_sampler: Option<ResourceSampler>,
    processor_sampler: Option<ResourceSampler>,
    /// Versions read from the running processes, for the profile.
    versions: Vec<BinaryVersion>,
    /// The seed label the signer keystore currently holds keys for. The keystore is in the OS
    /// keyring rather than in the data directory, so it survives a wipe and has to be
    /// re-pointed when the seed changes — B0 runs on a different seed from S0-S7, and a
    /// keystore holding the wrong keys fails every signature.
    signer_seed: Option<String>,
}

impl PaymentProcessor {
    pub fn new(cfg: Arc<HarnessConfig>, chain: ChainClient) -> Result<Self> {
        // The three validations the specification asks for at start-up, done here rather than
        // in `HarnessConfig::validate` because all three are only meaningful when this mode is
        // actually being run.
        if cfg.params.fee_rate == 0 {
            return Err(anyhow!(
                "params.fee_rate is 0, and the payment processor refuses a FEE_PER_GRAM of 0"
            ));
        }
        if cfg.params.s5_k as usize > PP_MAX_BATCH_SIZE {
            return Err(anyhow!(
                "params.s5_k ({}) is above the payment processor's MAX_BATCH_SIZE of \
                 {PP_MAX_BATCH_SIZE}; one POST /v1/payment-batches is one transaction, so S5's \
                 batch arm cannot be run at this width",
                cfg.params.s5_k
            ));
        }
        let password = Zeroizing::new(cfg.wallet_password()?);
        let signer_passphrase = Zeroizing::new(
            std::env::var(&cfg.payment_processor.signer_passphrase_env).map_err(|_| {
                anyhow!(
                    "The offline signer keystore passphrase environment variable `{}` is not set. \
                     The payment processor cannot sign anything without it.",
                    cfg.payment_processor.signer_passphrase_env
                )
            })?,
        );
        let http = reqwest::Client::builder()
            .timeout(Duration::from_secs(120))
            .build()
            .context("Could not build the HTTP client for the payment processor mode")?;
        Ok(Self {
            daemon_url: cfg.payment_processor.daemon_url(),
            pp_url: cfg.payment_processor.pp_url(),
            cfg,
            chain,
            http,
            password,
            signer_passphrase,
            inner: Mutex::new(Inner::default()),
        })
    }

    fn account(&self) -> &str {
        &self.cfg.payment_processor.account_name
    }

    /// URL of a daemon account route, e.g. `.../accounts/bench/balance`.
    fn daemon_account_url(&self, suffix: &str) -> String {
        format!(
            "{}/accounts/{}/{suffix}",
            self.daemon_url,
            urlencode(self.account())
        )
    }

    async fn get_json<T: serde::de::DeserializeOwned>(&self, url: &str) -> Result<T> {
        let response = self
            .http
            .get(url)
            .send()
            .await
            .with_context(|| format!("GET {url} failed"))?;
        let status = response.status();
        let body = response
            .text()
            .await
            .with_context(|| format!("GET {url} returned a body that could not be read"))?;
        if !status.is_success() {
            return Err(anyhow!("GET {url} returned {status}: {body}"));
        }
        serde_json::from_str(&body)
            .with_context(|| format!("GET {url} returned a body this harness cannot parse: {body}"))
    }

    async fn scan_status(&self) -> Result<ScanStatus> {
        self.get_json(&self.daemon_account_url("scan_status")).await
    }

    async fn payment(&self, payment_id: &str) -> Result<PaymentResponse> {
        self.get_json(&format!(
            "{}/v1/payments/{}",
            self.pp_url,
            urlencode(payment_id)
        ))
        .await
    }

    /// Every event the processor recorded for one batch, oldest first.
    async fn batch_events(&self, batch_id: &str) -> Result<Vec<Event>> {
        let url = format!(
            "{}/v1/events?batch_id={}&limit=200",
            self.pp_url,
            urlencode(batch_id)
        );
        Ok(self.get_json::<EventListResponse>(&url).await?.events)
    }

    /// A read-only connection to the daemon's wallet database.
    ///
    /// The daemon has no endpoint that lists unspent outputs — `lock_funds` is the only place
    /// raw UTXOs appear in its API, and calling that to read a balance would reserve funds.
    /// The scenarios need the UTXO count, so the outputs table is read directly. SQLite is in
    /// WAL mode, so this does not contend with the daemon's own writes.
    async fn daemon_db(&self) -> Result<rusqlite::Connection> {
        let path = {
            let inner = self.inner.lock().await;
            inner.daemon_db.clone().ok_or_else(|| {
                anyhow!("The payment processor mode has not been initialised; call reset first")
            })?
        };
        rusqlite::Connection::open_with_flags(
            &path,
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_URI,
        )
        .with_context(|| format!("Could not open the daemon database at {}", path.display()))
    }

    /// The daemon's account row id, for the direct database reads.
    fn account_id(conn: &rusqlite::Connection, name: &str) -> Result<i64> {
        minotari::db::get_account_by_name(conn, name)?
            .map(|account| account.id)
            .ok_or_else(|| anyhow!("Account `{name}` is missing from the daemon database"))
    }

    /// Runs one of the mode's command line tools to completion and returns its stdout.
    async fn run_tool(
        binary: &Path,
        args: &[String],
        env: &[(&str, &str)],
        stdin_text: Option<&str>,
        what: &str,
    ) -> Result<String> {
        let mut command = tokio::process::Command::new(binary);
        command
            .args(args)
            .stdin(if stdin_text.is_some() {
                Stdio::piped()
            } else {
                Stdio::null()
            })
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        for (key, value) in env {
            command.env(key, value);
        }
        let mut child = command
            .spawn()
            .with_context(|| format!("Could not start {} for {what}", binary.display()))?;
        if let Some(text) = stdin_text {
            if let Some(mut stdin) = child.stdin.take() {
                stdin.write_all(text.as_bytes()).await?;
                stdin.shutdown().await?;
            }
        }
        let output = child
            .wait_with_output()
            .await
            .with_context(|| format!("{what} could not be run"))?;
        let stdout = String::from_utf8_lossy(&output.stdout).to_string();
        if !output.status.success() {
            return Err(anyhow!(
                "{what} failed ({}).\nstdout: {stdout}\nstderr: {}",
                output.status,
                String::from_utf8_lossy(&output.stderr)
            ));
        }
        Ok(stdout)
    }

    /// Points the offline signer's keystore at `seed`, if it is not already.
    ///
    /// The keystore lives in the OS keyring, not in a data directory, so it outlives every
    /// wipe. It holds no measurable state and re-initialising it is not part of any
    /// measurement — but it does have to hold the *right* keys, and the harness runs B0 on a
    /// different seed from the rest of the protocol.
    async fn ensure_signer_keystore(&self, seed: &SeedEntry) -> Result<bool> {
        {
            let inner = self.inner.lock().await;
            if inner.signer_seed.as_deref() == Some(seed.label.as_str()) {
                return Ok(false);
            }
        }
        let binary = self.cfg.payment_processor.offline_signer_binary.clone();
        let passphrase = self.signer_passphrase.to_string();
        let env = [("TARI_PASSPHRASE", passphrase.as_str())];

        let status = Self::run_tool(
            &binary,
            &["status".to_string()],
            &env,
            None,
            "minotari_offline_signer status",
        )
        .await?;
        if signer_is_initialised(&status) {
            // Cleared rather than reused: it may hold another wallet's keys, and a signer
            // signing with the wrong spend key fails every batch in the same way a missing
            // keystore does, only less obviously. `clear` asks for confirmation on stdin.
            Self::run_tool(
                &binary,
                &["clear".to_string()],
                &env,
                Some("y\n"),
                "minotari_offline_signer clear",
            )
            .await?;
        }
        Self::run_tool(
            &binary,
            &[
                "init".to_string(),
                "seed-words".to_string(),
                "--seed-words".to_string(),
                seed.seed_words.clone(),
            ],
            &env,
            None,
            "minotari_offline_signer init seed-words",
        )
        .await?;

        let mut inner = self.inner.lock().await;
        inner.signer_seed = Some(seed.label.clone());
        Ok(true)
    }

    /// Global arguments every `minotari` invocation needs.
    fn minotari_globals(&self, config_path: &Path) -> Vec<String> {
        vec![
            "--config".to_string(),
            config_path.display().to_string(),
            "--network".to_string(),
            self.cfg.network.clone(),
        ]
    }

    async fn stop_processes(inner: &mut Inner) {
        // The processor first, then the daemon: the processor calls the daemon on every
        // worker cycle, and killing the daemon underneath it produces a burst of connection
        // failures in the log that look like a wallet fault and are not.
        for sampler in [inner.processor_sampler.take(), inner.daemon_sampler.take()]
            .into_iter()
            .flatten()
        {
            sampler.stop();
        }
        for mut child in [inner.processor.take(), inner.daemon.take()]
            .into_iter()
            .flatten()
        {
            let _ = child.kill().await;
            let _ = child.wait().await;
        }
    }

    /// Waits for a URL to answer with a body this harness can parse.
    ///
    /// Also watches the two child processes. A daemon that could not bind its port, or a
    /// processor whose `.env` it rejected, exits within a second and then never answers —
    /// waiting the whole start-up budget out and reporting a timeout would hide the reason
    /// behind five minutes of silence, so the exit is detected and the process's own last
    /// words are quoted instead.
    async fn wait_for<T: serde::de::DeserializeOwned>(&self, url: &str, what: &str) -> Result<T> {
        let deadline =
            Instant::now() + Duration::from_secs(self.cfg.payment_processor.startup_timeout_secs);
        loop {
            let last_error = match self.get_json::<T>(url).await {
                Ok(value) => return Ok(value),
                Err(e) => format!("{e:#}"),
            };
            if let Some(died) = self.exited_process().await? {
                return Err(anyhow!(
                    "{what} could not be started: {died}\nThe last request said: {last_error}"
                ));
            }
            if Instant::now() >= deadline {
                return Err(anyhow!(
                    "Timed out after {}s waiting for {what} on {url}: {last_error}",
                    self.cfg.payment_processor.startup_timeout_secs
                ));
            }
            tokio::time::sleep(Duration::from_secs(2)).await;
        }
    }

    /// Describes whichever managed process has exited, with the tail of its log.
    async fn exited_process(&self) -> Result<Option<String>> {
        let mut inner = self.inner.lock().await;
        let work_dir = inner.work_dir.clone();
        // Checked one at a time: two `as_mut()` borrows of `inner` cannot be alive together,
        // and the daemon is checked first because the processor cannot outlive it usefully.
        let exited = match inner
            .daemon
            .as_mut()
            .map(Child::try_wait)
            .transpose()?
            .flatten()
        {
            Some(status) => Some((PROCESS_DAEMON, "daemon.log", status)),
            None => inner
                .processor
                .as_mut()
                .map(Child::try_wait)
                .transpose()?
                .flatten()
                .map(|status| (PROCESS_PAYMENT_PROCESSOR, "payment_processor.log", status)),
        };
        Ok(exited.map(|(label, log, status)| {
            let tail = work_dir
                .as_ref()
                .map(|dir| tail_of(&dir.join(log), 15))
                .unwrap_or_else(|| "No log was captured.".to_string());
            format!("the {label} process exited with {status}.\n{tail}")
        }))
    }

    /// Peak usage of both processes since the last time this was called, headline first.
    async fn sample_processes(&self) -> Vec<ProcessSample> {
        let mut inner = self.inner.lock().await;
        let daemon = inner
            .daemon_sampler
            .take()
            .map(ResourceSampler::stop)
            .unwrap_or_default();
        let processor = inner
            .processor_sampler
            .take()
            .map(ResourceSampler::stop)
            .unwrap_or_default();
        inner.daemon_sampler = Some(ResourceSampler::start(
            inner.daemon.as_ref().and_then(Child::id),
        ));
        inner.processor_sampler = Some(ResourceSampler::start(
            inner.processor.as_ref().and_then(Child::id),
        ));
        vec![
            ProcessSample {
                process: PROCESS_DAEMON.to_string(),
                peak_rss_bytes: daemon.peak_rss_bytes,
                peak_cpu_percent: daemon.peak_cpu_percent,
                includes_harness: false,
            },
            ProcessSample {
                process: PROCESS_PAYMENT_PROCESSOR.to_string(),
                peak_rss_bytes: processor.peak_rss_bytes,
                peak_cpu_percent: processor.peak_cpu_percent,
                includes_harness: false,
            },
        ]
    }
}

#[async_trait]
impl WalletMode for PaymentProcessor {
    fn name(&self) -> &'static str {
        PAYMENT_PROCESSOR
    }

    async fn version(&self) -> Result<Option<String>> {
        let daemon: DaemonVersion = self
            .get_json(&format!("{}/version", self.daemon_url))
            .await?;
        let processor: ServiceVersion = self
            .get_json(&format!("{}/health/version", self.pp_url))
            .await?;
        Ok(Some(format!(
            "{} {} + minotari_payment_processor {}",
            daemon.name, daemon.version, processor.version
        )))
    }

    async fn runtime_versions(&self) -> Vec<BinaryVersion> {
        self.inner.lock().await.versions.clone()
    }

    #[allow(clippy::too_many_lines)]
    async fn reset(&self, seed: &SeedEntry, birthday: Birthday) -> Result<()> {
        let words = seed.words_for(birthday)?;
        let tip_at_start = self.chain.tip_height().await.unwrap_or(0);
        // The daemon reads its scan start from the account's birthday column, filled in when
        // the account is created, so a genesis scan is a `create` from a birthday-zero
        // mnemonic and nothing else — the same mechanism both other modes use.
        let scan_start_height = match birthday {
            Birthday::Genesis => 0,
            Birthday::FromSeed => self
                .chain
                .height_at_time(seed.scan_start_epoch_secs(DAEMON_BIRTHDAY_OFFSET_DAYS))
                .await
                .unwrap_or(0),
        };

        {
            let mut inner = self.inner.lock().await;
            Self::stop_processes(&mut inner).await;
        }

        let work_dir = self.cfg.wallet_dir(PAYMENT_PROCESSOR, &seed.label);
        if work_dir.exists() {
            std::fs::remove_dir_all(&work_dir)
                .with_context(|| format!("Could not wipe {}", work_dir.display()))?;
        }
        std::fs::create_dir_all(&work_dir)
            .with_context(|| format!("Could not create {}", work_dir.display()))?;
        // The processor's embedded log4rs default writes a rolling appender to `logs/` under
        // its working directory, and log4rs does not create that directory itself.
        std::fs::create_dir_all(work_dir.join("logs"))?;

        let work_dir = work_dir.canonicalize().unwrap_or_else(|_| work_dir.clone());
        let daemon_db = work_dir.join("wallet.db");
        let payments_db = work_dir.join("payments.db");
        let daemon_config = work_dir.join("minotari-config.toml");
        std::fs::write(&daemon_config, render_daemon_config(&self.cfg, &daemon_db))?;

        let binary = self.cfg.payment_processor.minotari_binary.clone();
        let globals = self.minotari_globals(&daemon_config);

        // Create the account from the (possibly birthday-zeroed) mnemonic.
        let mut create = globals.clone();
        create.extend([
            "create".to_string(),
            "--password".to_string(),
            self.password.to_string(),
            "--database-path".to_string(),
            daemon_db.display().to_string(),
            "--account-name".to_string(),
            self.account().to_string(),
            "--seed-words".to_string(),
            words,
        ]);
        Self::run_tool(&binary, &create, &[], None, "minotari create").await?;

        // The processor watches the account by view key and public spend key; it never sees
        // the seed.
        let mut show_keys = globals.clone();
        show_keys.extend([
            "show-keys".to_string(),
            "--password".to_string(),
            self.password.to_string(),
            "--database-path".to_string(),
            daemon_db.display().to_string(),
            "--account-name".to_string(),
            self.account().to_string(),
        ]);
        let keys = Self::run_tool(&binary, &show_keys, &[], None, "minotari show-keys").await?;
        let (view_key, public_spend_key) = parse_show_keys(&keys)?;

        let reinitialised = self.ensure_signer_keystore(seed).await?;
        if reinitialised {
            info!(
                "[{PAYMENT_PROCESSOR}] signer keystore re-pointed at seed `{}`",
                seed.label
            );
        }

        let env = render_pp_env(&self.cfg, &PpEnvContext {
            payments_db: payments_db.clone(),
            daemon_url: self.daemon_url.clone(),
            offline_signer: self.cfg.payment_processor.offline_signer_binary.clone(),
            signer_passphrase: self.signer_passphrase.to_string(),
            view_key,
            public_spend_key,
        });
        // The rendered fee has to be the one the rest of the protocol is run at: the processor
        // ignores any per-request fee and takes it from here.
        let rendered_fee = env
            .iter()
            .find(|(key, _)| key == "FEE_PER_GRAM")
            .map(|(_, value)| value.clone())
            .unwrap_or_default();
        if rendered_fee != self.cfg.params.fee_rate.to_string() {
            return Err(anyhow!(
                "the payment processor would be run at FEE_PER_GRAM {rendered_fee} while the \
                 protocol is at params.fee_rate {}; the modes would not be comparable",
                self.cfg.params.fee_rate
            ));
        }
        std::fs::write(work_dir.join(".env"), render_env_file(&env))?;

        // --- daemon ---------------------------------------------------------------------
        let daemon_log = std::fs::File::create(work_dir.join("daemon.log"))?;
        let daemon_log_err = daemon_log.try_clone()?;
        let mut daemon_cmd = tokio::process::Command::new(&binary);
        daemon_cmd
            .args(&globals)
            .arg("daemon")
            .arg("--password")
            .arg(self.password.as_str())
            .arg("--base-url")
            .arg(&self.cfg.base_node_url)
            .arg("--database-path")
            .arg(&daemon_db)
            .arg("--api-port")
            .arg(self.cfg.payment_processor.daemon_port.to_string())
            .arg("--api-bind-address")
            .arg(&self.cfg.payment_processor.daemon_bind_address)
            .arg("--scan-interval-secs")
            .arg(self.cfg.payment_processor.daemon_scan_interval_secs.to_string())
            // The processor builds its client with no credential at all
            // (`MinotariConfiguration { base_path, ..default() }`), so every call would be a
            // 401 without this. The API is on loopback and can spend funds; that is the
            // trade this mode makes and the README says so.
            .arg("--api-disable-auth")
            .current_dir(&work_dir)
            .stdin(Stdio::null())
            .stdout(Stdio::from(daemon_log))
            .stderr(Stdio::from(daemon_log_err))
            .kill_on_drop(true);
        let daemon = daemon_cmd
            .spawn()
            .with_context(|| format!("Could not start the daemon ({})", binary.display()))?;
        let daemon_pid = daemon.id();

        // --- payment processor ----------------------------------------------------------
        let pp_binary = self.cfg.payment_processor.pp_binary.clone();
        let pp_log = std::fs::File::create(work_dir.join("payment_processor.log"))?;
        let pp_log_err = pp_log.try_clone()?;
        let mut pp_cmd = tokio::process::Command::new(&pp_binary);
        for (key, value) in &env {
            pp_cmd.env(key, value);
        }
        pp_cmd
            .current_dir(&work_dir)
            .stdin(Stdio::null())
            .stdout(Stdio::from(pp_log))
            .stderr(Stdio::from(pp_log_err))
            .kill_on_drop(true);
        let processor = pp_cmd.spawn().with_context(|| {
            format!(
                "Could not start the payment processor ({})",
                pp_binary.display()
            )
        })?;
        let pp_pid = processor.id();

        {
            let mut inner = self.inner.lock().await;
            inner.started_at = Some(Instant::now());
            inner.scan_start_height = scan_start_height;
            inner.tip_at_start = tip_at_start;
            inner.work_dir = Some(work_dir);
            inner.daemon_db = Some(daemon_db);
            inner.daemon = Some(daemon);
            inner.processor = Some(processor);
            inner.daemon_sampler = Some(ResourceSampler::start(daemon_pid));
            inner.processor_sampler = Some(ResourceSampler::start(pp_pid));
        }

        // Both have to answer before anything is measured. The processor's version is only
        // reachable here: its binary takes no arguments at all, so there is no `--version` to
        // check before it is running.
        let daemon_version: DaemonVersion = self
            .wait_for(
                &format!("{}/version", self.daemon_url),
                "the minotari daemon",
            )
            .await?;
        let pp_version: ServiceVersion = self
            .wait_for(
                &format!("{}/health/version", self.pp_url),
                "the payment processor",
            )
            .await?;

        let observed = vec![
            binaries::check(
                "minotari_daemon",
                &format!("{}/version", self.daemon_url),
                &format!("{} {}", daemon_version.name, daemon_version.version),
                &self.cfg.pins.minotari,
            )?,
            binaries::check(
                "minotari_payment_processor",
                &format!("{}/health/version", self.pp_url),
                &pp_version.version,
                &self.cfg.pins.payment_processor,
            )?,
        ];
        let mut inner = self.inner.lock().await;
        inner.versions = observed;
        Ok(())
    }

    async fn shutdown(&self) -> Result<()> {
        let mut inner = self.inner.lock().await;
        Self::stop_processes(&mut inner).await;
        Ok(())
    }

    async fn sync_to_tip(&self, timeout: Duration) -> Result<ScanMetrics> {
        let (started_at, scan_start_height, tip_at_start) = {
            let mut inner = self.inner.lock().await;
            (
                inner.started_at.take().unwrap_or_else(Instant::now),
                inner.scan_start_height,
                inner.tip_at_start,
            )
        };
        let deadline = Instant::now() + timeout;

        // A daemon that has stopped answering looks exactly like one that is scanning slowly,
        // and a genesis scan's budget is hours. Tolerate a few failures — the API is briefly
        // unavailable while a scan batch commits — then say what actually happened rather
        // than waiting the whole budget out on a dead process.
        let mut consecutive_failures = 0u32;
        let mut last_scanned = 0u64;
        let scanned_height = loop {
            let tip = self.chain.tip_height().await?;
            match self.scan_status().await {
                Ok(status) => {
                    consecutive_failures = 0;
                    last_scanned = status.last_scanned_height;
                    if last_scanned >= tip && last_scanned > 0 {
                        break last_scanned;
                    }
                },
                Err(e) => {
                    consecutive_failures += 1;
                    if consecutive_failures >= DAEMON_UNRESPONSIVE_POLLS {
                        return Err(anyhow!(
                            "The minotari daemon stopped answering: {DAEMON_UNRESPONSIVE_POLLS} \
                             consecutive scan_status calls failed, the last with {e:#}"
                        ));
                    }
                },
            }
            if Instant::now() >= deadline {
                return Err(anyhow!(
                    "Timed out waiting for the minotari daemon to reach the tip (daemon at \
                     {last_scanned}, tip {tip})"
                ));
            }
            tokio::time::sleep(self.cfg.params.poll_interval()).await;
        };

        let duration = started_at.elapsed();
        let tip_end = self.chain.tip_height().await?;
        let outputs_found = self
            .unspent_amounts()
            .await
            .map(|a| a.len() as u64)
            .unwrap_or(0);

        Ok(ScanMetrics {
            duration_secs: duration.as_secs_f64(),
            start_height: scan_start_height,
            end_height: scanned_height,
            tip_height_start: tip_at_start,
            tip_height_end: tip_end,
            blocks_scanned: scanned_height.saturating_sub(scan_start_height),
            blocks_per_sec: 0.0,
            outputs_found,
            // The daemon resolves spends as it scans; there is no separate validation pass to
            // wait for, and no rollback counter on its API.
            validation_secs: 0.0,
            validation_completed: true,
            rollbacks_detected: 0,
            peak_rss_bytes: 0,
            peak_cpu_percent: 0.0,
            headline_process: String::new(),
            processes: Vec::new(),
        }
        .with_processes(self.sample_processes().await)
        .finish())
    }

    async fn refresh(&self) -> Result<()> {
        // The daemon scans on its own loop; there is nothing to drive. This only checks it is
        // still answering, so a dead daemon surfaces as an error rather than as a wallet that
        // never confirms anything.
        self.scan_status().await.map(|_| ())
    }

    async fn scanned_height(&self) -> Result<u64> {
        Ok(self.scan_status().await?.last_scanned_height)
    }

    async fn address(&self) -> Result<String> {
        Ok(self
            .get_json::<AddressResponse>(&self.daemon_account_url("address"))
            .await?
            .address)
    }

    async fn balance(&self) -> Result<Balance> {
        let balance: AccountBalance = self.get_json(&self.daemon_account_url("balance")).await?;
        Ok(Balance {
            total: balance.total,
            available: balance.available,
            pending_incoming: balance.unconfirmed,
            pending_outgoing: balance.locked,
            timelocked: balance.immature,
        })
    }

    async fn unspent_amounts(&self) -> Result<Vec<u64>> {
        let conn = self.daemon_db().await?;
        let account_id = Self::account_id(&conn, self.account())?;
        let height = self.scanned_height().await?;
        crate::modes::new_wallet::held_output_values(&conn, account_id, height)
    }

    async fn unspent_amounts_at(&self, height: u64) -> Result<Option<Vec<u64>>> {
        let conn = self.daemon_db().await?;
        let account_id = Self::account_id(&conn, self.account())?;
        Ok(Some(crate::modes::new_wallet::held_output_values(
            &conn, account_id, height,
        )?))
    }

    async fn spendable_amounts(&self) -> Result<Vec<u64>> {
        // The daemon exposes no unspent-output listing, so this is the held set capped by
        // what the daemon says it will actually pay out — the same cap the other two modes
        // apply, for the same reason: an output can be mined, buried and counted and still be
        // one the wallet refuses to spend.
        let available = self.balance().await?.available;
        Ok(crate::modes::cap_to_available(
            self.unspent_amounts().await?,
            available,
        ))
    }

    async fn send(&self, request: SendRequest) -> Result<SendReceipt> {
        if request.outputs.len() > PP_MAX_BATCH_SIZE {
            return Err(anyhow!(
                "a payment batch may hold at most {PP_MAX_BATCH_SIZE} items ({} requested)",
                request.outputs.len()
            ));
        }
        let items = request
            .outputs
            .iter()
            .enumerate()
            .map(|(i, output)| {
                let amount = i64::try_from(output.amount).map_err(|_| {
                    anyhow!(
                        "amount {} does not fit the processor's i64 column",
                        output.amount
                    )
                })?;
                Ok(BulkPaymentItem {
                    client_id: format!("{}-out{i}", request.label),
                    recipient_address: output.address.clone(),
                    amount,
                    payment_id: None,
                })
            })
            .collect::<Result<Vec<_>>>()?;
        let body = BulkPaymentRequest {
            account_name: self.account().to_string(),
            items,
        };

        // One request is one batch. `POST /v1/payments` would go through `batch_creator`,
        // which coalesces every pending payment for an account into a single batch per cycle
        // and would silently merge transactions the scenario intended to keep apart.
        let url = format!("{}/v1/payment-batches", self.pp_url);
        let started = Instant::now();
        let response = self
            .http
            .post(&url)
            .json(&body)
            .send()
            .await
            .with_context(|| format!("POST {url} failed"))?;
        let status = response.status();
        let text = response.text().await?;
        // Everything up to the 202 and nothing after it. The processor is an asynchronous
        // queue: this is the time to accept the work, not the time to build a transaction,
        // and calling it "construction" would report axum's latency as the wallet's.
        let accepted = started.elapsed();
        if !status.is_success() {
            return Err(anyhow!("POST {url} returned {status}: {text}"));
        }
        let accepted_batch: BulkPaymentResponse =
            serde_json::from_str(&text).with_context(|| {
                format!("POST {url} returned a body this harness cannot parse: {text}")
            })?;

        // 200 rather than 202 means the processor recognised every `client_id` and returned
        // the batch it already had. That is its idempotency working, and it is worth seeing in
        // the log: a scenario that unknowingly replayed a batch would otherwise report the
        // original transaction's timings as a new one's.
        info!(
            "[{PAYMENT_PROCESSOR}] {} -> batch {} ({status}, {} payments, batch status {})",
            request.label,
            accepted_batch.batch_id,
            accepted_batch.payments.len(),
            accepted_batch.status
        );

        Ok(SendReceipt {
            // The processor has no transaction id to give. The batch and payment ids are on
            // the tracker, and the daemon's `TxId` is recovered by `enrich` after the fact.
            tx_id: None,
            // Unknowable here: the fee is fixed by the transaction the processor builds
            // later, and `enrich` reads it back off the chain.
            fee: 0,
            construction: accepted,
            broadcast: None,
            broadcast_unavailable: Some(NO_BROADCAST_REASON.to_string()),
            tracker: TxTracker::payment_batch(
                accepted_batch.batch_id,
                accepted_batch
                    .payments
                    .into_iter()
                    .map(|p| p.payment_id)
                    .collect(),
            ),
        })
    }

    async fn tx_state(&self, tracker: &TxTracker) -> Result<TxState> {
        let payment_ids = tracker.expect_payment_ids()?;
        let mut payments = Vec::with_capacity(payment_ids.len());
        for payment_id in payment_ids {
            payments.push(self.payment(payment_id).await?);
        }
        // Only asked when the payments are not yet terminal, because it is one extra request
        // per poll and it only distinguishes `Unknown` from `Mempool`.
        let batch_signed = if payments
            .iter()
            .all(|p| p.status == PaymentStatus::Confirmed)
        {
            true
        } else {
            let TxTracker::PaymentBatch { batch_id, .. } = tracker else {
                unreachable!("expect_payment_ids already rejected the other variant")
            };
            self.batch_events(batch_id)
                .await
                .map(|events| events.iter().any(|e| e.event_type == BATCH_SIGNED))
                .unwrap_or(false)
        };
        let scanned_height = self.scanned_height().await.unwrap_or(0);
        Ok(map_payment_state(&payments, batch_signed, scanned_height))
    }

    fn max_outputs_per_tx(&self) -> usize {
        PP_MAX_BATCH_SIZE
    }

    fn observes_construction(&self) -> bool {
        // The 202 is an acknowledgement, not a construction. The transaction is built later,
        // by a worker, and the processor records no timestamp for it.
        false
    }

    fn resource_processes(&self) -> Vec<ProcessRole> {
        vec![
            ProcessRole::external(PROCESS_DAEMON),
            ProcessRole::external(PROCESS_PAYMENT_PROCESSOR),
        ]
    }

    fn known_findings(&self) -> Vec<&'static str> {
        KNOWN_FINDINGS.to_vec()
    }

    async fn enrich(&self, report: &mut ScenarioReport) -> Result<()> {
        // What the scenario reconciled against. Every fee found below is one it could not
        // have known, so the expectation has to move with them.
        let fees_before = report.fees_paid;
        let mut enriched = 0u32;
        let mut fees = 0u64;
        for metric in &mut report.transactions {
            let Some(batch_id) = metric.batch_id.clone() else {
                continue;
            };
            let mut enrichment = TxEnrichment::default();

            // 1. The payrefs, which only materialise at confirmation and are the only handle that
            //    reaches the chain.
            for payment_id in &metric.payment_ids {
                match self.payment(payment_id).await {
                    Ok(payment) => {
                        if let Some(payref) = payment.payref {
                            enrichment.payrefs.push(payref);
                        }
                    },
                    Err(e) => warn!("[{PAYMENT_PROCESSOR}] payment {payment_id}: {e:#}"),
                }
            }

            // 2. The daemon's own record of the transaction, by payref.
            if let Some(payref) = enrichment.payrefs.first() {
                let url = self.daemon_account_url(&format!(
                    "completed_transactions/by_payref/{}",
                    urlencode(payref)
                ));
                match self.get_json::<CompletedTransactionResponse>(&url).await {
                    Ok(tx) => {
                        enrichment.wallet_tx_id = Some(tx.id);
                        enrichment.kernel_excess_hex = Some(tx.kernel_excess_hex.clone());
                        #[allow(clippy::cast_sign_loss)]
                        {
                            enrichment.mined_height =
                                tx.mined_height.filter(|h| *h > 0).map(|h| h as u64);
                            enrichment.confirmation_height =
                                tx.confirmation_height.filter(|h| *h > 0).map(|h| h as u64);
                        }
                        enrichment.broadcast_attempts = Some(i64::from(tx.broadcast_attempts));

                        // 3. The authoritative fee, from the kernel of the transaction the daemon
                        //    stored. Neither the processor nor the daemon's API report a fee, so
                        //    this is the only place it exists.
                        if let Ok(Some(serialized)) =
                            self.stored_transaction(&tx.kernel_excess_hex).await
                        {
                            match decode_transaction(&serialized) {
                                Ok(decoded) => {
                                    enrichment.transaction_fee = Some(decoded.fee);
                                    enrichment.fee_per_recipient =
                                        fee_per_recipient(decoded.fee, metric.payment_ids.len());
                                    metric.fee = decoded.fee;
                                    fees += decoded.fee;

                                    // The authoritative confirmation, from the chain rather
                                    // than from either service. Keyed on the excess
                                    // *signature*, which is why the transaction had to be
                                    // decoded: the daemon reports the excess and nothing else.
                                    match self
                                        .chain
                                        .transaction_location(
                                            &decoded.excess_sig_nonce,
                                            &decoded.excess_sig,
                                        )
                                        .await
                                    {
                                        Ok(location) => {
                                            enrichment.base_node_location =
                                                Some(format!("{:?}", location.location));
                                            enrichment.base_node_mined_height =
                                                location.mined_height;
                                        },
                                        Err(e) => warn!(
                                            "[{PAYMENT_PROCESSOR}] base node did not answer for \
                                             kernel {}: {e:#}",
                                            tx.kernel_excess_hex
                                        ),
                                    }
                                },
                                Err(e) => warn!(
                                    "[{PAYMENT_PROCESSOR}] could not decode the stored \
                                     transaction for kernel {}: {e:#}",
                                    tx.kernel_excess_hex
                                ),
                            }
                        }
                    },
                    Err(e) => warn!("[{PAYMENT_PROCESSOR}] payref {payref}: {e:#}"),
                }
            }

            // 4. The transition timeline. The processor overwrites `updated_at` on every transition
            //    and keeps no per-state timestamps, so the events table is the only history there
            //    is.
            match self.batch_events(&batch_id).await {
                Ok(events) => {
                    enrichment.timeline = events
                        .into_iter()
                        .map(|e| (e.event_type, e.created_at))
                        .collect();
                },
                Err(e) => warn!("[{PAYMENT_PROCESSOR}] events for batch {batch_id}: {e:#}"),
            }

            // 5. Broadcast timing stays unavailable, with the reason, whatever else was recovered:
            //    no event carries it.
            metric.broadcast_secs = None;
            metric.broadcast_unavailable = Some(NO_BROADCAST_REASON.to_string());

            metric.enrichment = Some(enrichment);
            enriched += 1;
        }

        if enriched > 0 {
            report.account_for_late_fees(fees_before);
            report.note("enriched_transactions", enriched);
            report.note("enriched_fees", fees);
            report.note(
                "enrichment_note",
                "fees, kernel excesses and transition timelines are recovered after the scenario: \
                 the payment processor reports no fee and no transaction id, and a payref only \
                 exists once a payment has confirmed",
            );
            let confirmed = report
                .transactions
                .iter()
                .filter(|t| t.outcome == TxOutcome::Confirmed)
                .count();
            let with_payref = report
                .transactions
                .iter()
                .filter(|t| t.enrichment.as_ref().is_some_and(|e| !e.payrefs.is_empty()))
                .count();
            if with_payref < confirmed {
                report.note(
                    "enrichment_incomplete",
                    format!(
                        "{with_payref} of {confirmed} confirmed transactions had a payref; the \
                         rest cannot be correlated to the chain"
                    ),
                );
            }
        }
        Ok(())
    }
}

impl PaymentProcessor {
    /// The daemon's stored copy of a transaction, by kernel excess.
    ///
    /// Read straight out of the daemon's database: the API returns the excess as hex but not
    /// the transaction itself, and the base node's `transaction_query` wants the excess
    /// *signature*, which nothing in the daemon's API exposes either.
    async fn stored_transaction(&self, kernel_excess_hex: &str) -> Result<Option<Vec<u8>>> {
        let excess = hex::decode(kernel_excess_hex)
            .with_context(|| format!("`{kernel_excess_hex}` is not hex"))?;
        let conn = self.daemon_db().await?;
        let mut statement = conn.prepare(
            "SELECT serialized_transaction FROM completed_transactions WHERE kernel_excess = ?1",
        )?;
        let row = statement
            .query_row(rusqlite::params![excess], |row| row.get::<_, Vec<u8>>(0))
            .ok();
        Ok(row)
    }
}

/// The last `lines` lines of a log file, for an error message that explains itself.
fn tail_of(path: &Path, lines: usize) -> String {
    match std::fs::read_to_string(path) {
        Ok(text) => {
            let tail: Vec<&str> = text.lines().rev().take(lines).collect();
            format!(
                "Last lines of {}:\n{}",
                path.display(),
                tail.into_iter().rev().collect::<Vec<_>>().join("\n")
            )
        },
        Err(e) => format!("Could not read {}: {e}", path.display()),
    }
}

/// Consecutive failed `scan_status` polls that mean the daemon is gone rather than busy.
const DAEMON_UNRESPONSIVE_POLLS: u32 = 10;

/// Days the daemon's scanner steps back from the account birthday before resolving a start
/// height. Matches `new_wallet`; only used to report where the scan began.
const DAEMON_BIRTHDAY_OFFSET_DAYS: u64 = 2;

/// Percent-encodes the characters that would otherwise change a path segment's meaning.
///
/// Deliberately minimal: the only values interpolated into these URLs are an account name, a
/// uuid and a payref, and reaching for a URL-building dependency to escape three of them would
/// be more code than this.
fn urlencode(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for byte in value.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(byte as char);
            },
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Params;

    fn payment(status: PaymentStatus, mined_height: Option<i64>) -> PaymentResponse {
        PaymentResponse {
            payment_id: "11111111-1111-4111-8111-111111111111".to_string(),
            status,
            payref: None,
            failure_reason: None,
            mined_height,
        }
    }

    #[test]
    fn a_batch_the_processor_has_not_signed_yet_is_not_in_a_mempool() {
        // There is no broadcast event, so "signed" is the closest the processor comes to
        // saying a transaction is on its way. Before that, claiming `Mempool` would be an
        // invention.
        let payments = vec![payment(PaymentStatus::Received, None)];
        assert_eq!(map_payment_state(&payments, false, 100), TxState::Unknown);
        assert_eq!(map_payment_state(&payments, true, 100), TxState::Mempool);
    }

    #[test]
    fn a_confirmed_batch_is_mined_at_the_daemons_own_depth() {
        let payments = vec![
            payment(PaymentStatus::Confirmed, Some(90)),
            payment(PaymentStatus::Confirmed, Some(90)),
        ];
        assert_eq!(map_payment_state(&payments, true, 92), TxState::Mined {
            height: 90,
            confirmations: 3,
            wallet_confirmed: true,
        });
    }

    #[test]
    fn a_confirmed_payment_with_no_height_is_not_confirmed_at_height_zero() {
        // The processor contradicting itself must not become a transaction mined in the
        // genesis block and instantly deep enough for every scenario.
        let payments = vec![payment(PaymentStatus::Confirmed, None)];
        assert_eq!(map_payment_state(&payments, true, 100), TxState::Mempool);
        let payments = vec![payment(PaymentStatus::Confirmed, Some(0))];
        assert_eq!(map_payment_state(&payments, true, 100), TxState::Mempool);
    }

    #[test]
    fn one_failed_payment_sinks_the_whole_batch() {
        // Every payment in a batch shares one transaction, so there is no state in which some
        // settle and the rest do not.
        let mut failed = payment(PaymentStatus::Failed, None);
        failed.failure_reason = Some("insufficient funds".to_string());
        let payments = vec![payment(PaymentStatus::Confirmed, Some(90)), failed];
        assert_eq!(map_payment_state(&payments, true, 100), TxState::Rejected {
            reason: "insufficient funds".to_string()
        });
    }

    #[test]
    fn a_failure_with_no_reason_still_says_it_failed() {
        let payments = vec![payment(PaymentStatus::Failed, None)];
        assert!(matches!(
            map_payment_state(&payments, true, 100),
            TxState::Rejected { .. }
        ));
        let payments = vec![payment(PaymentStatus::Cancelled, None)];
        assert_eq!(map_payment_state(&payments, true, 100), TxState::Rejected {
            reason: "cancelled".to_string()
        });
    }

    #[test]
    fn a_batch_with_no_payments_is_unknown_rather_than_confirmed() {
        // `all()` on an empty slice is true, which would otherwise report an empty batch as
        // confirmed.
        assert_eq!(map_payment_state(&[], true, 100), TxState::Unknown);
    }

    #[test]
    fn a_partly_confirmed_batch_is_still_in_flight() {
        let payments = vec![
            payment(PaymentStatus::Confirmed, Some(90)),
            payment(PaymentStatus::Batched, None),
        ];
        assert_eq!(map_payment_state(&payments, true, 100), TxState::Mempool);
    }

    // --- wire format --------------------------------------------------------------------

    #[test]
    fn a_batch_response_parses() {
        // Shaped exactly as `BulkPaymentResponse`/`PaymentResponse` serialise: the batch
        // status is a Display string, the payment status is SCREAMING_SNAKE_CASE, and the
        // optional fields are omitted rather than null.
        let json = r#"{
            "batch_id": "9c1f7a2e-0e2a-4a1c-9a2f-2f1a3b4c5d6e",
            "account_name": "bench",
            "status": "PENDING_BATCHING",
            "payments": [
                {
                    "payment_id": "11111111-1111-4111-8111-111111111111",
                    "status": "RECEIVED",
                    "client_id": "s5-batch-tx0-out0",
                    "account_name": "bench",
                    "recipient_address": "f2abc",
                    "amount": 100000,
                    "created_at": "2026-09-04T10:00:00Z",
                    "updated_at": "2026-09-04T10:00:00Z"
                }
            ]
        }"#;
        let parsed: BulkPaymentResponse = serde_json::from_str(json).unwrap();
        assert_eq!(parsed.batch_id, "9c1f7a2e-0e2a-4a1c-9a2f-2f1a3b4c5d6e");
        assert_eq!(parsed.status, "PENDING_BATCHING");
        assert_eq!(parsed.payments.len(), 1);
        assert_eq!(parsed.payments[0].status, PaymentStatus::Received);
        assert!(parsed.payments[0].payref.is_none());
    }

    #[test]
    fn a_confirmed_payment_parses_with_its_payref_and_height() {
        let json = r#"{
            "payment_id": "11111111-1111-4111-8111-111111111111",
            "status": "CONFIRMED",
            "client_id": "s1-r1-tx0-out0",
            "account_name": "bench",
            "recipient_address": "f2abc",
            "amount": 100000,
            "payref": "deadbeef",
            "mined_height": 827888,
            "mined_header_hash": "aa",
            "mined_timestamp": 1756900000,
            "created_at": "2026-09-04T10:00:00Z",
            "updated_at": "2026-09-04T10:05:00Z"
        }"#;
        let parsed: PaymentResponse = serde_json::from_str(json).unwrap();
        assert_eq!(parsed.status, PaymentStatus::Confirmed);
        assert_eq!(parsed.payref.as_deref(), Some("deadbeef"));
        assert_eq!(parsed.mined_height, Some(827_888));
    }

    #[test]
    fn every_payment_status_the_processor_can_report_is_understood() {
        for (text, expected) in [
            ("RECEIVED", PaymentStatus::Received),
            ("BATCHED", PaymentStatus::Batched),
            ("CONFIRMED", PaymentStatus::Confirmed),
            ("FAILED", PaymentStatus::Failed),
            ("CANCELLED", PaymentStatus::Cancelled),
        ] {
            let parsed: PaymentStatus = serde_json::from_str(&format!("\"{text}\""))
                .unwrap_or_else(|e| {
                    panic!("`{text}` is a state the processor can report, but: {e}")
                });
            assert_eq!(parsed, expected);
        }
    }

    #[test]
    fn events_arrive_with_pascal_case_type_names() {
        // The column is written from `Display`, which is `{:?}` — not the SCREAMING_SNAKE_CASE
        // the payment and batch statuses use. Matching on the wrong spelling would make every
        // batch look unsigned for ever.
        let json = r#"{
            "events": [
                {
                    "id": 1,
                    "event_type": "BatchCreated",
                    "description": "created",
                    "metadata_json": null,
                    "account_name": "bench",
                    "payment_id": null,
                    "batch_id": "b1",
                    "created_at": "2026-09-04T10:00:00Z"
                },
                {
                    "id": 2,
                    "event_type": "BatchSigned",
                    "description": "signed",
                    "metadata_json": "{}",
                    "account_name": "bench",
                    "payment_id": null,
                    "batch_id": "b1",
                    "created_at": "2026-09-04T10:00:05Z"
                }
            ],
            "total_count": 2
        }"#;
        let parsed: EventListResponse = serde_json::from_str(json).unwrap();
        assert_eq!(parsed.events.len(), 2);
        assert!(parsed.events.iter().any(|e| e.event_type == BATCH_SIGNED));
        assert_eq!(parsed.events[1].created_at, "2026-09-04T10:00:05Z");
    }

    #[test]
    fn the_daemons_balance_maps_onto_the_harnesss_view_of_one() {
        let json = r#"{
            "total": 1000,
            "available": 700,
            "locked": 100,
            "unconfirmed": 150,
            "immature": 50,
            "total_credits": 1000,
            "total_debits": 0,
            "max_height": 827888,
            "max_date": "2026-09-04T10:00:00Z"
        }"#;
        let parsed: AccountBalance = serde_json::from_str(json).unwrap();
        assert_eq!(parsed.total, 1000);
        assert_eq!(parsed.available, 700);
        assert_eq!(parsed.unconfirmed, 150);
        assert_eq!(parsed.immature, 50);
    }

    #[test]
    fn the_daemons_completed_transaction_parses() {
        let json = r#"{
            "id": 12345,
            "pending_tx_id": "p1",
            "account_id": 1,
            "status": "mined_confirmed",
            "last_rejected_reason": null,
            "kernel_excess_hex": "aabb",
            "sent_payref": "deadbeef",
            "sent_output_hash": "ccdd",
            "mined_height": 827888,
            "mined_block_hash_hex": "eeff",
            "confirmation_height": 827891,
            "broadcast_attempts": 2,
            "created_at": "2026-09-04T10:00:00+00:00",
            "updated_at": "2026-09-04T10:05:00+00:00"
        }"#;
        let parsed: CompletedTransactionResponse = serde_json::from_str(json).unwrap();
        assert_eq!(parsed.id, 12_345);
        assert_eq!(parsed.kernel_excess_hex, "aabb");
        assert_eq!(parsed.confirmation_height, Some(827_891));
        assert_eq!(parsed.broadcast_attempts, 2);
    }

    #[test]
    fn the_daemon_and_the_processor_report_versions_in_different_shapes() {
        let daemon: DaemonVersion =
            serde_json::from_str(r#"{"version":"0.2.0","name":"minotari"}"#).unwrap();
        assert_eq!(daemon.name, "minotari");
        let processor: ServiceVersion = serde_json::from_str(r#"{"version":"0.1.0"}"#).unwrap();
        assert_eq!(processor.version, "0.1.0");
    }

    #[test]
    fn a_batch_request_serialises_the_way_the_processor_reads_it() {
        let body = BulkPaymentRequest {
            account_name: "bench".to_string(),
            items: vec![BulkPaymentItem {
                client_id: "s5-batch-tx0-out3".to_string(),
                recipient_address: "f2abc".to_string(),
                amount: 200,
                payment_id: None,
            }],
        };
        let json = serde_json::to_value(&body).unwrap();
        assert_eq!(json["account_name"], "bench");
        assert_eq!(json["items"][0]["client_id"], "s5-batch-tx0-out3");
        assert_eq!(json["items"][0]["amount"], 200);
        assert!(
            json["items"][0]["payment_id"].is_null(),
            "the processor's `payment_id` is an optional memo and has to be present as null"
        );
    }

    // --- configuration rendering --------------------------------------------------------

    fn test_config() -> HarnessConfig {
        HarnessConfig {
            network: "esmeralda".to_string(),
            base_node_url: "https://rpc.esmeralda.tari.com".to_string(),
            params: Params {
                fee_rate: 5,
                c_min: 3,
                ..Params::default()
            },
            ..HarnessConfig::default()
        }
    }

    fn env_of(cfg: &HarnessConfig) -> std::collections::HashMap<String, String> {
        render_pp_env(cfg, &PpEnvContext {
            payments_db: PathBuf::from("/work/payments.db"),
            daemon_url: "http://127.0.0.1:9000".to_string(),
            offline_signer: PathBuf::from("tools/minotari_offline_signer"),
            signer_passphrase: "secret".to_string(),
            view_key: "aa".to_string(),
            public_spend_key: "bb".to_string(),
        })
        .into_iter()
        .collect()
    }

    #[test]
    fn the_processor_is_configured_at_the_protocols_fee_and_confirmation_depth() {
        let env = env_of(&test_config());
        // The processor ignores any per-request fee, so this variable is the only place the
        // mode's fee is set, and it has to be the one the other modes are run at.
        assert_eq!(env["FEE_PER_GRAM"], "5");
        // Its own default is 10, which would make this mode's "confirmed" a different claim
        // from the other two modes'.
        assert_eq!(env["CONFIRMATION_CHECKER_REQUIRED_CONFIRMATIONS"], "3");
    }

    #[test]
    fn the_network_is_set_because_the_processor_defaults_to_mainnet() {
        let env = env_of(&test_config());
        assert_eq!(env["TARI_NETWORK"], "esmeralda");
    }

    #[test]
    fn the_database_url_lets_sqlx_create_the_file() {
        // The processor connects with `create_if_missing` off and its README expects a manual
        // `sqlx database create`. The harness wipes this database per scan scenario, so the
        // URL has to carry the mode.
        let env = env_of(&test_config());
        assert_eq!(env["DATABASE_URL"], "sqlite:///work/payments.db?mode=rwc");
    }

    #[test]
    fn the_account_is_configured_under_the_processors_nested_key_pattern() {
        let env = env_of(&test_config());
        assert_eq!(env["ACCOUNTS__BENCH__NAME"], "bench");
        assert_eq!(env["ACCOUNTS__BENCH__VIEW_KEY"], "aa");
        assert_eq!(env["ACCOUNTS__BENCH__PUBLIC_SPEND_KEY"], "bb");
    }

    #[test]
    fn the_worker_sleeps_are_the_configured_ones_and_not_the_processors_defaults() {
        let env = env_of(&test_config());
        // Left at the processor's own 600s, `batch_creator` alone would put ten minutes into
        // every send. These are disclosed in the profile.
        assert_eq!(env["BATCH_CREATOR_SLEEP_SECS"], "1");
        assert_eq!(env["UNSIGNED_TX_CREATOR_SLEEP_SECS"], "1");
        assert_eq!(env["TRANSACTION_SIGNER_SLEEP_SECS"], "1");
        assert_eq!(env["BROADCASTER_SLEEP_SECS"], "1");
        assert_eq!(env["CONFIRMATION_CHECKER_SLEEP_SECS"], "1");
    }

    #[test]
    fn every_mandatory_processor_variable_is_rendered() {
        let env = env_of(&test_config());
        for key in [
            "DATABASE_URL",
            "PAYMENT_RECEIVER",
            "BASE_NODE",
            "OFFLINE_SIGNER_PATH",
            "OFFLINE_SIGNER_PASSPHRASE",
        ] {
            assert!(
                env.contains_key(key),
                "{key} is mandatory and was not rendered"
            );
        }
        assert_eq!(env["PAYMENT_RECEIVER"], "http://127.0.0.1:9000");
        assert_eq!(env["BASE_NODE"], "https://rpc.esmeralda.tari.com");
    }

    #[test]
    fn the_env_file_is_written_in_a_form_a_human_can_reuse() {
        let text = render_env_file(&[("FEE_PER_GRAM".to_string(), "5".to_string())]);
        assert!(text.contains("FEE_PER_GRAM=\"5\""));
        assert!(text.starts_with('#'), "the file has to explain itself");
    }

    #[test]
    fn the_daemon_config_carries_the_confirmation_window_that_has_no_flag() {
        // `--confirmation-window` exists on other subcommands but not on `daemon`, so a
        // harness relying on the daemon's auto-created default config would silently run at
        // 3 whatever `c_min` said.
        let mut cfg = test_config();
        cfg.params.c_min = 6;
        let text = render_daemon_config(&cfg, Path::new("/work/wallet.db"));
        assert!(text.contains("confirmation_window = 6"), "{text}");
        assert!(text.contains("api_disable_auth = true"), "{text}");
        assert!(text.contains("scan_interval_secs = 5"), "{text}");
        assert!(text.contains("network = \"esmeralda\""), "{text}");
        assert!(
            text.contains("database_path = \"/work/wallet.db\""),
            "{text}"
        );
        assert!(text.contains("account_name = \"bench\""), "{text}");
        // It has to parse as TOML, or the daemon refuses to start.
        let parsed: toml::Value = toml::from_str(&text).unwrap();
        assert_eq!(parsed["wallet"]["api_port"].as_integer(), Some(9_000));
    }

    // --- command output parsing ---------------------------------------------------------

    #[test]
    fn show_keys_output_is_parsed_out_of_its_banner() {
        let output = "---------------------------------------------------------\nWallet           \
                      : bench\nPrivate View_Key : ignored\nPrivate View Key : 4b51aa\nPublic \
                      Spend Key : \
                      504bff\n---------------------------------------------------------\nWARNING: \
                      Keep your private view key safe.\n";
        let (view, spend) = parse_show_keys(output).unwrap();
        assert_eq!(view, "4b51aa");
        assert_eq!(spend, "504bff");
    }

    #[test]
    fn missing_keys_are_reported_with_what_the_command_actually_said() {
        let err = parse_show_keys("error: wrong password\n")
            .unwrap_err()
            .to_string();
        assert!(err.contains("wrong password"), "unexpected error: {err}");
    }

    #[test]
    fn an_uninitialised_signer_is_not_read_as_an_initialised_one() {
        // "not initialized" contains "initialized"; testing the positive first would report
        // every empty keystore as ready and every batch would then fail to sign.
        assert!(!signer_is_initialised(
            "✗ Offline signer is not initialized"
        ));
        assert!(signer_is_initialised("✓ Offline signer is initialized"));
        assert!(!signer_is_initialised(""));
    }

    // --- arithmetic and URLs ------------------------------------------------------------

    #[test]
    fn a_batch_fee_is_split_across_the_recipients_that_shared_it() {
        // One transaction pays ten recipients: charging each of them the whole fee would
        // multiply it by the batch width, which is exactly the comparison S5 makes.
        assert_eq!(fee_per_recipient(1_000, 10), Some(100.0));
        assert_eq!(fee_per_recipient(1_000, 1), Some(1_000.0));
        assert_eq!(fee_per_recipient(1_000, 0), None);
    }

    #[test]
    fn account_names_and_payrefs_are_escaped_into_urls() {
        assert_eq!(urlencode("bench"), "bench");
        assert_eq!(urlencode("a b/c"), "a%20b%2Fc");
        assert_eq!(
            urlencode("11111111-1111-4111-8111-111111111111"),
            "11111111-1111-4111-8111-111111111111"
        );
    }

    #[test]
    fn the_batch_width_is_the_processors_own_limit() {
        assert_eq!(PP_MAX_BATCH_SIZE, 100);
    }

    #[test]
    fn a_garbled_transaction_blob_is_an_error_rather_than_a_zero_fee() {
        // A zero fee would be indistinguishable from a free transaction and would quietly
        // break every balance reconciliation that follows.
        assert!(decode_transaction(b"not json").is_err());
    }

    #[test]
    fn a_log_tail_quotes_the_end_of_the_file_and_survives_a_missing_one() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("daemon.log");
        std::fs::write(&path, "one\ntwo\nthree\nfour\n").unwrap();
        let tail = tail_of(&path, 2);
        assert!(tail.contains("three\nfour"), "{tail}");
        assert!(!tail.contains("one"), "{tail}");

        // A process that died before it opened its log still has to produce a message.
        let missing = tail_of(&dir.path().join("nope.log"), 5);
        assert!(missing.contains("nope.log"), "{missing}");
    }

    #[test]
    fn the_findings_are_the_ones_this_mode_is_meant_to_disclose() {
        assert_eq!(KNOWN_FINDINGS.len(), 6);
        assert!(KNOWN_FINDINGS
            .iter()
            .any(|f| f.contains("TransactionBroadcast")));
        assert!(KNOWN_FINDINGS.iter().any(|f| f.contains("serial")));
        assert!(NO_BROADCAST_REASON.contains("TransactionBroadcast"));
    }
}
