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
pub const MINOTARI_CLI_REV: &str = "84c79e9c0f322746ad089e6f09241dced3198aa2";

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
    pub tx_id: Option<u64>,
    /// Payment outputs requested (change excluded).
    pub outputs: u32,
    /// Total amount sent, in µT.
    pub amount: u64,
    /// Fee paid, in µT. Zero when the transaction never made it onto the chain.
    pub fee: u64,
    /// Time spent building and signing.
    pub construction_secs: f64,
    /// Time from the end of construction to the node accepting the transaction.
    pub broadcast_secs: f64,
    /// Offset from the batch start at which the send was submitted.
    pub submitted_at_secs: f64,
    /// Offset from the batch start at which construction completed. S4 uses the gaps between
    /// consecutive values of this field to expose internal serialisation.
    pub construction_done_secs: f64,
    /// Offset from the batch start at which the transaction reached `C_min` confirmations.
    pub confirmed_at_secs: Option<f64>,
    pub outcome: TxOutcome,
    pub error: Option<String>,
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
    pub peak_rss_bytes: u64,
    pub peak_cpu_percent: f32,
    /// True when the sampled process also runs the harness (the library wallet).
    pub resource_sample_includes_harness: bool,
}

impl ScanMetrics {
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
            "{:<12}{:<4}{:>10}{:>12}{:>10}{:>9}{:>9}{:>10}{:>9}",
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
        for (report, scan) in scans {
            if !last_mode.is_empty() && last_mode != report.mode {
                let _ = writeln!(out);
            }
            last_mode = &report.mode;
            let _ = writeln!(
                out,
                "{:<12}{:<4}{:>9.1}s{:>11.1}s{:>10}{:>9.0}{:>9}{:>9.0}M{:>8.0}%",
                report.mode,
                report.scenario,
                scan.duration_secs,
                scan.validation_secs,
                scan.blocks_scanned,
                scan.blocks_per_sec,
                scan.outputs_found,
                scan.peak_rss_bytes as f64 / 1e6,
                scan.peak_cpu_percent,
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
        let s6 = scan_of("S6");

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
