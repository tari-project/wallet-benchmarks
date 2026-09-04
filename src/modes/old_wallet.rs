//! Old wallet mode: `minotari_console_wallet` driven over gRPC.
//!
//! The harness owns the wallet process lifecycle. Starting a wallet on a fresh data
//! directory with seed words puts the console wallet into recovery: it scans the chain and
//! only then serves gRPC. That is why [`reset`](OldWallet::reset) returns as soon as the
//! process is spawned and [`sync_to_tip`](OldWallet::sync_to_tip) does the waiting — the time
//! between the two is the wallet's scan.

use std::{
    path::{Path, PathBuf},
    process::Stdio,
    sync::Arc,
    time::{Duration, Instant},
};

use anyhow::{anyhow, Context, Result};
use async_trait::async_trait;
use minotari_app_grpc::tari_rpc::{
    self as grpc,
    payment_recipient::PaymentType,
    wallet_client::WalletClient,
};
use tokio::{process::Child, sync::Mutex};
use tonic::transport::Channel;

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
        OLD_WALLET,
    },
    seeds::SeedEntry,
    sysprobe::ResourceSampler,
};

/// gRPC transaction status values that mean "mined and confirmed by the wallet's own rules".
const STATUS_MINED_CONFIRMED: i32 = 6;
const STATUS_ONE_SIDED_CONFIRMED: i32 = 9;
const STATUS_MINED_CONFIRMED_LOCKED: i32 = 15;
const STATUS_ONE_SIDED_CONFIRMED_LOCKED: i32 = 16;
const STATUS_COINBASE_CONFIRMED: i32 = 13;
const STATUS_COINBASE_CONFIRMED_LOCKED: i32 = 17;
/// Mined but not yet deep enough for the wallet.
const STATUS_MINED_UNCONFIRMED: i32 = 2;
const STATUS_ONE_SIDED_UNCONFIRMED: i32 = 8;
const STATUS_COINBASE_UNCONFIRMED: i32 = 12;
/// Not mined.
const STATUS_REJECTED: i32 = 7;
const STATUS_NOT_FOUND: i32 = 11;

/// Label the console wallet process is reported under in the profile's resource breakdown.
const PROCESS_CONSOLE_WALLET: &str = "console_wallet";

/// Days the console wallet steps back from the seed birthday before it starts scanning
/// (`wallet.birthday_offset`, default 2 in tari). Only used to report where the scan began.
const BIRTHDAY_OFFSET_DAYS: u64 = 2;

/// The last `lines` lines the wallet wrote, for an error message that explains itself.
fn tail_of_log(data_dir: &std::path::Path, lines: usize) -> String {
    let path = data_dir.join("wallet.log");
    match std::fs::read_to_string(&path) {
        Ok(text) => {
            let tail: Vec<&str> = text
                .lines()
                .filter(|line| !line.contains("Recovery process"))
                .rev()
                .take(lines)
                .collect();
            format!(
                "Last lines of {}:\n{}",
                path.display(),
                tail.into_iter().rev().collect::<Vec<_>>().join("\n")
            )
        },
        Err(e) => format!("Could not read {}: {e}", path.display()),
    }
}

/// The argv and environment for the `minotari_console_wallet` process.
///
/// The wallet password and the seed words go in the environment rather than in argv: argv is
/// world-readable through the process table (`ps`, `/proc/<pid>/cmdline`), so spelling them as
/// `--password`/`--seed-words` would put a wallet password and a full seed where every other
/// local user can read them — and this process lives for the whole of a mode 1 run, so that is
/// the whole run, not one short-lived call. The console wallet declares clap `env` fallbacks
/// for exactly this — `MINOTARI_WALLET_PASSWORD` and `MINOTARI_WALLET_SEED_WORDS` — and its own
/// CLI help says to use them in preference to the flags, so dropping the flags is the whole fix.
fn console_wallet_invocation<'a>(
    cfg: &HarnessConfig,
    data_dir: &Path,
    grpc_address: &str,
    password: &'a str,
    seed_words: &'a str,
) -> (Vec<String>, Vec<(&'static str, &'a str)>) {
    let args = vec![
        "--base-path".to_string(),
        data_dir.display().to_string(),
        "--network".to_string(),
        cfg.network.clone(),
        "--non-interactive-mode".to_string(),
        "--grpc-address".to_string(),
        grpc_address.to_string(),
        "-p".to_string(),
        format!("wallet.http_server_url={}", cfg.base_node_url),
        "-p".to_string(),
        format!(
            "wallet.recovery_retry_limit={}",
            cfg.old_wallet.recovery_retry_limit
        ),
    ];
    (args, vec![
        ("MINOTARI_WALLET_PASSWORD", password),
        ("MINOTARI_WALLET_SEED_WORDS", seed_words),
    ])
}

/// The wallet's `Command`, before its stdio is attached.
///
/// Split out so a test can spawn the same command the run does and check what the child
/// actually receives, rather than what the caller believes it passed.
fn console_wallet_command(
    binary: &Path,
    args: &[String],
    env: &[(&'static str, &str)],
) -> tokio::process::Command {
    let mut command = tokio::process::Command::new(binary);
    command.args(args);
    for (key, value) in env {
        command.env(key, value);
    }
    command
}

/// A `minotari_console_wallet` process plus its gRPC connection.
pub struct OldWallet {
    cfg: Arc<HarnessConfig>,
    chain: ChainClient,
    password: String,
    endpoint: String,
    inner: Mutex<Inner>,
}

#[derive(Default)]
struct Inner {
    child: Option<Child>,
    data_dir: Option<PathBuf>,
    /// When the current wallet process was spawned; the scan starts here.
    started_at: Option<Instant>,
    /// Height the wallet was told to start scanning from.
    scan_start_height: u64,
    /// Chain tip when the process was spawned.
    tip_at_start: u64,
    sampler: Option<ResourceSampler>,
    client: Option<WalletClient<Channel>>,
}

impl OldWallet {
    pub fn new(cfg: Arc<HarnessConfig>, chain: ChainClient) -> Result<Self> {
        let password = cfg.wallet_password()?;
        let endpoint = format!("http://127.0.0.1:{}", cfg.old_wallet.grpc_port);
        Ok(Self {
            cfg,
            chain,
            password,
            endpoint,
            inner: Mutex::new(Inner::default()),
        })
    }

    /// Returns a connected client, dialling the wallet if this is the first call since a
    /// reset. Waits up to `startup_timeout_secs`, because on a fresh data directory gRPC only
    /// comes up once recovery has finished.
    async fn client(&self) -> Result<WalletClient<Channel>> {
        let mut inner = self.inner.lock().await;
        if let Some(client) = &inner.client {
            return Ok(client.clone());
        }
        let deadline =
            Instant::now() + Duration::from_secs(self.cfg.old_wallet.startup_timeout_secs);
        let mut last_error = None;
        while Instant::now() < deadline {
            if let Some(child) = inner.child.as_mut() {
                if let Some(status) = child.try_wait()? {
                    // Say why, rather than pointing at a file: the reason a recovery gave up
                    // (a base node returning 503, say) belongs in the result profile.
                    let log = inner.data_dir.as_ref().map(|dir| tail_of_log(dir, 12));
                    return Err(anyhow!(
                        "The console wallet exited before serving gRPC (status {status}).\n{}",
                        log.unwrap_or_else(|| "No log was captured.".to_string())
                    ));
                }
            }
            match WalletClient::connect(self.endpoint.clone()).await {
                Ok(mut client) => {
                    // Connecting is not enough: the server accepts connections a moment
                    // before it can answer.
                    match client.get_version(grpc::GetVersionRequest {}).await {
                        Ok(_) => {
                            inner.client = Some(client.clone());
                            return Ok(client);
                        },
                        Err(e) => last_error = Some(e.to_string()),
                    }
                },
                Err(e) => last_error = Some(e.to_string()),
            }
            tokio::time::sleep(Duration::from_secs(2)).await;
        }
        Err(anyhow!(
            "Timed out after {}s waiting for the console wallet gRPC on {}: {}",
            self.cfg.old_wallet.startup_timeout_secs,
            self.endpoint,
            last_error.unwrap_or_else(|| "no response".to_string())
        ))
    }

    async fn stop_process(inner: &mut Inner) -> Result<()> {
        inner.client = None;
        if let Some(sampler) = inner.sampler.take() {
            sampler.stop();
        }
        if let Some(mut child) = inner.child.take() {
            let _ = child.kill().await;
            let _ = child.wait().await;
        }
        Ok(())
    }

    /// Height the wallet will start scanning from, so `blocks_scanned` is honest.
    ///
    /// The console wallet takes the birthday from the seed it was started with and steps
    /// `BIRTHDAY_OFFSET_DAYS` back from it before asking the base node for a height, so the
    /// same calculation is repeated here rather than assuming the birthday itself.
    async fn resolve_scan_start(&self, seed: &SeedEntry, birthday: Birthday) -> u64 {
        match birthday {
            Birthday::Genesis => 0,
            Birthday::FromSeed => self
                .chain
                .height_at_time(seed.scan_start_epoch_secs(BIRTHDAY_OFFSET_DAYS))
                .await
                .unwrap_or(0),
        }
    }

    /// Waits until revalidation stops changing what the wallet says it owns.
    ///
    /// Revalidation is asynchronous and announces no completion of its own, so the observable
    /// signal is the UTXO set: it shrinks as spent outputs are recognised, then holds still.
    /// "Still" has to mean still for a while — validating a single output took 60 s in S0, so
    /// a couple of identical readings prove nothing more than that a batch is in progress.
    ///
    /// Returns whether it settled inside the deadline. A wallet that never stops moving is a
    /// result, not an error.
    async fn wait_for_stable_utxo_set(
        &self,
        deadline: Instant,
        stable_for: Duration,
    ) -> Result<bool> {
        let mut previous: Option<(usize, u64)> = None;
        let mut last_change = Instant::now();
        loop {
            let reading = (
                self.unspent_amounts().await?.len(),
                self.balance().await?.total(),
            );
            if previous != Some(reading) {
                previous = Some(reading);
                last_change = Instant::now();
            } else if last_change.elapsed() >= stable_for {
                return Ok(true);
            }
            if Instant::now() >= deadline {
                return Ok(false);
            }
            tokio::time::sleep(self.cfg.params.poll_interval()).await;
        }
    }

    async fn transaction_info(&self, tx_id: u64) -> Result<Option<grpc::TransactionInfo>> {
        let mut client = self.client().await?;
        let response = client
            .get_transaction_info(grpc::GetTransactionInfoRequest {
                transaction_ids: vec![tx_id],
            })
            .await
            .map_err(|e| anyhow!("GetTransactionInfo failed: {e}"))?;
        Ok(response.into_inner().transactions.into_iter().next())
    }
}

#[async_trait]
impl WalletMode for OldWallet {
    fn name(&self) -> &'static str {
        OLD_WALLET
    }

    async fn version(&self) -> Result<Option<String>> {
        let mut client = self.client().await?;
        let response = client
            .get_version(grpc::GetVersionRequest {})
            .await
            .map_err(|e| anyhow!("GetVersion failed: {e}"))?;
        Ok(Some(response.into_inner().version))
    }

    async fn reset(&self, seed: &SeedEntry, birthday: Birthday) -> Result<()> {
        // `--seed-words` is the only birthday input the console wallet has, so for
        // `Birthday::Genesis` these words encode a `CipherSeed` whose birthday is 0. The
        // wallet copies that into its `WalletBirthday` setting when it stores the master
        // seed — but only on a wallet that does not exist yet, which is why the data
        // directory below is wiped rather than reused.
        let words = seed.words_for(birthday)?;
        let scan_start_height = self.resolve_scan_start(seed, birthday).await;
        let tip_at_start = self.chain.tip_height().await.unwrap_or(0);

        let mut inner = self.inner.lock().await;
        Self::stop_process(&mut inner).await?;

        let data_dir = self.cfg.wallet_dir(OLD_WALLET, &seed.label);
        if data_dir.exists() {
            std::fs::remove_dir_all(&data_dir)
                .with_context(|| format!("Could not wipe {}", data_dir.display()))?;
        }
        std::fs::create_dir_all(&data_dir)
            .with_context(|| format!("Could not create {}", data_dir.display()))?;
        let log = std::fs::File::create(data_dir.join("wallet.log"))?;
        let log_err = log.try_clone()?;

        let grpc_address = format!("/ip4/127.0.0.1/tcp/{}", self.cfg.old_wallet.grpc_port);
        let (args, env) =
            console_wallet_invocation(&self.cfg, &data_dir, &grpc_address, &self.password, &words);
        let mut command = console_wallet_command(&self.cfg.old_wallet.binary, &args, &env);
        command
            .stdin(Stdio::null())
            .stdout(Stdio::from(log))
            .stderr(Stdio::from(log_err))
            .kill_on_drop(true);

        let child = command.spawn().with_context(|| {
            format!(
                "Could not start {}. Set `old_wallet.binary` in the config.",
                self.cfg.old_wallet.binary.display()
            )
        })?;
        let pid = child.id();

        inner.started_at = Some(Instant::now());
        inner.scan_start_height = scan_start_height;
        inner.tip_at_start = tip_at_start;
        inner.data_dir = Some(data_dir);
        inner.sampler = Some(ResourceSampler::start(pid));
        inner.child = Some(child);
        Ok(())
    }

    async fn shutdown(&self) -> Result<()> {
        let mut inner = self.inner.lock().await;
        Self::stop_process(&mut inner).await
    }

    async fn sync_to_tip(&self, timeout: Duration) -> Result<ScanMetrics> {
        let (started_at, scan_start_height, tip_at_start) = {
            let mut inner = self.inner.lock().await;
            // `take` so only the first sync after a reset is charged with the start-up scan;
            // later tip refreshes are timed from here.
            (
                inner.started_at.take().unwrap_or_else(Instant::now),
                inner.scan_start_height,
                inner.tip_at_start,
            )
        };
        let deadline = Instant::now() + timeout;

        // Waiting for gRPC is waiting for recovery: the console wallet does not serve until
        // the initial scan is done.
        let mut client = self.client().await?;

        let scanned_height = loop {
            let tip = self.chain.tip_height().await?;
            let state = client
                .get_state(grpc::GetStateRequest {})
                .await
                .map_err(|e| anyhow!("GetState failed: {e}"))?
                .into_inner();
            let scanned_height = state.scanned_height;
            if scanned_height >= tip {
                break scanned_height;
            }
            if Instant::now() >= deadline {
                return Err(anyhow!(
                    "Timed out waiting for the console wallet to reach the tip (wallet at {}, tip \
                     {})",
                    scanned_height,
                    tip
                ));
            }
            tokio::time::sleep(self.cfg.params.poll_interval()).await;
        };
        let scan_only = started_at.elapsed();

        // Reaching the tip is not the same as being right. A recovery imports every output the
        // seed ever received and only learns which of them are already spent when the wallet
        // validates them against the base node afterwards; until then it reports its whole
        // history as unspent. `has_done_initial_validation` is the wallet saying it has
        // finished that pass.
        //
        // It is only ever set from a `TransactionValidationStateChanged { faux: true }` event,
        // and `check_faux_transaction_status` publishes that event `if state_changed` — so a
        // wallet with nothing to validate never emits it and the flag stays false for the life
        // of the process. Waiting on an empty wallet therefore waits for ever, which is what
        // B0 does. Only wait when the wallet actually found something.
        let has_something_to_validate = {
            let balance = self.balance().await.unwrap_or_default();
            let utxos = self.unspent_amounts().await.unwrap_or_default();
            !utxos.is_empty() || balance.total() > 0
        };
        let mut validation_completed = true;
        if has_something_to_validate {
            // `has_done_initial_validation` turns out to mean "some faux validation finished
            // since scanning did", not "every recovered output has been checked": after a
            // recovery of 639 outputs it was already true 14 ms in, with none of the spent
            // ones marked. So ask the wallet to do the work rather than watching a flag.
            //
            // `RevalidateAllTransactions` with `OutputValidationMode::Revalidate` is the
            // wallet's own reconciliation — `revalidate_all_transactions` plus
            // `revalidate_all_outputs` — which is what a recovered wallet needs before its
            // UTXO set means anything. The call returns as soon as the work is queued, so
            // what follows waits for the wallet's own view to stop moving.
            client
                .revalidate_all_transactions(grpc::RevalidateRequest {
                    transaction_mode: grpc::TransactionValidationMode::RevalidateAll as i32,
                    output_mode: grpc::OutputValidationMode::Revalidate as i32,
                })
                .await
                .map_err(|e| anyhow!("RevalidateAllTransactions failed: {e}"))?;

            let validation_deadline = deadline.min(
                Instant::now() + Duration::from_secs(self.cfg.old_wallet.validation_timeout_secs),
            );
            validation_completed = self
                .wait_for_stable_utxo_set(
                    validation_deadline,
                    Duration::from_secs(self.cfg.old_wallet.validation_stable_secs),
                )
                .await?;
        }

        let duration = started_at.elapsed();
        let validation_secs = duration.saturating_sub(scan_only).as_secs_f64();
        let tip_end = self.chain.tip_height().await?;
        let outputs_found = self
            .unspent_amounts()
            .await
            .map(|a| a.len() as u64)
            .unwrap_or(0);

        let usage = {
            let mut inner = self.inner.lock().await;
            // Stop sampling here so the peaks describe the scan, not the idle wallet, and
            // start a fresh sampler so later scenarios still get numbers.
            let usage = inner
                .sampler
                .take()
                .map(ResourceSampler::stop)
                .unwrap_or_default();
            let pid = inner.child.as_ref().and_then(Child::id);
            inner.sampler = Some(ResourceSampler::start(pid));
            usage
        };

        Ok(ScanMetrics {
            duration_secs: duration.as_secs_f64(),
            start_height: scan_start_height,
            end_height: scanned_height,
            tip_height_start: tip_at_start,
            tip_height_end: tip_end,
            blocks_scanned: scanned_height.saturating_sub(scan_start_height),
            blocks_per_sec: 0.0,
            outputs_found,
            validation_secs,
            validation_completed,
            // The console wallet logs its rollbacks but does not report them over gRPC.
            rollbacks_detected: 0,
            peak_rss_bytes: 0,
            peak_cpu_percent: 0.0,
            headline_process: String::new(),
            processes: Vec::new(),
        }
        .with_processes(vec![ProcessSample {
            process: PROCESS_CONSOLE_WALLET.to_string(),
            peak_rss_bytes: usage.peak_rss_bytes,
            peak_cpu_percent: usage.peak_cpu_percent,
            includes_harness: false,
        }])
        .finish())
    }

    async fn scanned_height(&self) -> Result<u64> {
        let mut client = self.client().await?;
        Ok(client
            .get_state(grpc::GetStateRequest {})
            .await
            .map_err(|e| anyhow!("GetState failed: {e}"))?
            .into_inner()
            .scanned_height)
    }

    async fn address(&self) -> Result<String> {
        let mut client = self.client().await?;
        let response = client
            .get_complete_address(grpc::Empty {})
            .await
            .map_err(|e| anyhow!("GetCompleteAddress failed: {e}"))?
            .into_inner();
        Ok(response.one_sided_address_base58)
    }

    async fn balance(&self) -> Result<Balance> {
        let mut client = self.client().await?;
        let response = client
            .get_balance(grpc::GetBalanceRequest { payment_id: None })
            .await
            .map_err(|e| anyhow!("GetBalance failed: {e}"))?
            .into_inner();
        Ok(Balance {
            // The console wallet has no single "everything I own" figure, so it is summed
            // here. `pending_outgoing` is deliberately left out: it is the value of the
            // inputs committed to unconfirmed outbound transactions, and the change from
            // those already shows up in `pending_incoming`, so adding it would double count
            // a transaction that is still in flight.
            total: response.available_balance +
                response.pending_incoming_balance +
                response.timelocked_balance,
            available: response.available_balance,
            pending_incoming: response.pending_incoming_balance,
            pending_outgoing: response.pending_outgoing_balance,
            timelocked: response.timelocked_balance,
        })
    }

    async fn unspent_amounts(&self) -> Result<Vec<u64>> {
        let mut client = self.client().await?;
        let response = client
            .get_unspent_amounts(grpc::Empty {})
            .await
            .map_err(|e| anyhow!("GetUnspentAmounts failed: {e}"))?
            .into_inner();
        Ok(response.amount)
    }

    async fn spendable_amounts(&self) -> Result<Vec<u64>> {
        // `GetUnspentAmounts` lists every unspent output without saying which ones the output
        // manager has encumbered, and it will list all of them while the available balance
        // covers only a fraction. Sending against that list produces "Funds are still
        // pending" once the available balance runs out, so the list is capped by it.
        let available = self.balance().await?.available;
        Ok(crate::modes::cap_to_available(
            self.unspent_amounts().await?,
            available,
        ))
    }

    async fn refresh(&self) -> Result<()> {
        // The console wallet runs its own scanning and validation services, so there is
        // nothing to drive; this just checks it is still answering.
        self.scanned_height().await.map(|_| ())
    }

    async fn send(&self, request: SendRequest) -> Result<SendReceipt> {
        let mut client = self.client().await?;
        let recipients = request
            .outputs
            .iter()
            .map(|output| grpc::PaymentRecipient {
                address: output.address.clone(),
                amount: output.amount,
                fee_per_gram: request.fee_per_gram,
                payment_type: PaymentType::OneSidedToStealthAddress as i32,
                raw_payment_id: Vec::new(),
                user_payment_id: None,
            })
            .collect();

        // The console wallet builds and broadcasts inside one call, so construction and
        // broadcast cannot be separated here; the whole call is reported as construction.
        let started = Instant::now();
        let response = client
            .transfer(grpc::TransferRequest {
                recipients,
                single_tx: request.outputs.len() > 1,
            })
            .await
            .map_err(|e| anyhow!("Transfer failed: {e}"))?
            .into_inner();
        let construction = started.elapsed();

        let result = response
            .results
            .into_iter()
            .next()
            .ok_or_else(|| anyhow!("Transfer returned no result"))?;
        if !result.is_success {
            return Err(anyhow!("Transfer rejected: {}", result.failure_message));
        }

        let mut fee = result.transaction_info.as_ref().map_or(0, |info| info.fee);
        if fee == 0 {
            fee = self
                .transaction_info(result.transaction_id)
                .await
                .ok()
                .flatten()
                .map_or(0, |info| info.fee);
        }

        Ok(SendReceipt {
            tx_id: Some(result.transaction_id),
            fee,
            construction,
            // The console wallet builds, signs and broadcasts inside one `Transfer` call and
            // reports nothing in between, so there is no broadcast to time. Recording zero
            // would claim the broadcast was instantaneous, which is a different statement.
            broadcast: None,
            broadcast_unavailable: Some(
                "the console wallet builds and broadcasts inside a single Transfer call"
                    .to_string(),
            ),
            tracker: TxTracker::wallet_tx_id(result.transaction_id),
        })
    }

    fn resource_processes(&self) -> Vec<ProcessRole> {
        vec![ProcessRole::external(PROCESS_CONSOLE_WALLET)]
    }

    async fn tx_state(&self, tracker: &TxTracker) -> Result<TxState> {
        let Some(info) = self
            .transaction_info(tracker.expect_wallet_tx_id()?)
            .await?
        else {
            return Ok(TxState::Unknown);
        };
        if info.is_cancelled || info.status == STATUS_REJECTED {
            let reason = if info.rejected_reason.is_empty() {
                "cancelled".to_string()
            } else {
                info.rejected_reason
            };
            return Ok(TxState::Rejected { reason });
        }
        if info.status == STATUS_NOT_FOUND {
            return Ok(TxState::Unknown);
        }

        // The wallet's own verdict: these are the statuses it sets once a transaction has met
        // its confirmation rule.
        let wallet_confirmed = matches!(
            info.status,
            STATUS_MINED_CONFIRMED |
                STATUS_ONE_SIDED_CONFIRMED |
                STATUS_MINED_CONFIRMED_LOCKED |
                STATUS_ONE_SIDED_CONFIRMED_LOCKED |
                STATUS_COINBASE_CONFIRMED |
                STATUS_COINBASE_CONFIRMED_LOCKED
        );
        let mined = wallet_confirmed ||
            matches!(
                info.status,
                STATUS_MINED_UNCONFIRMED |
                    STATUS_ONE_SIDED_UNCONFIRMED |
                    STATUS_COINBASE_UNCONFIRMED
            );
        if !mined || info.mined_in_block_height == 0 {
            return Ok(TxState::Mempool);
        }
        // Depth is counted against the height the *wallet* has scanned to, not the chain tip.
        // The wallet will not spend against a transaction it has not caught up with, so its
        // own height is the one that decides whether a round has landed.
        let scanned_height = self.scanned_height().await?;
        Ok(TxState::Mined {
            height: info.mined_in_block_height,
            confirmations: scanned_height.saturating_sub(info.mined_in_block_height) + 1,
            wallet_confirmed,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_constants_match_the_wallet_proto() {
        // Guards against a silent renumbering of `TransactionStatus` in wallet.proto.
        assert_eq!(
            grpc::TransactionStatus::MinedConfirmed as i32,
            STATUS_MINED_CONFIRMED
        );
        assert_eq!(
            grpc::TransactionStatus::OneSidedConfirmed as i32,
            STATUS_ONE_SIDED_CONFIRMED
        );
        assert_eq!(
            grpc::TransactionStatus::MinedUnconfirmed as i32,
            STATUS_MINED_UNCONFIRMED
        );
        assert_eq!(grpc::TransactionStatus::Rejected as i32, STATUS_REJECTED);
        assert_eq!(grpc::TransactionStatus::NotFound as i32, STATUS_NOT_FOUND);
        assert_eq!(
            grpc::TransactionStatus::MinedConfirmedLocked as i32,
            STATUS_MINED_CONFIRMED_LOCKED
        );
    }

    #[test]
    fn stealth_one_sided_is_the_payment_type_used() {
        assert_eq!(PaymentType::OneSidedToStealthAddress as i32, 2);
    }

    /// A stand-in for `minotari_console_wallet` that reports what it was actually handed.
    #[cfg(unix)]
    fn reporting_wallet(dir: &Path) -> PathBuf {
        use std::os::unix::fs::PermissionsExt;

        let script = dir.join("reporting_wallet");
        std::fs::write(
            &script,
            "#!/bin/sh\necho \"argv:$*\"\necho \"password:$MINOTARI_WALLET_PASSWORD\"\necho \
             \"seed:$MINOTARI_WALLET_SEED_WORDS\"\n",
        )
        .unwrap();
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
        script
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn the_console_wallet_is_handed_its_secrets_in_its_environment_and_not_in_its_argv() {
        // argv is world-readable through the process table, and this process runs for the
        // length of the whole mode 1 run, so a password or mnemonic spelled as a flag is
        // readable by every other local user for that entire time. Asserting on the child's own
        // view of argv and environment is the only way to catch a regression here: both
        // spellings start the same wallet, and only one of them is safe.
        let mnemonic =
            "abandon ability able about above absent absorb abstract absurd abuse access accident";
        let password = "wallet-password";
        let cfg = HarnessConfig::default();
        let dir = tempfile::tempdir().unwrap();
        let (args, env) = console_wallet_invocation(
            &cfg,
            dir.path(),
            "/ip4/127.0.0.1/tcp/18143",
            password,
            mnemonic,
        );

        let script = reporting_wallet(dir.path());
        let output = console_wallet_command(&script, &args, &env)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .output()
            .await
            .unwrap();
        assert!(output.status.success(), "{output:?}");
        let stdout = String::from_utf8_lossy(&output.stdout).to_string();

        let line = |prefix: &str| {
            stdout
                .lines()
                .find_map(|l| l.strip_prefix(prefix))
                .unwrap_or_else(|| panic!("`{prefix}` missing from:\n{stdout}"))
        };
        let argv = line("argv:");
        assert!(!argv.contains(password), "password in argv: {argv}");
        assert!(!argv.contains(mnemonic), "seed words in argv: {argv}");
        assert!(!argv.contains("--password"), "{argv}");
        assert!(!argv.contains("--seed-words"), "{argv}");
        assert_eq!(line("password:"), password);
        assert_eq!(line("seed:"), mnemonic);
    }
}
