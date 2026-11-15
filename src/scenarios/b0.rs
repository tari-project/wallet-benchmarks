use crate::modes::WalletMode;
use crate::metrics::ScenarioReport;
use std::error::Error;

pub async fn run_b0(mode: &mut dyn WalletMode) -> Result<ScenarioReport, Box<dyn Error>> {
    // B0 — Baseline Scan (empty wallet)
    // Preconditions: fresh seed, never funded. Birthday = 0.
    let report = mode.reset_and_scan(0).await?;
    
    let balance = mode.get_balance().await?;
    if balance != 0 {
        return Err("B0 failed: Balance should be 0".into());
    }

    Ok(report)
}
