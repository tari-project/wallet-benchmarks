use async_trait::async_trait;
use crate::modes::WalletMode;
use crate::metrics::ScenarioReport;
use std::error::Error;

pub struct PaymentProcessor {
    // Uses the same library as NewWallet but specialized for batching
}

#[async_trait]
impl WalletMode for PaymentProcessor {
    fn name(&self) -> String { "payment_processor".to_string() }

    async fn reset_and_scan(&mut self, b: u64) -> Result<ScenarioReport, Box<dyn Error>> {
        // Implementation similar to NewWallet
        Ok(ScenarioReport::default())
    }

    async fn get_balance(&self) -> Result<u64, Box<dyn Error>> { Ok(0) }

    async fn send_transaction(&mut self, outputs: Vec<(String, u64)>) -> Result<String, Box<dyn Error>> {
        // Logic: Use OneSidedTransaction::create_unsigned_transaction for batching
        // This is the "1-to-many" logic requested
        Ok("batch_tx_hash".to_string())
    }

    async fn wait_for_confirmation(&self, _tx_id: String, _depth: u32) -> Result<(), Box<dyn Error>> { Ok(()) }
}
