use async_trait::async_trait;
use crate::metrics::ScenarioReport;
use std::error::Error;

pub mod old_wallet;
pub mod new_wallet;
pub mod payment_processor;

#[async_trait]
pub trait WalletMode {
    async fn reset_and_scan(&mut self, birthday: u64) -> Result<ScenarioReport, Box<dyn Error>>;
    async fn get_balance(&self) -> Result<u64, Box<dyn Error>>;
    async fn send_transaction(&mut self, outputs: Vec<(String, u64)>) -> Result<String, Box<dyn Error>>;
    async fn wait_for_confirmation(&self, tx_id: String, depth: u32) -> Result<(), Box<dyn Error>>;
    fn name(&self) -> String;
}
