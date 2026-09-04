//! The benchmark protocol: one type per scenario, all behind the [`Scenario`] trait.
//!
//! Scenarios are ordered and share state through [`RunContext`]: S0 records `H_birth`, S1
//! leaves the wallet holding `volume_target` UTXOs, S5 fixes the recipient set. Running them
//! out of order is allowed but the later ones then report what they could not verify rather
//! than inventing a baseline.

mod b0;
mod s0;
mod s1;
mod s4;
mod s5;
mod scan;
pub mod support;

use std::sync::Arc;

use anyhow::Result;
use async_trait::async_trait;

use crate::{
    chain::ChainClient,
    config::HarnessConfig,
    metrics::ScenarioReport,
    modes::WalletMode,
    seeds::{baseline_label, SeedBook, SeedEntry},
};

/// Scenario ids in protocol order.
pub const ALL: [&str; 9] = ["B0", "S0", "S1", "S2", "S3", "S4", "S5", "S6", "S7"];

/// One benchmark scenario, run against one wallet mode.
#[async_trait]
pub trait Scenario: Send + Sync {
    /// Protocol id, e.g. `S1`.
    fn id(&self) -> &'static str;

    /// One-line description, copied into the run log.
    fn description(&self) -> &'static str;

    /// Runs the scenario. Returning `Err` means the harness could not take a measurement at
    /// all; a wallet that misbehaved is reported inside the [`ScenarioReport`] instead.
    async fn run(
        &mut self,
        mode: &Arc<dyn WalletMode>,
        ctx: &mut RunContext,
    ) -> Result<ScenarioReport>;

    /// Recomputes the figures that could not be known while the scenario was running.
    ///
    /// Called once, after [`WalletMode::enrich`] and before the run decides whether the
    /// scenario passed, so a verdict reached here still stops the rest of the mode.
    ///
    /// Only fees need this so far, and only for `payment_processor`: it reports none at send
    /// time, and every fee-derived number — S1's value conservation, S4's per-level cost, S5's
    /// whole reason for existing — would otherwise be computed against zero. A mode that
    /// answers [`WalletMode::reports_fee_at_send`] leaves this doing nothing.
    fn finalise(&mut self, _report: &mut ScenarioReport) {}
}

/// Builds a scenario by id.
pub fn scenario(id: &str) -> Option<Box<dyn Scenario>> {
    match id.to_ascii_uppercase().as_str() {
        "B0" => Some(Box::new(b0::BaselineScan)),
        "S0" => Some(Box::new(s0::FundingBaseline)),
        "S1" => Some(Box::new(s1::UtxoBuildUp::default())),
        "S2" => Some(Box::new(scan::ScanScenario::s2())),
        "S3" => Some(Box::new(scan::ScanScenario::s3())),
        "S4" => Some(Box::new(s4::ConcurrentConstruction::default())),
        "S5" => Some(Box::new(s5::PaymentThroughput::default())),
        "S6" => Some(Box::new(scan::ScanScenario::s6())),
        "S7" => Some(Box::new(scan::ScanScenario::s7())),
        _ => None,
    }
}

/// State carried across the scenarios of one mode.
#[derive(Debug, Default, Clone)]
pub struct ModeState {
    /// `H_birth`, resolved in S0 and reused by S3.
    pub birth_height: Option<u64>,
    /// The recipient set S5 pays; both arms must pay the same addresses.
    pub recipients: Vec<String>,
}

/// Everything a scenario needs besides the wallet itself.
pub struct RunContext {
    pub cfg: Arc<HarnessConfig>,
    pub chain: ChainClient,
    pub seeds: SeedBook,
    /// Mode label, which is also the label of the funded seed.
    pub mode_label: String,
    pub state: ModeState,
}

impl RunContext {
    pub fn new(
        cfg: Arc<HarnessConfig>,
        chain: ChainClient,
        seeds: SeedBook,
        mode_label: &str,
    ) -> Self {
        Self {
            cfg,
            chain,
            seeds,
            mode_label: mode_label.to_string(),
            state: ModeState::default(),
        }
    }

    /// The funded seed S0–S6 run against.
    pub fn funded_seed(&self) -> Result<SeedEntry> {
        Ok(self.seeds.require(&self.mode_label)?.clone())
    }

    /// The never-funded seed B0 runs against.
    pub fn baseline_seed(&self) -> Result<SeedEntry> {
        Ok(self
            .seeds
            .require(&baseline_label(&self.mode_label))?
            .clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_protocol_is_the_full_nine_scenarios_in_order() {
        assert_eq!(ALL, ["B0", "S0", "S1", "S2", "S3", "S4", "S5", "S6", "S7"]);
    }

    #[test]
    fn every_protocol_id_resolves_to_a_scenario() {
        for id in ALL {
            let scenario = scenario(id).unwrap_or_else(|| panic!("no scenario for {id}"));
            assert_eq!(scenario.id(), id);
            assert!(!scenario.description().is_empty());
        }
    }

    #[test]
    fn ids_are_case_insensitive_and_unknown_ids_are_rejected() {
        assert!(scenario("s1").is_some());
        assert!(scenario("s7").is_some(), "S7 is part of the protocol");
        assert!(scenario("nonsense").is_none());
        assert!(scenario("S8").is_none());
    }
}
