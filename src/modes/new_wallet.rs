//! New wallet mode: the `minotari` crate used as a library.
//!
//! There is no wallet process: the harness owns the wallet database, runs the scanner in
//! process and signs transactions offline with `sign_locked_transaction` before submitting
//! them to the base node over HTTP.
//!
//! Both send paths are the library's own, so what the benchmark measures is the wallet:
//!
//! * single output — [`TransactionSender`], which selects, locks, builds and broadcasts;
//! * several outputs — [`FundLocker::lock`] to reserve the inputs, then
//!   [`OneSidedTransaction::create_unsigned_transaction`] to build. `TransactionSender` takes one
//!   recipient per call, so the S1 fan-out and the S5 batch arm go through the same lock-then-build
//!   pair the CLI's `create-unsigned-transaction` and the REST endpoint use.
//!
//! Signing and broadcasting are identical for both: `sign_locked_transaction`, then the
//! completed transaction is recorded and submitted through [`WalletHttpClient`].

use std::{
    collections::HashMap,
    path::PathBuf,
    sync::Arc,
    time::{Duration, Instant},
};

use anyhow::{anyhow, Context, Result};
use async_trait::async_trait;
use minotari::{
    db::{self, CompletedTransactionStatus, SqlitePool},
    http::WalletHttpClient,
    init_db,
    models::{PendingTransactionStatus, WalletEventType},
    scan::{ScanMode, Scanner},
    transactions::{
        fund_locker::FundLocker,
        manager::{record_first_broadcast, TransactionSender},
        one_sided_transaction::{unsigned_transaction_binding, OneSidedTransaction, Recipient},
    },
    utils::init_wallet::init_with_seed_words,
};
use rusqlite::OptionalExtension;
use tari_common::configuration::Network;
use tari_common_types::{tari_address::TariAddress, transaction::TxId, types::FixedHash};
use tari_transaction_components::{
    consensus::ConsensusConstantsBuilder,
    key_manager::TransactionKeyManagerInterface,
    offline_signing::{models::SignedOneSidedTransactionResult, sign_locked_transaction},
    MicroMinotari,
};
use tari_utilities::ByteArray;
use tokio::sync::Mutex;
use zeroize::Zeroizing;

use crate::{
    chain::ChainClient,
    config::HarnessConfig,
    metrics::{ProcessSample, ScanMetrics},
    modes::{
        Balance,
        Birthday,
        ProcessRole,
        SendReceipt,
        SendRequest,
        TxState,
        TxTracker,
        WalletMode,
        NEW_WALLET,
    },
    seeds::SeedEntry,
    sysprobe::ResourceSampler,
};

/// Account name inside the wallet database. One account per wallet database is enough here.
const ACCOUNT: &str = "benchmark";

/// Label this mode's resource samples are reported under. The wallet runs inside the harness
/// process, so there is no separate process to attribute anything to.
const PROCESS_HARNESS: &str = "harness";

/// Days the scanner steps back from the account birthday before resolving a start height
/// (`DEFAULT_SCANNING_OFFSET_DAYS` in `minotari-cli`, which is private, so it is repeated
/// here). Only used to report where the scan began.
const BIRTHDAY_OFFSET_DAYS: u64 = 2;

/// A `minotari-cli` wallet database driven directly through the library.
pub struct NewWallet {
    cfg: Arc<HarnessConfig>,
    chain: ChainClient,
    password: Zeroizing<String>,
    network: Network,
    inner: Mutex<Inner>,
}

#[derive(Default)]
struct Inner {
    db_path: Option<PathBuf>,
    pool: Option<SqlitePool>,
    scan_start_height: u64,
    tip_at_start: u64,
    started_at: Option<Instant>,
    sampler: Option<ResourceSampler>,
    /// Per transaction, the hashes of the outputs it creates for this wallet. See
    /// [`NewWallet::tx_state`].
    watched_outputs: HashMap<u64, Vec<FixedHash>>,
}

/// What one scan pass did.
struct ScanPass {
    outputs_found: u64,
    rollbacks: u64,
    more_blocks: bool,
}

/// Everything a send needs from the wallet state, cloned out from behind the lock.
#[derive(Clone)]
struct Handles {
    pool: SqlitePool,
    db_path: PathBuf,
}

impl NewWallet {
    pub fn new(cfg: Arc<HarnessConfig>, chain: ChainClient) -> Result<Self> {
        let password = Zeroizing::new(cfg.wallet_password()?);
        let network = cfg.tari_network()?;
        Ok(Self {
            cfg,
            chain,
            password,
            network,
            inner: Mutex::new(Inner::default()),
        })
    }

    async fn handles(&self) -> Result<Handles> {
        let inner = self.inner.lock().await;
        let pool = inner
            .pool
            .clone()
            .ok_or_else(|| anyhow!("The new wallet has not been initialised; call reset first"))?;
        let db_path = inner
            .db_path
            .clone()
            .ok_or_else(|| anyhow!("The new wallet has not been initialised; call reset first"))?;
        Ok(Handles { pool, db_path })
    }

    /// Records the outputs a broadcast transaction will create for this wallet, so
    /// [`WalletMode::tx_state`] can ask the wallet whether it has seen them.
    async fn watch_outputs(&self, tx_id: u64, hashes: Vec<FixedHash>) {
        self.inner
            .lock()
            .await
            .watched_outputs
            .insert(tx_id, hashes);
    }

    fn account_id(pool: &SqlitePool) -> Result<i64> {
        let conn = pool.get()?;
        let account = db::get_account_by_name(&conn, ACCOUNT)?
            .ok_or_else(|| anyhow!("Account `{ACCOUNT}` is missing from the wallet database"))?;
        Ok(account.id)
    }

    /// Tip height as the *wallet* sees it: UTXO maturity and confirmation depth are judged
    /// against what has been scanned, not against what the node knows.
    fn wallet_tip(pool: &SqlitePool, account_id: i64) -> Result<u64> {
        let conn = pool.get()?;
        Ok(
            db::get_latest_scanned_tip_block_by_account(&conn, account_id)?
                .map(|b| b.height)
                .unwrap_or(0),
        )
    }

    /// Runs one full scan pass to the chain tip, returning the outputs it detected, the
    /// rollbacks it went through and whether the tip moved out from under it.
    async fn scan_once(&self, db_path: PathBuf) -> Result<ScanPass> {
        let scanner = Scanner::new(
            &self.password,
            &self.cfg.base_node_url,
            db_path,
            self.cfg.new_wallet.scan_batch_size,
            self.cfg.params.c_min,
        )
        .account(ACCOUNT)
        .mode(ScanMode::Full)
        .processing_threads(self.cfg.new_wallet.scan_threads);

        let (events, more_blocks) = scanner
            .run()
            .await
            .map_err(|e| anyhow!("Scan failed: {e}"))?;
        Ok(ScanPass {
            outputs_found: events
                .iter()
                .filter(|e| matches!(e.event_type, WalletEventType::OutputDetected { .. }))
                .count() as u64,
            rollbacks: events
                .iter()
                .filter(|e| matches!(e.event_type, WalletEventType::BlockRolledBack { .. }))
                .count() as u64,
            more_blocks,
        })
    }

    /// Builds and signs a batch transaction through the library's own send path:
    /// [`FundLocker`] reserves the inputs, [`OneSidedTransaction`] builds the unsigned
    /// transaction and `sign_locked_transaction` signs it. Nothing about selection, locking
    /// or construction is reimplemented here, so what S1's fan-out and S5's batch arm measure
    /// is the wallet's behaviour and not the harness's.
    ///
    /// Returns the signed transaction and the idempotency key its UTXO reservation is filed
    /// under, which is how the broadcast step finds the pending transaction again.
    fn build_batch(
        &self,
        pool: &SqlitePool,
        request: &SendRequest,
    ) -> Result<(SignedOneSidedTransactionResult, String)> {
        // One connection for the whole build: the account lookup and the fund lock share it.
        // `FundLocker::lock` takes the caller's connection rather than a pool of its own, so
        // a batch send holds exactly one.
        let mut conn = pool.get()?;
        let account = db::get_account_by_name(&conn, ACCOUNT)?
            .ok_or_else(|| anyhow!("Account `{ACCOUNT}` is missing from the wallet database"))?;

        let recipients = request
            .outputs
            .iter()
            .map(|output| {
                Ok(Recipient {
                    address: parse_address(&output.address)?,
                    amount: MicroMinotari(output.amount),
                    payment_id: None,
                })
            })
            .collect::<Result<Vec<_>>>()?;
        let amount = MicroMinotari(request.total_amount());
        let fee_per_gram = MicroMinotari(request.fee_per_gram);
        let lock_seconds = self.cfg.new_wallet.lock_seconds;
        let confirmation_window = self.cfg.params.c_min;
        let idempotency_key = format!("{}-{}", request.label, TxId::new_random());

        // The same binding the CLI and the REST endpoint build for this operation, so the
        // locker applies its usual idempotency rules.
        let binding = unsigned_transaction_binding(
            Some(idempotency_key.clone()),
            account.id,
            &recipients,
            fee_per_gram,
            lock_seconds,
            confirmation_window,
        );

        let locked_funds = FundLocker::new()
            .lock(
                &mut conn,
                account.id,
                amount,
                recipients.len(),
                fee_per_gram,
                None,
                binding,
                lock_seconds,
                confirmation_window,
            )
            .map_err(|e| anyhow!("Failed to lock funds: {e}"))?;

        let build = (|| -> Result<SignedOneSidedTransactionResult> {
            let unsigned =
                OneSidedTransaction::new(pool.clone(), self.network, self.password.clone())
                    .create_unsigned_transaction(
                        &account,
                        locked_funds,
                        recipients,
                        fee_per_gram,
                    )?;
            let key_manager = account.get_key_manager(&self.password)?;
            let consensus_constants = ConsensusConstantsBuilder::new(self.network).build();
            let signed =
                sign_locked_transaction(&key_manager, consensus_constants, self.network, unsigned)?;
            Ok(signed)
        })();

        match build {
            Ok(signed) => Ok((signed, idempotency_key)),
            Err(e) => {
                // Nothing was broadcast, so release the reservation the same way the
                // library's single-recipient path does when construction fails.
                if let Ok(conn) = pool.get() {
                    if let Ok(Some(pending)) = db::find_pending_transaction_by_idempotency_key(
                        &conn,
                        &idempotency_key,
                        account.id,
                    ) {
                        let _ = db::expire_and_unlock_pending_transaction(
                            &conn,
                            &pending.id.to_string(),
                        );
                    }
                }
                Err(e)
            },
        }
    }

    /// Broadcasts a batch transaction and records it the way
    /// `TransactionSender::finalize_transaction_and_broadcast` records its own: the reservation
    /// claimed, the completed transaction and its `UNCONFIRMED` change output written in one
    /// write transaction, then submitted through [`WalletHttpClient`] and the node's answer
    /// filed with `record_first_broadcast`.
    ///
    /// The change output has to be written here: the scan recognises the wallet's own sends
    /// by it, so a batch send without it would never be reconciled.
    ///
    /// Any failure before the send is recorded releases the reservation, as the library does;
    /// nothing else in a harness run would. A reservation another request has already claimed
    /// is left alone. Once the send is recorded nothing is an error: a send the node does not
    /// accept stays recorded, with its inputs locked, for the monitor to rebroadcast, and the
    /// node's reason is returned as `Ok(Some(reason))`.
    async fn broadcast_batch(
        &self,
        pool: &SqlitePool,
        signed: SignedOneSidedTransactionResult,
        idempotency_key: &str,
    ) -> Result<Option<String>> {
        let mut conn = pool.get()?;
        let account = db::get_account_by_name(&conn, ACCOUNT)?
            .ok_or_else(|| anyhow!("Account `{ACCOUNT}` is missing from the wallet database"))?;
        let pending_id =
            db::find_pending_transaction_by_idempotency_key(&conn, idempotency_key, account.id)?
                .ok_or_else(|| anyhow!("The UTXO reservation for `{idempotency_key}` is gone"))?
                .id
                .to_string();
        let tx_id = signed.signed_transaction.tx_id;

        // Everything up to the commit. `Ok(Some(status))` is a reservation that is no longer
        // pending: the write transaction is dropped unchanged and the reservation is not ours
        // to release.
        let record_result = (|| -> Result<(WalletHttpClient, Option<PendingTransactionStatus>)> {
            // The node's address is checked before anything is recorded, as the library does.
            let client = WalletHttpClient::new(self.cfg.base_node_url.parse()?)?;
            let kernel_excess = signed
                .signed_transaction
                .transaction
                .body()
                .kernels()
                .first()
                .map(|k| k.excess.as_bytes().to_vec())
                .unwrap_or_default();
            let serialized = serde_json::to_vec(&signed.signed_transaction.transaction)?;
            let sent_output_hash = signed
                .signed_transaction
                .sent_hashes
                .first()
                .map(hex::encode);
            // Decrypting the key manager is slow, so it is done before the write lock is
            // taken, as the library does.
            let view_key = account
                .get_key_manager(&self.password)?
                .get_private_view_key();

            let record =
                conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
            if let Some(status) = db::claim_pending_transaction_for_broadcast(&record, &pending_id)?
            {
                return Ok((client, Some(status)));
            }
            db::create_completed_transaction(
                &record,
                account.id,
                &pending_id,
                &kernel_excess,
                &serialized,
                sent_output_hash,
                tx_id,
            )?;
            if let Some(change_output) = &signed.signed_transaction.change_output {
                db::insert_unconfirmed_change_output(
                    &record,
                    account.id,
                    &view_key,
                    &pending_id,
                    change_output,
                )?;
            }
            record.commit()?;
            Ok((client, None))
        })();

        let client = match record_result {
            Ok((client, None)) => client,
            Ok((_, Some(status))) => {
                return Err(anyhow!(
                    "The UTXO reservation for `{idempotency_key}` is already {status}"
                ));
            },
            Err(e) => {
                // A failed write rolled back, so the reservation is as it was. `Ok(false)`
                // means it is no longer pending and belongs to whoever moved it.
                let _ = db::expire_and_unlock_pending_transaction(&conn, &pending_id);
                return Err(e);
            },
        };

        let response = client
            .submit_transaction(signed.signed_transaction.transaction)
            .await;
        let first_broadcast = record_first_broadcast(Some(&conn), tx_id, response);
        if first_broadcast.accepted_by_node {
            Ok(None)
        } else {
            Ok(Some(first_broadcast.node_reason.unwrap_or_default()))
        }
    }
}

/// The outputs a signed transaction creates that belong to this wallet: its change, plus the
/// payment outputs whenever the wallet is paying itself.
///
/// These are what the scanner will detect once the transaction is mined, and detecting them is
/// how the wallet learns the transaction landed.
fn wallet_owned_hashes(signed: &SignedOneSidedTransactionResult) -> Vec<FixedHash> {
    let mut hashes = Vec::new();
    if let Some(change) = &signed.signed_transaction.change_output {
        hashes.push(change.output_hash());
    }
    hashes.extend(signed.signed_transaction.change_hashes.iter().copied());
    hashes.extend(signed.signed_transaction.sent_hashes.iter().copied());
    hashes
}

/// Opens the wallet database with a pool the benchmark can actually use.
///
/// `init_db` builds its pool with `DB_POOL_SIZE = 5` and r2d2's default 30-second checkout
/// timeout. One send holds up to three connections at once — `start_new_transaction` keeps one
/// for its whole body while `create_or_find_pending_transaction` and
/// `create_pending_transaction` each take another — so two concurrent sends exhaust the pool
/// and the rest fail after 30 seconds with "timed out waiting for connection". In S4 at
/// N = 128 that was 237 of 248 calls, and it measured the pool rather than the wallet.
///
/// This opens the same file, with the same pragmas, with room to queue and a timeout long
/// enough that a contended send waits instead of failing. Waiting is the point: S4 exists to
/// expose serialisation, and a stall that shows up as a delay is a measurement, while one that
/// shows up as an infrastructure error is noise. Both figures are in the recorded config.
fn open_pool(
    db_path: &std::path::Path,
    max_size: u32,
    checkout_timeout: Duration,
) -> Result<SqlitePool> {
    let manager = r2d2_sqlite::SqliteConnectionManager::file(db_path).with_init(|c| {
        // Identical to `connect_and_migrate`: WAL for concurrent readers, and a busy_timeout
        // so contention on the write lock waits rather than erroring.
        c.execute_batch(
            "PRAGMA journal_mode = WAL;
             PRAGMA synchronous = NORMAL;
             PRAGMA foreign_keys = ON;
             PRAGMA busy_timeout = 5000;",
        )
    });
    r2d2::Pool::builder()
        .max_size(max_size.max(1))
        .connection_timeout(checkout_timeout)
        .build(manager)
        .map_err(|e| anyhow!("Could not open {}: {e}", db_path.display()))
}

/// Values of every output the wallet still holds as of `height`, locked or not.
///
/// Deliberately not `db::fetch_unspent_outputs`, which filters `status = 'UNSPENT'` and so
/// drops outputs reserved by a pending transaction. A lock is a reservation, not a disposal:
/// the wallet owns the output either way, and its balance counts it either way. Leaving locked
/// outputs out made an S6 rescan look like it had invented one — 652 against 651, with the
/// balance identical to the µT, because the pre-wipe wallet was holding one output under a
/// lock that nothing had released and the fresh scan had no locks at all.
///
/// (Little releases it in a harness run: reservation expiry is the daemon's `tasks::unlocker`,
/// which only runs inside the daemon, and a recorded send the node will not take is released
/// only once the transaction monitor, which runs after each scan pass, has failed to
/// rebroadcast it `MAX_BROADCAST_ATTEMPTS` (10) times. A send that never confirms therefore
/// usually leaves its input locked for the rest of the run.)
pub(crate) fn held_output_values(
    conn: &rusqlite::Connection,
    account_id: i64,
    height: u64,
) -> Result<Vec<u64>> {
    #[allow(clippy::cast_possible_wrap)]
    let height = height.min(i64::MAX as u64) as i64;
    let mut statement = conn.prepare_cached(
        "SELECT value FROM outputs WHERE account_id = ?1 AND status IN ('UNSPENT', 'LOCKED') AND \
         mined_in_block_height <= ?2 AND maturity <= ?2 AND deleted_at IS NULL AND is_burn = 0",
    )?;
    let values = statement
        .query_map(rusqlite::params![account_id, height], |row| {
            row.get::<_, i64>(0)
        })?
        .collect::<Result<Vec<i64>, _>>()?;
    #[allow(clippy::cast_sign_loss)]
    Ok(values.into_iter().map(|v| v.max(0) as u64).collect())
}

/// Where one of the wallet's outputs sits, by hash: `(mined height, confirmed height)`.
///
/// This asks the outputs table directly rather than through `db::get_output_by_id`, because
/// that helper maps the whole row and the row cannot be mapped: the scanner writes
/// `mined_timestamp` as `2026-08-14 09:12:57+00:00`, which does not parse back into the
/// `NaiveDateTime` the struct declares, and the read fails with "Deserialization failed for
/// column: created_at". Another one for upstream. Only two integers are needed here, and
/// reading just those works.
fn output_depth(
    conn: &rusqlite::Connection,
    account_id: i64,
    hash: &FixedHash,
) -> Result<Option<(u64, Option<u64>)>> {
    let mut statement = conn.prepare_cached(
        "SELECT mined_in_block_height, confirmed_height FROM outputs WHERE account_id = ?1 AND \
         output_hash = ?2 AND deleted_at IS NULL",
    )?;
    let row = statement
        .query_row(rusqlite::params![account_id, hash.as_slice()], |row| {
            Ok((row.get::<_, i64>(0)?, row.get::<_, Option<i64>>(1)?))
        })
        .optional()?;

    #[allow(clippy::cast_sign_loss)]
    Ok(row.map(|(mined, confirmed)| {
        (
            mined.max(0) as u64,
            confirmed.filter(|h| *h >= 0).map(|h| h as u64),
        )
    }))
}

/// Parses a base58 Tari address.
pub fn parse_address(address: &str) -> Result<TariAddress> {
    TariAddress::from_base58(address).map_err(|e| anyhow!("Invalid Tari address `{address}`: {e}"))
}

/// The fee a signed transaction actually pays.
///
/// Deliberately not `signed.request.info.fee`: that is `TransactionBuilder::fee()`, which
/// returns the fee set by `with_fee()` and stays zero when the builder is driven by
/// `with_fee_per_gram()` as it is here. The fee is computed during `build()` and ends up in
/// the kernel, so that is where it is read from.
fn signed_fee(signed: &SignedOneSidedTransactionResult) -> u64 {
    signed
        .signed_transaction
        .transaction
        .body()
        .kernels()
        .iter()
        .map(|kernel| kernel.fee.as_u64())
        .sum()
}

#[async_trait]
impl WalletMode for NewWallet {
    fn name(&self) -> &'static str {
        NEW_WALLET
    }

    async fn version(&self) -> Result<Option<String>> {
        Ok(Some(format!(
            "minotari-cli {}",
            crate::metrics::MINOTARI_CLI_REV
        )))
    }

    async fn reset(&self, seed: &SeedEntry, birthday: Birthday) -> Result<()> {
        // For `Birthday::Genesis` this seed carries birthday 0: `create_account` copies it
        // into the account's `birthday` column, and that column is the only thing the
        // scanner consults for its start height.
        let cipher_seed = seed.cipher_seed_for(birthday)?;
        let expected_birthday = i64::from(cipher_seed.birthday());
        // `create_account` stores the birthday carried by these seed words, and the scanner
        // resolves its start height from that column less its scanning offset. Mirror that
        // calculation so `blocks_scanned` describes the blocks the wallet really walked.
        let scan_start_height = match birthday {
            Birthday::Genesis => 0,
            Birthday::FromSeed => self
                .chain
                .height_at_time(seed.scan_start_epoch_secs(BIRTHDAY_OFFSET_DAYS))
                .await
                .unwrap_or(0),
        };
        let tip_at_start = self.chain.tip_height().await.unwrap_or(0);

        let mut inner = self.inner.lock().await;
        if let Some(sampler) = inner.sampler.take() {
            sampler.stop();
        }
        inner.pool = None;

        let dir = self.cfg.wallet_dir(NEW_WALLET, &seed.label);
        if dir.exists() {
            std::fs::remove_dir_all(&dir)
                .with_context(|| format!("Could not wipe {}", dir.display()))?;
        }
        std::fs::create_dir_all(&dir)
            .with_context(|| format!("Could not create {}", dir.display()))?;
        let db_path = dir.join("wallet.db");

        // `init_db` creates the file and runs the migrations; the pool it returns is then
        // replaced by one sized for the benchmark. See `open_pool`.
        init_with_seed_words(cipher_seed, &self.password, &db_path, Some(ACCOUNT))?;
        drop(init_db(db_path.clone())?);
        let pool = open_pool(
            &db_path,
            self.cfg.new_wallet.db_pool_size,
            Duration::from_secs(self.cfg.new_wallet.db_checkout_timeout_secs),
        )?;

        // Read the birthday back out of the wallet rather than trusting that it took: this
        // column is what the scan starts from, and a genesis scan that silently began at the
        // seed birthday would look like a fast scan instead of a broken one.
        let stored_birthday = {
            let conn = pool.get()?;
            db::get_account_by_name(&conn, ACCOUNT)?
                .ok_or_else(|| anyhow!("Account `{ACCOUNT}` was not created"))?
                .birthday
        };
        if stored_birthday != expected_birthday {
            return Err(anyhow!(
                "The wallet stored birthday {stored_birthday} for a seed with birthday \
                 {expected_birthday}; a {birthday:?} scan would not start where it should"
            ));
        }

        inner.db_path = Some(db_path);
        inner.pool = Some(pool);
        inner.scan_start_height = scan_start_height;
        inner.tip_at_start = tip_at_start;
        inner.started_at = Some(Instant::now());
        inner.sampler = Some(ResourceSampler::start(None));
        Ok(())
    }

    async fn shutdown(&self) -> Result<()> {
        let mut inner = self.inner.lock().await;
        if let Some(sampler) = inner.sampler.take() {
            sampler.stop();
        }
        inner.pool = None;
        Ok(())
    }

    async fn sync_to_tip(&self, timeout: Duration) -> Result<ScanMetrics> {
        let handles = self.handles().await?;
        let (started_at, scan_start_height, tip_at_start) = {
            let mut inner = self.inner.lock().await;
            (
                inner.started_at.take().unwrap_or_else(Instant::now),
                inner.scan_start_height,
                inner.tip_at_start,
            )
        };
        let deadline = Instant::now() + timeout;

        let mut outputs_found = 0;
        let mut rollbacks = 0;
        loop {
            let pass = self.scan_once(handles.db_path.clone()).await?;
            outputs_found += pass.outputs_found;
            rollbacks += pass.rollbacks;
            if !pass.more_blocks {
                break;
            }
            if Instant::now() >= deadline {
                return Err(anyhow!("Timed out scanning to the chain tip"));
            }
        }
        let duration = started_at.elapsed();

        let account_id = Self::account_id(&handles.pool)?;
        let end_height = Self::wallet_tip(&handles.pool, account_id)?;
        let tip_end = self.chain.tip_height().await?;

        let usage = {
            let mut inner = self.inner.lock().await;
            let usage = inner
                .sampler
                .take()
                .map(ResourceSampler::stop)
                .unwrap_or_default();
            inner.sampler = Some(ResourceSampler::start(None));
            usage
        };

        Ok(ScanMetrics {
            duration_secs: duration.as_secs_f64(),
            start_height: scan_start_height,
            end_height,
            tip_height_start: tip_at_start,
            tip_height_end: tip_end,
            blocks_scanned: end_height.saturating_sub(scan_start_height),
            blocks_per_sec: 0.0,
            outputs_found,
            // Spends are resolved while scanning, so there is no separate validation pass.
            validation_secs: 0.0,
            validation_completed: true,
            rollbacks_detected: rollbacks,
            peak_rss_bytes: 0,
            peak_cpu_percent: 0.0,
            headline_process: String::new(),
            processes: Vec::new(),
        }
        .with_processes(vec![ProcessSample {
            process: PROCESS_HARNESS.to_string(),
            peak_rss_bytes: usage.peak_rss_bytes,
            peak_cpu_percent: usage.peak_cpu_percent,
            includes_harness: true,
        }])
        .finish())
    }

    async fn scanned_height(&self) -> Result<u64> {
        let handles = self.handles().await?;
        let account_id = Self::account_id(&handles.pool)?;
        Self::wallet_tip(&handles.pool, account_id)
    }

    async fn address(&self) -> Result<String> {
        let handles = self.handles().await?;
        let conn = handles.pool.get()?;
        let account = db::get_account_by_name(&conn, ACCOUNT)?
            .ok_or_else(|| anyhow!("Account `{ACCOUNT}` is missing from the wallet database"))?;
        Ok(account
            .get_address(self.network, &self.password)?
            .to_base58())
    }

    async fn balance(&self) -> Result<Balance> {
        let handles = self.handles().await?;
        let account_id = Self::account_id(&handles.pool)?;
        let conn = handles.pool.get()?;
        let balance = db::get_balance(&conn, account_id)?;
        Ok(Balance {
            // `AccountBalance::total` is credits less debits, so it covers the locked and
            // not-yet-confirmed outputs that `available` excludes.
            total: balance.total.as_u64(),
            available: balance.available.as_u64(),
            pending_incoming: balance.unconfirmed.as_u64(),
            pending_outgoing: balance.locked.as_u64(),
            timelocked: balance.immature.as_u64(),
        })
    }

    async fn unspent_amounts(&self) -> Result<Vec<u64>> {
        // Everything the wallet holds and has not spent, whatever its confirmation state and
        // whether or not a pending transaction has it reserved: this answers "what did the
        // round put on chain", not "what can be spent".
        let handles = self.handles().await?;
        let account_id = Self::account_id(&handles.pool)?;
        let tip = Self::wallet_tip(&handles.pool, account_id)?;
        let conn = handles.pool.get()?;
        held_output_values(&conn, account_id, tip)
    }

    async fn unspent_amounts_at(&self, height: u64) -> Result<Option<Vec<u64>>> {
        // The same query `unspent_amounts` runs, pinned to a height instead of the tip.
        let handles = self.handles().await?;
        let account_id = Self::account_id(&handles.pool)?;
        let conn = handles.pool.get()?;
        Ok(Some(held_output_values(&conn, account_id, height)?))
    }

    async fn spendable_amounts(&self) -> Result<Vec<u64>> {
        let handles = self.handles().await?;
        let account_id = Self::account_id(&handles.pool)?;
        let tip = Self::wallet_tip(&handles.pool, account_id)?;
        let conn = handles.pool.get()?;

        // The same window the selector uses: `min_height = tip - confirmation_window`. Note
        // this is one block deeper than "C_min confirmations counting the mining block".
        let min_height = tip.saturating_sub(self.cfg.params.c_min);
        let outputs = db::fetch_unspent_outputs(&conn, account_id, min_height, tip)?;
        let values = outputs.iter().map(|o| o.output.value().as_u64()).collect();

        // `InputSelector` checks its available bucket — confirmed, mature, unlocked — before
        // it looks at any output, and refuses outright when that bucket cannot cover the
        // payment. The height window alone is therefore too generous.
        let available = db::get_balance(&conn, account_id)?.available.as_u64();
        Ok(crate::modes::cap_to_available(values, available))
    }

    async fn send(&self, request: SendRequest) -> Result<SendReceipt> {
        let handles = self.handles().await?;

        if request.outputs.len() == 1 {
            let output = request.outputs[0].clone();
            let pool = handles.pool.clone();
            let password = self.password.clone();
            let network = self.network;
            let c_min = self.cfg.params.c_min;
            let fee_per_gram = request.fee_per_gram;
            let lock_seconds = self.cfg.new_wallet.lock_seconds;
            let label = request.label.clone();

            // Selection, building and signing are blocking SQLite and crypto work. Keeping
            // them off the async workers is what lets S4 actually run N constructions at
            // once, so the contention measured is the wallet's, not the harness's.
            let (sender, signed, construction) = tokio::task::spawn_blocking(
                move || -> Result<(TransactionSender, SignedOneSidedTransactionResult, Duration)> {
                    let started = Instant::now();
                    let mut sender =
                        TransactionSender::new(pool, ACCOUNT.to_string(), password.clone(), network, c_min)?;
                    sender.fee_per_gram = MicroMinotari(fee_per_gram);

                    let recipient = Recipient {
                        address: parse_address(&output.address)?,
                        amount: MicroMinotari(output.amount),
                        payment_id: None,
                    };
                    let idempotency_key = format!("{}-{}", label, TxId::new_random());
                    let prepared =
                        sender.start_new_transaction(idempotency_key, recipient, lock_seconds)?;

                    let key_manager = sender.account.get_key_manager(&password)?;
                    let consensus_constants = ConsensusConstantsBuilder::new(network).build();
                    let signed =
                        sign_locked_transaction(&key_manager, consensus_constants, network, prepared)?;
                    Ok((sender, signed, started.elapsed()))
                },
            )
            .await??;

            let fee = signed_fee(&signed);
            let tx_id = u64::from(signed.signed_transaction.tx_id);
            // Remember what the scanner should find once this lands; that is how the wallet
            // is asked whether it confirmed.
            self.watch_outputs(tx_id, wallet_owned_hashes(&signed))
                .await;

            let broadcast_start = Instant::now();
            let finalized = sender
                .finalize_transaction_and_broadcast(signed, self.cfg.base_node_url.clone())
                .await?;
            // The library no longer errors when the node turns a recorded send away: it keeps
            // the send, and its inputs locked, for the monitor to rebroadcast, so it may yet be
            // mined. It is followed like any other send, with the refusal noted.
            let first_broadcast_refused = if finalized.accepted_by_node {
                None
            } else {
                Some(finalized.node_reason.unwrap_or_default())
            };
            return Ok(SendReceipt {
                tx_id: Some(tx_id),
                fee,
                construction,
                broadcast: Some(broadcast_start.elapsed()),
                broadcast_unavailable: None,
                first_broadcast_refused,
                tracker: TxTracker::wallet_tx_id(tx_id),
            });
        }

        // Batch transaction: one input set, many recipients.
        let started = Instant::now();
        let (signed, idempotency_key) = self.build_batch(&handles.pool, &request)?;
        let construction = started.elapsed();

        let fee = signed_fee(&signed);
        let tx_id = u64::from(signed.signed_transaction.tx_id);
        self.watch_outputs(tx_id, wallet_owned_hashes(&signed))
            .await;

        let broadcast_start = Instant::now();
        let first_broadcast_refused = self
            .broadcast_batch(&handles.pool, signed, &idempotency_key)
            .await?;
        Ok(SendReceipt {
            tx_id: Some(tx_id),
            fee,
            construction,
            broadcast: Some(broadcast_start.elapsed()),
            broadcast_unavailable: None,
            first_broadcast_refused,
            tracker: TxTracker::wallet_tx_id(tx_id),
        })
    }

    async fn tx_state(&self, tracker: &TxTracker) -> Result<TxState> {
        let handles = self.handles().await?;
        let account_id = Self::account_id(&handles.pool)?;
        let tip = Self::wallet_tip(&handles.pool, account_id)?;
        let watched = {
            let inner = self.inner.lock().await;
            inner
                .watched_outputs
                .get(&tracker.expect_wallet_tx_id()?)
                .cloned()
                .unwrap_or_default()
        };
        let conn = handles.pool.get()?;

        // The wallet learns a transaction landed by scanning up its own outputs, so that is
        // what is asked here.
        //
        // Since `minotari-cli` 1ab7c8c6 the scan also settles `completed_transactions`
        // (`Broadcast` -> `MinedUnconfirmed` -> `MinedConfirmed`), so the record below is
        // maintained under `ScanMode::Full` too. The outputs table stays the first source
        // anyway: it is what the same scan writes first, and reading it keeps confirmation
        // times comparable with profiles taken when the transaction monitor never ran.
        for hash in &watched {
            if let Some((height, confirmed_height)) = output_depth(&conn, account_id, hash)? {
                if height > 0 {
                    return Ok(TxState::Mined {
                        height,
                        confirmations: tip.saturating_sub(height) + 1,
                        // The scanner sets `confirmed_height` once the output is
                        // `required_confirmations` deep, and the harness passes `C_min` in as
                        // that value, so this is the wallet's own verdict at the configured
                        // depth rather than a restatement of the arithmetic above.
                        wallet_confirmed: confirmed_height.is_some(),
                    });
                }
            }
        }

        // Fall back to the wallet's own transaction record, tolerating a read failure: this
        // row has the same unmappable timestamp columns as the outputs table, and a broken
        // fallback should not turn a pending transaction into a hard error.
        let Ok(Some(tx)) =
            db::get_completed_transaction_by_id(&conn, TxId::from(tracker.expect_wallet_tx_id()?))
        else {
            return Ok(TxState::Mempool);
        };

        Ok(match tx.status {
            CompletedTransactionStatus::Rejected | CompletedTransactionStatus::Canceled => {
                TxState::Rejected {
                    reason: tx
                        .last_rejected_reason
                        .unwrap_or_else(|| tx.status.to_string()),
                }
            },
            CompletedTransactionStatus::MinedConfirmed |
            CompletedTransactionStatus::MinedUnconfirmed => match tx.mined_height {
                Some(height) if height > 0 => {
                    #[allow(clippy::cast_sign_loss)]
                    let height = height as u64;
                    TxState::Mined {
                        height,
                        confirmations: tip.saturating_sub(height) + 1,
                        wallet_confirmed: tx.status == CompletedTransactionStatus::MinedConfirmed,
                    }
                },
                _ => TxState::Mempool,
            },
            CompletedTransactionStatus::Completed | CompletedTransactionStatus::Broadcast => {
                TxState::Mempool
            },
        })
    }

    async fn refresh(&self) -> Result<()> {
        // A library wallet only moves when it is told to scan, and the scan is also what runs
        // the transaction monitor that updates confirmations.
        let handles = self.handles().await?;
        loop {
            if !self.scan_once(handles.db_path.clone()).await?.more_blocks {
                return Ok(());
            }
        }
    }

    fn resource_processes(&self) -> Vec<ProcessRole> {
        // The wallet is the harness process, so there is nothing separate to sample.
        vec![ProcessRole::with_harness(PROCESS_HARNESS)]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::seeds;

    #[test]
    fn addresses_round_trip_through_base58() {
        let words = seeds::generate_seed_words().unwrap();
        let address = seeds::derive_one_sided_address(&words, Network::Esmeralda).unwrap();
        let parsed = parse_address(&address.to_base58()).unwrap();
        assert_eq!(parsed.to_base58(), address.to_base58());
    }

    #[test]
    fn a_bad_address_is_rejected_rather_than_sent_to() {
        assert!(parse_address("not-an-address").is_err());
    }

    /// The outputs table, with just the columns these queries read.
    fn outputs_table() -> rusqlite::Connection {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE outputs (
                 id INTEGER PRIMARY KEY AUTOINCREMENT,
                 account_id INTEGER NOT NULL,
                 output_hash blob NOT NULL,
                 mined_in_block_height INTEGER NOT NULL,
                 value INTEGER NOT NULL DEFAULT 0,
                 created_at TIMESTAMP NOT NULL DEFAULT '',
                 mined_timestamp TIMESTAMP NOT NULL DEFAULT '',
                 confirmed_height INTEGER,
                 status TEXT NOT NULL DEFAULT 'UNSPENT',
                 deleted_at TIMESTAMP,
                 is_burn INTEGER NOT NULL DEFAULT 0,
                 maturity INTEGER NOT NULL DEFAULT 0
             );",
        )
        .unwrap();
        conn
    }

    fn insert_output(conn: &rusqlite::Connection, value: i64, height: i64, status: &str) {
        conn.execute(
            "INSERT INTO outputs (account_id, output_hash, mined_in_block_height, value, status) \
             VALUES (1, randomblob(32), ?1, ?2, ?3)",
            rusqlite::params![height, value, status],
        )
        .unwrap();
    }

    #[test]
    fn a_locked_output_is_still_one_the_wallet_holds() {
        let conn = outputs_table();
        insert_output(&conn, 200, 100, "UNSPENT");
        insert_output(&conn, 300, 100, "LOCKED");
        insert_output(&conn, 900, 100, "SPENT");

        // The locked one is reserved by a pending transaction, not gone: counting it is what
        // makes a rescan comparable against the wallet it replaced.
        let held = held_output_values(&conn, 1, 100).unwrap();
        assert_eq!(held.len(), 2);
        assert_eq!(held.iter().sum::<u64>(), 500);
    }

    #[test]
    fn held_outputs_are_bounded_by_height() {
        let conn = outputs_table();
        insert_output(&conn, 200, 100, "UNSPENT");
        insert_output(&conn, 400, 105, "UNSPENT");

        // A rescan finishes several blocks after the snapshot it is compared against, so the
        // later output has to be excluded when comparing at the snapshot's height.
        assert_eq!(held_output_values(&conn, 1, 100).unwrap().len(), 1);
        assert_eq!(held_output_values(&conn, 1, 105).unwrap().len(), 2);
        assert!(held_output_values(&conn, 2, 105).unwrap().is_empty());
    }

    #[test]
    fn output_depth_reads_rows_the_librarys_own_mapper_cannot() {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE outputs (
                 id INTEGER PRIMARY KEY AUTOINCREMENT,
                 account_id INTEGER NOT NULL,
                 output_hash blob NOT NULL,
                 mined_in_block_height INTEGER NOT NULL,
                 created_at TIMESTAMP NOT NULL,
                 mined_timestamp TIMESTAMP NOT NULL,
                 confirmed_height INTEGER,
                 deleted_at TIMESTAMP
             );",
        )
        .unwrap();
        let hash = FixedHash::from([7u8; 32]);
        conn.execute(
            "INSERT INTO outputs (account_id, output_hash, mined_in_block_height, created_at, \
             mined_timestamp, confirmed_height) VALUES (1, ?1, 827888, '2026-08-14 11:21:02', \
             '2026-08-14 11:20:56+00:00', 827891)",
            rusqlite::params![hash.as_slice()],
        )
        .unwrap();

        // `mined_timestamp` here is the offset-bearing format the scanner writes, the one
        // that makes `db::get_output_by_id` fail; reading only the heights sidesteps it.
        let depth = output_depth(&conn, 1, &hash).unwrap();
        assert_eq!(depth, Some((827_888, Some(827_891))));

        let missing = FixedHash::from([9u8; 32]);
        assert_eq!(output_depth(&conn, 1, &missing).unwrap(), None);
        assert_eq!(
            output_depth(&conn, 2, &hash).unwrap(),
            None,
            "another account's output"
        );
    }
}
