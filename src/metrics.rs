//! Measurement types and the result profile.
//!
//! Every scenario returns a [`ScenarioReport`]; a run collects them into a [`ResultProfile`]
//! together with the environment, the pinned versions, the full configuration and the
//! computed cross-scenario deltas. The profile is the artefact that gets committed.

use std::{collections::BTreeMap, fmt::Write};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::{config::HarnessConfig, sysprobe::EnvironmentInfo};

/// Git revision of `minotari-cli` this harness is pinned to. Keep in step with `Cargo.toml`.
///
/// Bumped from `84c79e9c` to current main. The five intervening commits are all
/// transaction-reservation and idempotency fixes — atomic send idempotency, replayed sends no
/// longer tearing down or double-spending a reservation, teardown guarded on the reservation
/// still being pending, and the unlocker kept off claimed rows. That is exactly the ground S4
/// stresses, so `new_wallet`'s S4 numbers are expected to move against any profile taken on
/// the old pin.
pub const MINOTARI_CLI_REV: &str = "60fde7b39e20a9d92c3b987a25eb636bf4da2722";

/// How a scenario ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Outcome {
    /// Ran to completion and every check passed.
    Ok,
    /// Ran, but a verification step or a wallet operation failed. Still a result.
    Failed,
    /// Not run (not requested, or a precondition from an earlier scenario is missing).
    Skipped,
}

/// Per-transaction measurements.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct TxMetric {
    /// Where this transaction came from, e.g. `s1-round-3-tx-2`.
    pub label: String,
    /// The wallet's own transaction id, when the mode exposes one. `payment_processor` does
    /// not; `payment_ids` carries its handle instead.
    pub tx_id: Option<u64>,
    /// The payment processor's handles for this send: the batch id and the payment ids it
    /// created. Empty for the modes that own the transaction outright.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub payment_ids: Vec<String>,
    /// The payment processor batch this send became. `None` for the other modes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub batch_id: Option<String>,
    /// Payment outputs requested (change excluded).
    pub outputs: u32,
    /// Total amount sent, in µT.
    pub amount: u64,
    /// Fee paid, in µT. Zero when the transaction never made it onto the chain.
    pub fee: u64,
    /// Time spent building and signing.
    pub construction_secs: f64,
    /// Time from the end of construction to the node accepting the transaction.
    ///
    /// `None` when the wallet gives the harness no way to observe the broadcast at all;
    /// `broadcast_unavailable` then says why. Deliberately not zero — zero is a claim that
    /// the broadcast was instantaneous, which is a different (and false) statement.
    #[serde(default)]
    pub broadcast_secs: Option<f64>,
    /// Why `broadcast_secs` is absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub broadcast_unavailable: Option<String>,
    /// Offset from the batch start at which the send was submitted.
    pub submitted_at_secs: f64,
    /// Offset from the batch start at which construction completed. S4 uses the gaps between
    /// consecutive values of this field to expose internal serialisation.
    pub construction_done_secs: f64,
    /// Offset from the batch start at which the transaction reached `C_min` confirmations.
    pub confirmed_at_secs: Option<f64>,
    pub outcome: TxOutcome,
    pub error: Option<String>,
    /// Post-run enrichment: what the chain and the wallet said about this transaction once it
    /// had settled. Only `payment_processor` fills this in — the other modes know all of it
    /// at send time and record it in the fields above.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub enrichment: Option<TxEnrichment>,
}

/// What a post-run lookup added to a transaction that the send itself could not report.
///
/// The payment processor exposes no transaction id and persists no per-state timestamps, so
/// everything here has to be recovered afterwards: the on-chain identity through the
/// `payref` the daemon records at confirmation, and the transition timeline through the
/// processor's `events` table.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct TxEnrichment {
    /// Reference the payment processor assigns at confirmation; the only handle that reaches
    /// the chain.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub payrefs: Vec<String>,
    /// The daemon's own transaction id, looked up by `payref`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub wallet_tx_id: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kernel_excess_hex: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mined_height: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub confirmation_height: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub broadcast_attempts: Option<i64>,
    /// Fee of the whole transaction, in µT, as read back from the chain.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub transaction_fee: Option<u64>,
    /// `transaction_fee` divided across the payments that shared the batch. Reporting the
    /// whole fee against each payment would multiply one fee by the batch width.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fee_per_recipient: Option<f64>,
    /// `(event_type, timestamp)` in the order the processor recorded them.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub timeline: Vec<(String, String)>,
}

/// Terminal state of a single transaction.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TxOutcome {
    /// Broadcast accepted and confirmed at `C_min`.
    Confirmed,
    /// Broadcast accepted, still not confirmed when the scenario stopped waiting.
    #[default]
    Broadcast,
    /// Construction or broadcast failed.
    Failed,
    /// Did not reach a terminal state inside the scenario's time budget.
    TimedOut,
}

/// Failure messages that mean the harness's own plumbing gave out rather than the wallet
/// rejecting anything: the database pool, or a lock the harness holds.
const INFRASTRUCTURE_MARKERS: [&str; 3] = [
    "timed out waiting for connection",
    "Failed to acquire database connection",
    "database is locked",
];

/// Counts failures caused by the harness rather than the wallet.
///
/// A send rejected for "Funds are pending" is a measurement; one that died waiting for a
/// database connection is noise, and counting them together turns a concurrency scenario into
/// a report on connection limits.
pub fn count_infrastructure_failures(metrics: &[TxMetric]) -> u32 {
    metrics
        .iter()
        .filter(|m| m.outcome == TxOutcome::Failed)
        .filter_map(|m| m.error.as_deref())
        .filter(|error| {
            INFRASTRUCTURE_MARKERS
                .iter()
                .any(|marker| error.contains(marker))
        })
        .count() as u32
}

/// Scan measurements, shared by B0, S2, S3 and S6.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ScanMetrics {
    /// Wall clock of the scan, `T_scan`.
    pub duration_secs: f64,
    /// Height the wallet started scanning from.
    pub start_height: u64,
    /// Height the wallet finished on.
    pub end_height: u64,
    pub tip_height_start: u64,
    pub tip_height_end: u64,
    pub blocks_scanned: u64,
    pub blocks_per_sec: f64,
    pub outputs_found: u64,
    /// Time spent, after reaching the tip, on the wallet validating what it found.
    ///
    /// The console wallet's recovery imports every output the seed ever received and only
    /// marks the already-spent ones once it has validated them against the base node, so this
    /// is part of the cost of a rescan and is included in `duration_secs`. The library wallet
    /// resolves spends while it scans and leaves this at zero.
    pub validation_secs: f64,
    /// Whether the wallet finished validating what it found, or had nothing to validate.
    ///
    /// False means the numbers below describe a wallet that has not yet reconciled its
    /// recovered outputs against the chain, and will over-report both UTXOs and balance.
    pub validation_completed: bool,
    /// Chain rollbacks the wallet detected during this scan.
    ///
    /// A scan that keeps rolling back is not measuring steady-state scan speed. Against a
    /// load-balanced public gateway this usually means the nodes behind it disagree about the
    /// tip rather than that the chain reorganised, so it is disclosed rather than smoothed
    /// over. Only the library wallet reports it; the console wallet's gRPC exposes no
    /// equivalent signal, and leaves it at zero.
    pub rollbacks_detected: u64,
    /// Peak RSS of the headline process.
    pub peak_rss_bytes: u64,
    /// Peak CPU of the headline process.
    pub peak_cpu_percent: f32,
    /// Label of the process the two figures above describe.
    #[serde(default)]
    pub headline_process: String,
    /// Every process this mode attributes resource usage to, headline first.
    ///
    /// A single-process mode reports one entry. `payment_processor` reports the daemon as the
    /// headline — it is the wallet — with the payment processor alongside it, because a
    /// single number for a three-process mode would describe neither.
    #[serde(default)]
    pub processes: Vec<ProcessSample>,
}

/// Peak resource usage of one named process over a measurement window.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ProcessSample {
    /// Stable label, e.g. `console_wallet`, `harness`, `daemon`, `payment_processor`.
    pub process: String,
    pub peak_rss_bytes: u64,
    pub peak_cpu_percent: f32,
    /// True when this process also runs the harness, so its figures include it.
    pub includes_harness: bool,
}

impl ScanMetrics {
    /// Records the per-process peaks and promotes the first one to the headline figures.
    pub fn with_processes(mut self, processes: Vec<ProcessSample>) -> Self {
        if let Some(headline) = processes.first() {
            self.headline_process = headline.process.clone();
            self.peak_rss_bytes = headline.peak_rss_bytes;
            self.peak_cpu_percent = headline.peak_cpu_percent;
        }
        self.processes = processes;
        self
    }

    /// Fills in the derived rate. Call once the durations and block counts are known.
    pub fn finish(mut self) -> Self {
        self.blocks_per_sec = if self.duration_secs > 0.0 {
            #[allow(clippy::cast_precision_loss)]
            let blocks = self.blocks_scanned as f64;
            blocks / self.duration_secs
        } else {
            0.0
        };
        self
    }
}

/// Everything one scenario measured on one mode.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ScenarioReport {
    pub scenario: String,
    pub mode: String,
    pub outcome: Outcome,
    pub error: Option<String>,
    pub started_at: DateTime<Utc>,
    pub wall_clock_secs: f64,
    pub tip_height_start: u64,
    pub tip_height_end: u64,
    pub balance_start: u64,
    pub balance_end: u64,
    /// What the balance should be, when the scenario can predict it.
    pub expected_balance_end: Option<u64>,
    /// `expected - observed`. Anything other than zero is flagged.
    pub balance_delta: Option<i128>,
    pub utxos_start: u64,
    pub utxos_end: u64,
    pub fees_paid: u64,
    pub successes: u32,
    pub failures: u32,
    pub failure_reasons: Vec<String>,
    pub scan: Option<ScanMetrics>,
    pub transactions: Vec<TxMetric>,
    /// Scenario specific values (S4 ramps, S5 arms, verification results, ...).
    pub notes: BTreeMap<String, Value>,
}

impl ScenarioReport {
    pub fn new(scenario: &str, mode: &str) -> Self {
        Self {
            scenario: scenario.to_string(),
            mode: mode.to_string(),
            outcome: Outcome::Ok,
            error: None,
            started_at: Utc::now(),
            wall_clock_secs: 0.0,
            tip_height_start: 0,
            tip_height_end: 0,
            balance_start: 0,
            balance_end: 0,
            expected_balance_end: None,
            balance_delta: None,
            utxos_start: 0,
            utxos_end: 0,
            fees_paid: 0,
            successes: 0,
            failures: 0,
            failure_reasons: Vec::new(),
            scan: None,
            transactions: Vec::new(),
            notes: BTreeMap::new(),
        }
    }

    /// A scenario that did not run, with the reason why.
    pub fn skipped(scenario: &str, mode: &str, reason: impl Into<String>) -> Self {
        let mut report = Self::new(scenario, mode);
        report.outcome = Outcome::Skipped;
        report.error = Some(reason.into());
        report
    }

    /// Records a failure. The scenario keeps its measurements; only the verdict changes.
    pub fn fail(&mut self, reason: impl Into<String>) {
        let reason = reason.into();
        self.outcome = Outcome::Failed;
        self.failures += 1;
        if self.error.is_none() {
            self.error = Some(reason.clone());
        }
        self.failure_reasons.push(reason);
    }

    pub fn note(&mut self, key: &str, value: impl Serialize) {
        let value = serde_json::to_value(value).unwrap_or(Value::Null);
        self.notes.insert(key.to_string(), value);
    }

    /// Reconciles the closing balance against what the scenario expected.
    pub fn reconcile(&mut self, expected: u64) {
        self.expected_balance_end = Some(expected);
        self.balance_delta = Some(i128::from(expected) - i128::from(self.balance_end));
    }

    /// Re-reconciles after fees that were not knowable while the scenario ran.
    ///
    /// Every scenario reconciles its closing balance against `something - fees`, and does so
    /// before it returns. `payment_processor` does not know a single fee at that point: the
    /// processor reports none, and the transaction only becomes readable off the chain once
    /// its payment has confirmed and a `payref` exists. Without this the whole of that mode's
    /// fee spend would show up as an unexplained balance shortfall in every sending scenario.
    ///
    /// `fees_before` is what `fees_paid` held when the scenario reconciled. The expectation
    /// moves down by whatever the enrichment discovered on top of that, and the delta is
    /// recomputed against the balance that was already read.
    pub fn account_for_late_fees(&mut self, fees_before: u64) {
        let discovered = self.total_recorded_fees();
        self.fees_paid = discovered;
        let extra = discovered.saturating_sub(fees_before);
        if extra == 0 {
            return;
        }
        if let Some(expected) = self.expected_balance_end {
            self.reconcile(expected.saturating_sub(extra));
        }
    }

    /// Sum of fees over the recorded transactions.
    pub fn total_recorded_fees(&self) -> u64 {
        self.transactions.iter().map(|t| t.fee).sum()
    }

    pub fn scan_duration(&self) -> Option<f64> {
        self.scan.as_ref().map(|s| s.duration_secs)
    }
}

/// Pinned software versions.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Versions {
    pub harness: String,
    /// Version string the console wallet reported over gRPC.
    pub console_wallet: Option<String>,
    /// Git revision of `minotari-cli` linked into this harness.
    pub minotari_cli_rev: String,
    /// Every spawned binary, the version it reported and the pin it was checked against.
    ///
    /// This is what makes the profile reproducible: a reader can rebuild each binary at the
    /// recorded version and get the same software back. An entry with `pinned: false` is one
    /// the operator switched the assertion off for.
    #[serde(default)]
    pub binaries: Vec<crate::binaries::BinaryVersion>,
    pub base_node_url: String,
    pub base_node_synced: Option<bool>,
}

/// A derived comparison between two scenarios of one mode.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Delta {
    pub mode: String,
    pub name: String,
    pub value: f64,
    pub unit: String,
}

/// The committed artefact: one run, all modes, all scenarios.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ResultProfile {
    pub generated_at: DateTime<Utc>,
    pub network: String,
    pub environment: EnvironmentInfo,
    pub versions: Versions,
    pub config: HarnessConfig,
    pub reports: Vec<ScenarioReport>,
    pub deltas: Vec<Delta>,
}

impl ResultProfile {
    pub fn new(
        network: &str,
        environment: EnvironmentInfo,
        versions: Versions,
        config: HarnessConfig,
        reports: Vec<ScenarioReport>,
    ) -> Self {
        let deltas = compute_deltas(&reports);
        Self {
            generated_at: Utc::now(),
            network: network.to_string(),
            environment,
            versions,
            config,
            reports,
            deltas,
        }
    }

    /// A compact `mode x scenario` matrix for the console.
    pub fn summary_lines(&self) -> Vec<String> {
        self.reports
            .iter()
            .map(|r| {
                let outcome = match r.outcome {
                    Outcome::Ok => "ok",
                    Outcome::Failed => "FAILED",
                    Outcome::Skipped => "skipped",
                };
                format!(
                    "{:<12} {:<3} {:<8} {:>8.1}s  txs={:<4} fees={:<12} {}",
                    r.mode,
                    r.scenario,
                    outcome,
                    r.wall_clock_secs,
                    r.transactions.len(),
                    r.fees_paid,
                    r.error.as_deref().unwrap_or("")
                )
            })
            .collect()
    }

    /// Modes in the order they were run.
    fn modes(&self) -> Vec<&str> {
        let mut modes: Vec<&str> = Vec::new();
        for report in &self.reports {
            if !modes.contains(&report.mode.as_str()) {
                modes.push(&report.mode);
            }
        }
        modes
    }

    /// Scenarios in protocol order, as they were run.
    fn scenarios(&self) -> Vec<&str> {
        let mut scenarios: Vec<&str> = Vec::new();
        for report in &self.reports {
            if !scenarios.contains(&report.scenario.as_str()) {
                scenarios.push(&report.scenario);
            }
        }
        scenarios
    }

    fn report(&self, mode: &str, scenario: &str) -> Option<&ScenarioReport> {
        self.reports
            .iter()
            .find(|r| r.mode == mode && r.scenario == scenario)
    }

    /// The tables printed when a run finishes: time per scenario, what the scans cost, what
    /// the transaction scenarios cost, and the computed deltas.
    ///
    /// This is the same view the profile JSON carries, laid out for reading. Anything a
    /// reader would otherwise have to compute — totals, ratios between the modes, medians —
    /// is computed here so the console output can be quoted directly.
    pub fn summary_report(&self) -> String {
        let mut out = String::new();
        self.write_wall_clock_table(&mut out);
        self.write_scan_table(&mut out);
        self.write_transaction_table(&mut out);
        self.write_delta_table(&mut out);
        out
    }

    fn write_wall_clock_table(&self, out: &mut String) {
        let modes = self.modes();
        // A ratio only means something when there are exactly two things to compare.
        let ratio = (modes.len() == 2).then(|| format!("{}/{}", short(modes[0]), short(modes[1])));

        let _ = writeln!(out, "TIME PER SCENARIO");
        let _ = write!(out, "\n{:<10}", "");
        for mode in &modes {
            let _ = write!(out, "{mode:>14}");
        }
        if let Some(header) = &ratio {
            let _ = write!(out, "{header:>14}");
        }
        let _ = writeln!(out);

        let mut totals: Vec<f64> = vec![0.0; modes.len()];
        let mut any_failed = false;
        let mut any_skipped = false;
        for scenario in self.scenarios() {
            let _ = write!(out, "{scenario:<10}");
            for (index, mode) in modes.iter().enumerate() {
                let cell = match self.report(mode, scenario) {
                    Some(r) if r.outcome == Outcome::Skipped => {
                        any_skipped = true;
                        "-".to_string()
                    },
                    Some(r) => {
                        totals[index] += r.wall_clock_secs;
                        if r.outcome == Outcome::Failed {
                            any_failed = true;
                            format!("{:.1}s*", r.wall_clock_secs)
                        } else {
                            format!("{:.1}s", r.wall_clock_secs)
                        }
                    },
                    None => "-".to_string(),
                };
                let _ = write!(out, "{cell:>14}");
            }
            if ratio.is_some() {
                let _ = write!(out, "{:>14}", self.ratio_cell(&modes, scenario));
            }
            let _ = writeln!(out);
        }

        let _ = write!(out, "{:<10}", "total");
        for total in &totals {
            let _ = write!(out, "{:>14}", format!("{total:.1}s"));
        }
        if ratio.is_some() && totals[1] > 0.0 {
            let _ = write!(out, "{:>14}", format!("{:.2}x", totals[0] / totals[1]));
        }
        let _ = writeln!(out);
        let _ = write!(out, "{:<10}", "");
        for total in &totals {
            let _ = write!(out, "{:>14}", format!("({:.2} h)", total / 3600.0));
        }
        let _ = writeln!(out);

        if any_failed || any_skipped {
            let _ = writeln!(out, "\n  * failed (time still counted)   - not run");
        }
        let _ = writeln!(out);
    }

    /// `old/new` for one scenario, blank unless both modes produced a time.
    fn ratio_cell(&self, modes: &[&str], scenario: &str) -> String {
        let time = |mode: &str| {
            self.report(mode, scenario)
                .filter(|r| r.outcome != Outcome::Skipped)
                .map(|r| r.wall_clock_secs)
        };
        match (time(modes[0]), time(modes[1])) {
            (Some(first), Some(second)) if second > 0.0 => format!("{:.2}x", first / second),
            _ => String::new(),
        }
    }

    fn write_scan_table(&self, out: &mut String) {
        let scans: Vec<(&ScenarioReport, &ScanMetrics)> = self
            .reports
            .iter()
            .filter_map(|r| r.scan.as_ref().map(|s| (r, s)))
            .collect();
        if scans.is_empty() {
            return;
        }

        let _ = writeln!(out, "SCANS\n");
        let _ = writeln!(
            out,
            "{:<12}{:<4}{:>10}{:>12}{:>10}{:>9}{:>9}{:>10}{:>9}  measured on",
            "mode",
            "id",
            "T_scan",
            "validation",
            "blocks",
            "blk/s",
            "outputs",
            "peak RSS",
            "peak CPU"
        );
        let mut last_mode = "";
        let mut multi_process = false;
        for (report, scan) in scans {
            if !last_mode.is_empty() && last_mode != report.mode {
                let _ = writeln!(out);
            }
            last_mode = &report.mode;
            multi_process |= scan.processes.len() > 1;
            let _ = writeln!(
                out,
                "{:<12}{:<4}{:>9.1}s{:>11.1}s{:>10}{:>9.0}{:>9}{:>9.0}M{:>8.0}%  {}",
                report.mode,
                report.scenario,
                scan.duration_secs,
                scan.validation_secs,
                scan.blocks_scanned,
                scan.blocks_per_sec,
                scan.outputs_found,
                scan.peak_rss_bytes as f64 / 1e6,
                scan.peak_cpu_percent,
                describe_processes(scan),
            );
        }
        if multi_process {
            let _ = writeln!(
                out,
                "\n  peak RSS and peak CPU are the headline process only; the rest of the \
                 breakdown is in\n  the profile's `scan.processes`."
            );
        }
        let _ = writeln!(out);
    }

    fn write_transaction_table(&self, out: &mut String) {
        let sending: Vec<&ScenarioReport> = self
            .reports
            .iter()
            .filter(|r| !r.transactions.is_empty())
            .collect();
        if sending.is_empty() {
            return;
        }

        let _ = writeln!(out, "TRANSACTIONS\n");
        let _ = writeln!(
            out,
            "{:<12}{:<4}{:>10}{:>6}{:>6}{:>7}{:>7}{:>11}{:>13}{:>15}",
            "mode",
            "id",
            "wall",
            "txs",
            "ok",
            "failed",
            "infra",
            "fees µT",
            "median build",
            "median confirm"
        );
        let mut last_mode = "";
        for report in sending {
            if !last_mode.is_empty() && last_mode != report.mode {
                let _ = writeln!(out);
            }
            last_mode = &report.mode;
            let build = median(
                report
                    .transactions
                    .iter()
                    .filter(|t| t.outcome == TxOutcome::Confirmed)
                    .map(|t| t.construction_secs)
                    .collect(),
            );
            let confirm = median(
                report
                    .transactions
                    .iter()
                    .filter_map(|t| t.confirmed_at_secs)
                    .collect(),
            );
            // Derived from the errors rather than read from the notes, so a profile written
            // before that note existed still reports it correctly.
            let infrastructure = count_infrastructure_failures(&report.transactions);
            let _ = writeln!(
                out,
                "{:<12}{:<4}{:>9.1}s{:>6}{:>6}{:>7}{:>7}{:>11}{:>12.3}s{:>14.1}s",
                report.mode,
                report.scenario,
                report.wall_clock_secs,
                report.transactions.len(),
                report.successes,
                report.failures,
                infrastructure,
                report.fees_paid,
                build,
                confirm,
            );
        }
        let _ = writeln!(
            out,
            "\n  infra = failures from the harness's own plumbing (database pool, locks). Any \
             above zero\n  make that row's numbers suspect."
        );
        // A mode that acknowledges a send before it builds anything has no construction time
        // to report, and the column would otherwise read as one.
        if self.reports.iter().any(construction_unobservable) {
            let _ = writeln!(
                out,
                "  median build = time to accept the work, not to construct a transaction, for \
                 any mode that\n  acknowledges a send before it builds it (see \
                 `construction_is_observable` in the notes)."
            );
        }
        let _ = writeln!(out);
    }

    fn write_delta_table(&self, out: &mut String) {
        if self.deltas.is_empty() {
            return;
        }
        let _ = writeln!(out, "DELTAS\n");
        for delta in &self.deltas {
            let _ = writeln!(
                out,
                "{:<12}{:<50}{:>10.3} {}",
                delta.mode, delta.name, delta.value, delta.unit
            );
        }
        let _ = writeln!(out);
    }
}

/// Whether a report came from a mode that cannot see when construction finished.
fn construction_unobservable(report: &ScenarioReport) -> bool {
    report
        .notes
        .get("construction_is_observable")
        .and_then(Value::as_bool)
        .is_some_and(|observable| !observable)
}

/// The processes a scan's resource figures were attributed to, headline first.
fn describe_processes(scan: &ScanMetrics) -> String {
    if scan.processes.is_empty() {
        return if scan.headline_process.is_empty() {
            "-".to_string()
        } else {
            scan.headline_process.clone()
        };
    }
    scan.processes
        .iter()
        .map(|p| {
            if p.includes_harness {
                format!("{} (+harness)", p.process)
            } else {
                p.process.clone()
            }
        })
        .collect::<Vec<_>>()
        .join(", ")
}

/// `old_wallet` -> `old`, for a column header that has to fit.
fn short(mode: &str) -> &str {
    mode.split('_').next().unwrap_or(mode)
}

/// Median of the values, or zero when there are none.
fn median(mut values: Vec<f64>) -> f64 {
    if values.is_empty() {
        return 0.0;
    }
    values.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let middle = values.len() / 2;
    if values.len().is_multiple_of(2) {
        (values[middle - 1] + values[middle]) / 2.0
    } else {
        values[middle]
    }
}

/// Computes the deltas the spec calls out, for every mode present in `reports`.
pub fn compute_deltas(reports: &[ScenarioReport]) -> Vec<Delta> {
    let mut deltas = Vec::new();
    let mut modes: Vec<&str> = reports.iter().map(|r| r.mode.as_str()).collect();
    modes.dedup();

    for mode in modes {
        let scan_of = |id: &str| -> Option<f64> {
            reports
                .iter()
                .find(|r| r.mode == mode && r.scenario == id && r.outcome == Outcome::Ok)
                .and_then(ScenarioReport::scan_duration)
        };
        let push = |deltas: &mut Vec<Delta>, name: &str, value: f64, unit: &str| {
            deltas.push(Delta {
                mode: mode.to_string(),
                name: name.to_string(),
                value,
                unit: unit.to_string(),
            });
        };

        let b0 = scan_of("B0");
        let s2 = scan_of("S2");
        let s3 = scan_of("S3");
        let s6 = scan_of("S6");
        let s7 = scan_of("S7");

        if let (Some(b0), Some(s2)) = (b0, s2) {
            push(&mut deltas, "T_scan(S2) - T_scan(B0)", s2 - b0, "seconds");
        }
        if let (Some(s2), Some(s6)) = (s2, s6) {
            push(&mut deltas, "T_scan(S6) - T_scan(S2)", s6 - s2, "seconds");
        }
        if let (Some(b0), Some(s6)) = (b0, s6) {
            if b0 > 0.0 {
                push(&mut deltas, "T_scan(S6) / T_scan(B0)", s6 / b0, "ratio");
            }
        }
        // The birthday-scan pair of `T_scan(S6) - T_scan(S2)`: what S4 and S5 added to a scan
        // that starts at the wallet's birthday rather than at genesis. Separating the two is
        // the point of running both scans at each checkpoint — the genesis figure is dominated
        // by the blocks before the wallet existed, which no amount of wallet history changes.
        if let (Some(s3), Some(s7)) = (s3, s7) {
            push(&mut deltas, "T_scan(S7) - T_scan(S3)", s7 - s3, "seconds");
        }
        if let (Some(s7), Some(s6)) = (s7, s6) {
            if s7 > 0.0 {
                push(&mut deltas, "T_scan(S6) / T_scan(S7)", s6 / s7, "ratio");
            }
        }

        if let Some(s5) = reports
            .iter()
            .find(|r| r.mode == mode && r.scenario == "S5")
        {
            let batch = s5.notes.get("t_batch_secs").and_then(Value::as_f64);
            let individual = s5.notes.get("t_individual_secs").and_then(Value::as_f64);
            if let (Some(batch), Some(individual)) = (batch, individual) {
                if batch > 0.0 {
                    push(
                        &mut deltas,
                        "S5 throughput multiplier (T_individual / T_batch)",
                        individual / batch,
                        "ratio",
                    );
                }
            }
        }
    }

    deltas
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scan_report(mode: &str, scenario: &str, secs: f64) -> ScenarioReport {
        let mut report = ScenarioReport::new(scenario, mode);
        report.scan = Some(ScanMetrics {
            duration_secs: secs,
            blocks_scanned: 1_000,
            ..ScanMetrics::default()
        });
        report
    }

    #[test]
    fn blocks_per_sec_is_derived() {
        let metrics = ScanMetrics {
            duration_secs: 4.0,
            blocks_scanned: 2_000,
            ..ScanMetrics::default()
        }
        .finish();
        assert!((metrics.blocks_per_sec - 500.0).abs() < f64::EPSILON);
    }

    #[test]
    fn a_zero_length_scan_does_not_divide_by_zero() {
        let metrics = ScanMetrics {
            duration_secs: 0.0,
            blocks_scanned: 10,
            ..ScanMetrics::default()
        }
        .finish();
        assert_eq!(metrics.blocks_per_sec, 0.0);
    }

    #[test]
    fn reconciliation_reports_the_shortfall() {
        let mut report = ScenarioReport::new("S1", "new_wallet");
        report.balance_end = 900;
        report.reconcile(1_000);
        assert_eq!(report.balance_delta, Some(100));
        report.balance_end = 1_100;
        report.reconcile(1_000);
        assert_eq!(report.balance_delta, Some(-100));
    }

    #[test]
    fn failing_a_scenario_keeps_the_first_reason_and_all_of_them() {
        let mut report = ScenarioReport::new("S1", "old_wallet");
        report.fail("round 2 came up short");
        report.fail("round 3 never confirmed");
        assert_eq!(report.outcome, Outcome::Failed);
        assert_eq!(report.failures, 2);
        assert_eq!(report.error.as_deref(), Some("round 2 came up short"));
        assert_eq!(report.failure_reasons.len(), 2);
    }

    #[test]
    fn scan_deltas_are_computed_per_mode() {
        let reports = vec![
            scan_report("old_wallet", "B0", 10.0),
            scan_report("old_wallet", "S2", 25.0),
            scan_report("old_wallet", "S6", 40.0),
        ];
        let deltas = compute_deltas(&reports);
        let by_name = |name: &str| {
            deltas
                .iter()
                .find(|d| d.name == name)
                .map(|d| d.value)
                .unwrap_or_else(|| panic!("missing delta {name}"))
        };
        assert!((by_name("T_scan(S2) - T_scan(B0)") - 15.0).abs() < f64::EPSILON);
        assert!((by_name("T_scan(S6) - T_scan(S2)") - 15.0).abs() < f64::EPSILON);
        assert!((by_name("T_scan(S6) / T_scan(B0)") - 4.0).abs() < f64::EPSILON);
    }

    #[test]
    fn the_second_checkpoint_reports_its_birthday_scan_delta_too() {
        let reports = vec![
            scan_report("payment_processor", "S3", 20.0),
            scan_report("payment_processor", "S6", 60.0),
            scan_report("payment_processor", "S7", 30.0),
        ];
        let deltas = compute_deltas(&reports);
        let by_name = |name: &str| {
            deltas
                .iter()
                .find(|d| d.name == name)
                .map(|d| d.value)
                .unwrap_or_else(|| panic!("missing delta {name}"))
        };
        assert!((by_name("T_scan(S7) - T_scan(S3)") - 10.0).abs() < f64::EPSILON);
        assert!((by_name("T_scan(S6) / T_scan(S7)") - 2.0).abs() < f64::EPSILON);
    }

    #[test]
    fn a_checkpoint_missing_its_birthday_scan_publishes_no_birthday_delta() {
        let deltas = compute_deltas(&[
            scan_report("old_wallet", "S3", 20.0),
            scan_report("old_wallet", "S6", 60.0),
        ]);
        assert!(!deltas.iter().any(|d| d.name.contains("S7")));
    }

    #[test]
    fn failed_scans_are_left_out_of_the_deltas() {
        let mut s2 = scan_report("new_wallet", "S2", 25.0);
        s2.outcome = Outcome::Failed;
        let deltas = compute_deltas(&[scan_report("new_wallet", "B0", 10.0), s2]);
        assert!(deltas.is_empty());
    }

    #[test]
    fn s5_multiplier_comes_from_the_two_arms() {
        let mut s5 = ScenarioReport::new("S5", "new_wallet");
        s5.note("t_batch_secs", 8.0);
        s5.note("t_individual_secs", 24.0);
        let deltas = compute_deltas(&[s5]);
        assert_eq!(deltas.len(), 1);
        assert!((deltas[0].value - 3.0).abs() < f64::EPSILON);
        assert_eq!(deltas[0].unit, "ratio");
    }

    #[test]
    fn no_multiplier_is_published_when_an_arm_did_not_finish() {
        // S5 withholds `t_batch_secs` when the batch arm failed, so there is nothing to
        // divide by and no headline gets invented from a duration that is really a
        // time-to-give-up.
        let mut s5 = ScenarioReport::new("S5", "old_wallet");
        s5.note("t_individual_secs", 360.0);
        s5.note("comparison_withheld", "batch confirmed 0 of 10");
        assert!(compute_deltas(&[s5]).is_empty());
    }

    #[test]
    fn fees_discovered_after_the_scenario_move_the_expectation_rather_than_the_balance() {
        // What a payment processor scenario looks like when it returns: the transactions
        // confirmed, but not one fee is known, so the scenario reconciled against a spend of
        // zero fees and the closing balance looks short by exactly the fees.
        let mut report = ScenarioReport::new("S1", "payment_processor");
        report.balance_start = 10_000;
        report.balance_end = 9_700;
        report.reconcile(10_000);
        assert_eq!(
            report.balance_delta,
            Some(300),
            "the fees look like a shortfall"
        );

        report.transactions.push(TxMetric {
            fee: 200,
            ..TxMetric::default()
        });
        report.transactions.push(TxMetric {
            fee: 100,
            ..TxMetric::default()
        });
        report.account_for_late_fees(0);

        assert_eq!(report.fees_paid, 300);
        assert_eq!(report.expected_balance_end, Some(9_700));
        assert_eq!(report.balance_delta, Some(0), "the shortfall was the fees");
    }

    #[test]
    fn a_mode_that_already_knew_its_fees_is_left_alone() {
        let mut report = ScenarioReport::new("S1", "new_wallet");
        report.transactions.push(TxMetric {
            fee: 300,
            ..TxMetric::default()
        });
        report.fees_paid = 300;
        report.balance_end = 9_700;
        report.reconcile(9_700);
        report.account_for_late_fees(300);
        assert_eq!(report.expected_balance_end, Some(9_700));
        assert_eq!(report.balance_delta, Some(0));
    }

    #[test]
    fn late_fees_on_a_scenario_that_never_reconciled_only_update_the_total() {
        let mut report = ScenarioReport::new("B0", "payment_processor");
        report.transactions.push(TxMetric {
            fee: 42,
            ..TxMetric::default()
        });
        report.account_for_late_fees(0);
        assert_eq!(report.fees_paid, 42);
        assert_eq!(report.expected_balance_end, None);
        assert_eq!(report.balance_delta, None);
    }

    #[test]
    fn a_mode_that_cannot_time_construction_is_flagged_in_the_transaction_table() {
        let mut report = ScenarioReport::new("S4", "payment_processor");
        report.transactions.push(TxMetric::default());
        assert!(
            !construction_unobservable(&report),
            "no note means the usual case"
        );
        report.note("construction_is_observable", false);
        assert!(construction_unobservable(&report));
        report.note("construction_is_observable", true);
        assert!(!construction_unobservable(&report));
    }

    #[test]
    fn the_headline_resource_figures_come_from_the_first_process() {
        let scan = ScanMetrics::default().with_processes(vec![
            ProcessSample {
                process: "daemon".to_string(),
                peak_rss_bytes: 900,
                peak_cpu_percent: 80.0,
                includes_harness: false,
            },
            ProcessSample {
                process: "payment_processor".to_string(),
                peak_rss_bytes: 100,
                peak_cpu_percent: 5.0,
                includes_harness: false,
            },
        ]);
        assert_eq!(scan.headline_process, "daemon");
        assert_eq!(scan.peak_rss_bytes, 900);
        assert!((scan.peak_cpu_percent - 80.0).abs() < f32::EPSILON);
        assert_eq!(scan.processes.len(), 2);
        assert_eq!(
            describe_processes(&scan),
            "daemon, payment_processor",
            "a three-process mode has to say which process the headline describes"
        );
    }

    #[test]
    fn a_harness_inclusive_sample_is_labelled_as_one() {
        let scan = ScanMetrics::default().with_processes(vec![ProcessSample {
            process: "harness".to_string(),
            peak_rss_bytes: 5,
            peak_cpu_percent: 1.0,
            includes_harness: true,
        }]);
        assert_eq!(describe_processes(&scan), "harness (+harness)");
    }

    #[test]
    fn an_unobservable_broadcast_is_null_with_a_reason_rather_than_zero() {
        let metric = TxMetric {
            broadcast_secs: None,
            broadcast_unavailable: Some("the payment processor emits no event".to_string()),
            ..TxMetric::default()
        };
        let json = serde_json::to_value(&metric).unwrap();
        assert!(json["broadcast_secs"].is_null());
        assert_eq!(
            json["broadcast_unavailable"],
            "the payment processor emits no event"
        );

        // A mode that can time its broadcast still reports a number.
        let timed = TxMetric {
            broadcast_secs: Some(0.25),
            ..TxMetric::default()
        };
        let json = serde_json::to_value(&timed).unwrap();
        assert!((json["broadcast_secs"].as_f64().unwrap() - 0.25).abs() < f64::EPSILON);
        assert!(json.get("broadcast_unavailable").is_none());
    }

    #[test]
    fn report_serialisation_round_trips() {
        let mut report = ScenarioReport::new("S4", "old_wallet");
        report.transactions.push(TxMetric {
            label: "s4-8-0".to_string(),
            fee: 17,
            outcome: TxOutcome::Confirmed,
            ..TxMetric::default()
        });
        report.note("levels", vec![8, 16]);
        let json = serde_json::to_string(&report).unwrap();
        let parsed: ScenarioReport = serde_json::from_str(&json).unwrap();
        assert_eq!(report, parsed);
        assert_eq!(parsed.total_recorded_fees(), 17);
    }
}
