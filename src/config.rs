//! Harness configuration.
//!
//! Everything that shapes a run lives here and is copied verbatim into the result profile, so
//! a third party can reproduce a run from the profile alone. Secrets never live in the config
//! file: the wallet password is read from the environment variable named by
//! [`HarnessConfig::password_env`], and seed words live in the (git-ignored) seed file.

use std::{path::PathBuf, time::Duration};

use anyhow::{anyhow, Context, Result};
use serde::{Deserialize, Serialize};

/// Top level harness configuration, normally read from `benchmark.toml`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct HarnessConfig {
    /// Tari network to run against, e.g. `esmeralda`.
    pub network: String,
    /// HTTP RPC endpoint of a synced base node. Both wallet modes talk to this node.
    pub base_node_url: String,
    /// Working directory for wallet data dirs. Wiped per scenario as the protocol requires.
    pub data_dir: PathBuf,
    /// Directory the result profiles are written to.
    pub results_dir: PathBuf,
    /// File holding the generated seed words (never committed).
    pub seed_file: PathBuf,
    /// Name of the environment variable carrying the wallet password.
    pub password_env: String,
    /// Old wallet (`minotari_console_wallet`) settings.
    pub old_wallet: OldWalletConfig,
    /// New wallet (`minotari-cli` library) settings.
    pub new_wallet: NewWalletConfig,
    /// Test protocol parameters. These are the knobs the issue specifies.
    pub params: Params,
}

impl Default for HarnessConfig {
    fn default() -> Self {
        Self {
            network: "esmeralda".to_string(),
            base_node_url: "https://rpc.esmeralda.tari.com".to_string(),
            data_dir: PathBuf::from("data"),
            results_dir: PathBuf::from("results"),
            seed_file: PathBuf::from("data/seeds.json"),
            password_env: "HARNESS_WALLET_PW".to_string(),
            old_wallet: OldWalletConfig::default(),
            new_wallet: NewWalletConfig::default(),
            params: Params::default(),
        }
    }
}

/// Settings for the wallet process the harness spawns and drives over gRPC.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct OldWalletConfig {
    /// Path to the `minotari_console_wallet` binary.
    pub binary: PathBuf,
    /// Port the harness tells the wallet to serve gRPC on (bound to loopback).
    pub grpc_port: u16,
    /// How long to wait for the wallet process to come up and answer gRPC.
    pub startup_timeout_secs: u64,
    /// How many times the wallet retries a failed recovery before giving up
    /// (`wallet.recovery_retry_limit`, 3 in tari).
    ///
    /// Raise it when running against a public base node gateway: a single 503 part way
    /// through a genesis scan otherwise ends the whole run. A retried recovery resumes where
    /// it left off, and the retries are included in the reported `T_scan`.
    pub recovery_retry_limit: usize,
    /// How long to wait for the wallet to validate what a recovery found, before giving up on
    /// the wait and recording that it did not finish.
    ///
    /// Bounded separately from the scan: a wallet that never reports validation complete
    /// should cost this much of the run, not all of it.
    pub validation_timeout_secs: u64,
    /// How long the wallet's UTXO set has to stop changing before its revalidation is treated
    /// as finished.
    ///
    /// Revalidation reports no completion of its own, so the harness watches for the set to
    /// hold still. Too short and a pause between batches reads as done; validating a single
    /// output has been observed to take 60 s.
    pub validation_stable_secs: u64,
}

impl Default for OldWalletConfig {
    fn default() -> Self {
        Self {
            binary: PathBuf::from("minotari_console_wallet"),
            grpc_port: 18_143,
            startup_timeout_secs: 7_200,
            recovery_retry_limit: 10,
            validation_timeout_secs: 1_800,
            validation_stable_secs: 60,
        }
    }
}

/// Settings for the in-process library wallet.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct NewWalletConfig {
    /// Blocks fetched per scan batch.
    pub scan_batch_size: u64,
    /// Output detection threads used by the scanner.
    pub scan_threads: usize,
    /// How long selected UTXOs stay locked while a transaction is built and broadcast.
    pub lock_seconds: u64,
    /// Connections in the wallet database pool, or 0 to size it from the S4 ramp.
    ///
    /// The library builds its own with 5, so anything past a handful of concurrent sends
    /// queues on connections rather than on the wallet. Left at 0 the harness sizes it to the
    /// ramp's peak and records what it used, so what S4 measures is the wallet's contention
    /// rather than the pool's.
    pub db_pool_size: u32,
    /// How long a send waits for a database connection before giving up.
    ///
    /// r2d2 defaults to 30 seconds, which turns a queue into a wall of failures under load.
    /// Waiting is what S4 wants to measure.
    pub db_checkout_timeout_secs: u64,
}

impl Default for NewWalletConfig {
    fn default() -> Self {
        Self {
            scan_batch_size: 100,
            scan_threads: 8,
            lock_seconds: 300,
            db_pool_size: 0,
            db_checkout_timeout_secs: 1_200,
        }
    }
}

/// Test protocol parameters. Defaults are the reference values from the bounty spec.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Params {
    /// Single funding UTXO per mode, in µT. Default 10,000 tXTM.
    pub a_fund: u64,
    /// Confirmation depth before a UTXO counts as spendable.
    pub c_min: u64,
    /// Target UTXO count after S1.
    pub volume_target: u64,
    /// Doubling rounds in S1 (1 → 2^rounds UTXOs).
    pub doubling_rounds: u32,
    /// Outputs per transaction in the S1 fan-out round.
    pub fanout_outputs_per_tx: u32,
    /// Concurrency ramp for S4.
    pub concurrent_batches: Vec<u32>,
    /// Stop condition per S4 batch, in seconds.
    pub s4_budget_secs: u64,
    /// Recipients paid in S5.
    pub s5_m: u32,
    /// Outputs per batch transaction in S5.
    pub s5_k: u32,
    /// Fee rate (µT per gram) passed explicitly to every mode.
    pub fee_rate: u64,
    /// Amount sent per output in S4, in µT.
    pub s4_amount: u64,
    /// Amount sent per recipient in S5, in µT.
    pub s5_amount: u64,
    /// How long to wait for a transaction to reach `c_min` confirmations.
    pub confirm_timeout_secs: u64,
    /// How long to wait for a wallet to scan to the chain tip.
    pub scan_timeout_secs: u64,
    /// How long S0 waits for the operator-supplied funding UTXO to become spendable.
    pub fund_timeout_secs: u64,
    /// Poll interval used by every wait loop.
    pub poll_interval_secs: u64,
}

impl Default for Params {
    fn default() -> Self {
        Self {
            a_fund: 10_000_000_000,
            c_min: 3,
            volume_target: 512,
            doubling_rounds: 6,
            fanout_outputs_per_tx: 8,
            concurrent_batches: vec![8, 16, 32, 64, 128],
            s4_budget_secs: 900,
            s5_m: 100,
            s5_k: 10,
            fee_rate: 5,
            s4_amount: 100_000,
            s5_amount: 100_000,
            confirm_timeout_secs: 1_800,
            scan_timeout_secs: 7_200,
            fund_timeout_secs: 3_600,
            poll_interval_secs: 10,
        }
    }
}

/// Database connections one send holds at the same time.
///
/// One, since `minotari-cli` 84c79e9 ("db connection pool starvation"): a send takes a
/// connection and threads it through selection, locking and the write transaction, and
/// `FundLocker::lock` uses the caller's rather than taking one of its own.
///
/// Before that fix it was three, and a pool smaller than three times the concurrency
/// deadlocked — every connection held by a send that could not finish without another one,
/// nothing moving until the checkout timeout fired. At N = 64 against a 64-connection pool
/// that was 6001 s of construction and zero transactions. Raise this again if the wallet ever
/// goes back to nesting checkouts.
const CONNECTIONS_PER_SEND: u32 = 1;

/// Spare connections for the scenario's own reads — balances, UTXO sets, confirmations.
const POOL_HEADROOM: u32 = 16;

/// One round of the S1 doubling phase.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct RoundPlan {
    /// 1-based round number.
    pub round: u32,
    /// Transactions broadcast serially in this round.
    pub txs: u64,
    /// Outputs each transaction creates (payment outputs plus change).
    pub outputs_per_tx: u32,
    /// UTXO count the wallet should hold once the round confirms.
    pub utxos_after: u64,
}

impl Params {
    /// The S1 doubling schedule for a wallet that starts on `start_utxos` UTXOs: round *r*
    /// spends every UTXO once, splitting each in two.
    ///
    /// The reference protocol starts from a single funding UTXO, but an operator funding
    /// from a faucet or a miner wallet often ends up with several. Scaling the plan to what
    /// the wallet actually holds keeps the shape of the scenario (every UTXO spent once per
    /// round) instead of failing on the funding shape; the run records the count it started
    /// from so a profile is never mistaken for a reference one.
    pub fn doubling_plan_from(&self, start_utxos: u64) -> Vec<RoundPlan> {
        let start = start_utxos.max(1);
        (1..=self.doubling_rounds)
            .map(|round| RoundPlan {
                round,
                txs: start << (round - 1),
                outputs_per_tx: 2,
                utxos_after: start << round,
            })
            .collect()
    }

    /// The S1 fan-out round: every UTXO is split into `fanout_outputs_per_tx` outputs.
    pub fn fanout_plan_from(&self, start_utxos: u64) -> RoundPlan {
        let txs = start_utxos.max(1) << self.doubling_rounds;
        RoundPlan {
            round: self.doubling_rounds + 1,
            txs,
            outputs_per_tx: self.fanout_outputs_per_tx,
            utxos_after: txs * u64::from(self.fanout_outputs_per_tx),
        }
    }

    /// UTXO count S1 ends on when it starts from `start_utxos`.
    pub fn expected_utxos_from(&self, start_utxos: u64) -> u64 {
        self.fanout_plan_from(start_utxos).utxos_after
    }

    /// The reference doubling schedule, from a single funding UTXO.
    pub fn doubling_plan(&self) -> Vec<RoundPlan> {
        self.doubling_plan_from(1)
    }

    /// The reference fan-out round, from a single funding UTXO.
    pub fn fanout_plan(&self) -> RoundPlan {
        self.fanout_plan_from(1)
    }

    /// UTXO count S1 is expected to end on in the reference protocol.
    pub fn expected_utxos_after_s1(&self) -> u64 {
        self.expected_utxos_from(1)
    }

    /// Number of batch transactions in S5 arm A.
    pub fn s5_batch_txs(&self) -> u32 {
        self.s5_m / self.s5_k
    }

    pub fn confirm_timeout(&self) -> Duration {
        Duration::from_secs(self.confirm_timeout_secs)
    }

    pub fn scan_timeout(&self) -> Duration {
        Duration::from_secs(self.scan_timeout_secs)
    }

    pub fn fund_timeout(&self) -> Duration {
        Duration::from_secs(self.fund_timeout_secs)
    }

    pub fn poll_interval(&self) -> Duration {
        Duration::from_secs(self.poll_interval_secs.max(1))
    }
}

impl HarnessConfig {
    /// Reads a config file. A missing file is an error: the harness never guesses which
    /// network or base node a recorded profile belongs to.
    pub fn load(path: &std::path::Path) -> Result<Self> {
        let text = std::fs::read_to_string(path)
            .with_context(|| format!("Could not read config file {}", path.display()))?;
        let mut cfg: Self = toml::from_str(&text)
            .with_context(|| format!("Could not parse config file {}", path.display()))?;
        cfg.resolve_derived();
        cfg.validate()?;
        Ok(cfg)
    }

    /// The wallet password, read from the environment variable named in the config.
    pub fn wallet_password(&self) -> Result<String> {
        std::env::var(&self.password_env).map_err(|_| {
            anyhow!(
                "Wallet password environment variable `{}` is not set",
                self.password_env
            )
        })
    }

    /// The configured network as a `tari_common` network.
    pub fn tari_network(&self) -> Result<tari_common::configuration::Network> {
        self.network
            .parse()
            .map_err(|_| anyhow!("Unknown network `{}`", self.network))
    }

    /// The smallest connection pool the S4 ramp can run through without deadlocking.
    pub fn required_db_pool_size(&self) -> u32 {
        let peak = self
            .params
            .concurrent_batches
            .iter()
            .copied()
            .max()
            .unwrap_or(1);
        peak.saturating_mul(CONNECTIONS_PER_SEND)
            .saturating_add(POOL_HEADROOM)
    }

    /// Fills in the settings that are derived from others, so the recorded config states the
    /// values the run actually used rather than the zeros that stood for "work it out".
    pub fn resolve_derived(&mut self) {
        if self.new_wallet.db_pool_size == 0 {
            self.new_wallet.db_pool_size = self.required_db_pool_size();
        }
    }

    /// Rejects parameter combinations that cannot produce a meaningful run.
    pub fn validate(&self) -> Result<()> {
        let p = &self.params;
        if p.a_fund == 0 {
            return Err(anyhow!("params.a_fund must be greater than zero"));
        }
        if p.c_min == 0 {
            return Err(anyhow!("params.c_min must be at least 1"));
        }
        if p.fee_rate == 0 {
            return Err(anyhow!("params.fee_rate must be at least 1"));
        }
        if p.doubling_rounds == 0 || p.doubling_rounds > 20 {
            return Err(anyhow!("params.doubling_rounds must be between 1 and 20"));
        }
        if p.fanout_outputs_per_tx < 2 {
            return Err(anyhow!("params.fanout_outputs_per_tx must be at least 2"));
        }
        if p.expected_utxos_after_s1() != p.volume_target {
            return Err(anyhow!(
                "params.volume_target ({}) does not match the build-up plan (2^{} * {} = {})",
                p.volume_target,
                p.doubling_rounds,
                p.fanout_outputs_per_tx,
                p.expected_utxos_after_s1()
            ));
        }
        if p.concurrent_batches.is_empty() {
            return Err(anyhow!("params.concurrent_batches must list at least one level"));
        }
        if p.concurrent_batches.contains(&0) {
            return Err(anyhow!("params.concurrent_batches may not contain zero"));
        }
        if p.s5_k == 0 || p.s5_m == 0 {
            return Err(anyhow!("params.s5_m and params.s5_k must be greater than zero"));
        }
        if !p.s5_m.is_multiple_of(p.s5_k) {
            return Err(anyhow!(
                "params.s5_m ({}) must be a whole multiple of params.s5_k ({})",
                p.s5_m,
                p.s5_k
            ));
        }
        // Too small a pool does not queue, it deadlocks: see `CONNECTIONS_PER_SEND`.
        let required_pool = self.required_db_pool_size();
        if self.new_wallet.db_pool_size != 0 && self.new_wallet.db_pool_size < required_pool {
            return Err(anyhow!(
                "new_wallet.db_pool_size ({}) is below the {required_pool} needed for a peak \
                 concurrency of {} ({CONNECTIONS_PER_SEND} connections per send plus \
                 {POOL_HEADROOM} spare); a pool this size deadlocks rather than queues. Set 0 \
                 to derive it.",
                self.new_wallet.db_pool_size,
                self.params.concurrent_batches.iter().copied().max().unwrap_or(1),
            ));
        }
        // A send that queues for a connection longer than this fails with a pool error, so a
        // checkout timeout below the S4 budget means the plumbing gives up before the
        // scenario does and the level's numbers describe the pool instead of the wallet.
        if self.new_wallet.db_checkout_timeout_secs < p.s4_budget_secs {
            return Err(anyhow!(
                "new_wallet.db_checkout_timeout_secs ({}) is below params.s4_budget_secs ({}); \
                 sends would fail on the connection pool before S4 stopped waiting for them",
                self.new_wallet.db_checkout_timeout_secs,
                p.s4_budget_secs
            ));
        }
        if self.base_node_url.trim().is_empty() {
            return Err(anyhow!("base_node_url must be set"));
        }
        self.tari_network()?;
        Ok(())
    }

    /// Wallet data directory for one mode and wallet slot.
    pub fn wallet_dir(&self, mode: &str, slot: &str) -> PathBuf {
        self.data_dir.join(mode).join(slot)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_are_the_reference_parameters() {
        let cfg = HarnessConfig::default();
        assert_eq!(cfg.params.a_fund, 10_000_000_000);
        assert_eq!(cfg.params.c_min, 3);
        assert_eq!(cfg.params.volume_target, 512);
        assert_eq!(cfg.params.concurrent_batches, vec![8, 16, 32, 64, 128]);
        cfg.validate().expect("reference defaults must validate");
    }

    #[test]
    fn doubling_plan_matches_the_specified_table() {
        let p = Params::default();
        let plan = p.doubling_plan();
        assert_eq!(plan.len(), 6);
        assert_eq!(plan[0].txs, 1);
        assert_eq!(plan[0].utxos_after, 2);
        assert_eq!(plan[5].txs, 32);
        assert_eq!(plan[5].utxos_after, 64);
        assert!(plan.iter().all(|r| r.outputs_per_tx == 2));
    }

    #[test]
    fn fanout_takes_64_utxos_to_512() {
        let p = Params::default();
        let fanout = p.fanout_plan();
        assert_eq!(fanout.txs, 64);
        assert_eq!(fanout.outputs_per_tx, 8);
        assert_eq!(fanout.utxos_after, 512);
        assert_eq!(p.expected_utxos_after_s1(), p.volume_target);
        assert_eq!(p.s5_batch_txs(), 10);
    }

    #[test]
    fn the_build_up_plan_scales_to_the_funding_shape() {
        let p = Params::default();
        // A wallet funded with four UTXOs still spends each of them once per round.
        let plan = p.doubling_plan_from(4);
        assert_eq!(plan[0].txs, 4);
        assert_eq!(plan[0].utxos_after, 8);
        assert_eq!(plan[5].txs, 128);
        assert_eq!(plan[5].utxos_after, 256);

        let fanout = p.fanout_plan_from(4);
        assert_eq!(fanout.txs, 256);
        assert_eq!(fanout.utxos_after, 2_048);
        assert_eq!(p.expected_utxos_from(4), 2_048);

        // Starting from nothing is treated as starting from one rather than collapsing the
        // plan to zero transactions.
        assert_eq!(p.expected_utxos_from(0), p.expected_utxos_after_s1());
    }

    #[test]
    fn volume_target_must_agree_with_the_build_up_plan() {
        let mut cfg = HarnessConfig::default();
        cfg.params.volume_target = 500;
        let err = cfg.validate().unwrap_err().to_string();
        assert!(err.contains("volume_target"), "unexpected error: {err}");
    }

    #[test]
    fn the_pool_is_sized_so_the_ramp_cannot_deadlock() {
        let mut cfg = HarnessConfig::default();
        // One connection per send at the ramp's peak of 128, plus spare reads.
        assert_eq!(cfg.required_db_pool_size(), 128 + 16);

        cfg.resolve_derived();
        assert_eq!(cfg.new_wallet.db_pool_size, 144, "0 means work it out");
        cfg.validate().unwrap();

        // Smaller than the ramp's peak: the level would queue on connections instead of
        // measuring the wallet.
        cfg.new_wallet.db_pool_size = 64;
        let err = cfg.validate().unwrap_err().to_string();
        assert!(err.contains("deadlocks"), "unexpected error: {err}");
    }

    #[test]
    fn a_smaller_ramp_needs_a_smaller_pool() {
        let mut cfg = HarnessConfig::default();
        cfg.params.concurrent_batches = vec![4, 8];
        assert_eq!(cfg.required_db_pool_size(), 8 + 16);
        cfg.resolve_derived();
        assert_eq!(cfg.new_wallet.db_pool_size, 24);
    }

    #[test]
    fn the_pool_may_not_give_up_before_s4_does() {
        let mut cfg = HarnessConfig::default();
        cfg.params.s4_budget_secs = 5_400;
        cfg.new_wallet.db_checkout_timeout_secs = 1_200;
        let err = cfg.validate().unwrap_err().to_string();
        assert!(err.contains("db_checkout_timeout_secs"), "unexpected error: {err}");

        // Raising the pool's patience to match makes it valid again.
        cfg.new_wallet.db_checkout_timeout_secs = 6_000;
        cfg.validate().unwrap();
    }

    #[test]
    fn s5_recipients_must_divide_into_batches() {
        let mut cfg = HarnessConfig::default();
        cfg.params.s5_k = 7;
        assert!(cfg.validate().is_err());
    }

    #[test]
    fn unknown_keys_are_rejected() {
        let err = toml::from_str::<HarnessConfig>("network = \"esmeralda\"\nnonsense = 1\n")
            .unwrap_err()
            .to_string();
        assert!(err.contains("nonsense"), "unexpected error: {err}");
    }

    #[test]
    fn partial_config_falls_back_to_defaults() {
        let cfg: HarnessConfig = toml::from_str(
            r#"
            network = "esmeralda"
            base_node_url = "http://127.0.0.1:18142"

            [params]
            fee_rate = 9
            doubling_rounds = 3
            volume_target = 64
            "#,
        )
        .unwrap();
        assert_eq!(cfg.params.fee_rate, 9);
        assert_eq!(cfg.params.c_min, 3);
        assert_eq!(cfg.params.expected_utxos_after_s1(), 64);
        cfg.validate().unwrap();
    }

    #[test]
    fn config_round_trips_through_toml() {
        let cfg = HarnessConfig::default();
        let text = toml::to_string(&cfg).unwrap();
        let parsed: HarnessConfig = toml::from_str(&text).unwrap();
        assert_eq!(cfg, parsed);
    }
}
