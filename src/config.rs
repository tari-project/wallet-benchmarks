use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HarnessConfig {
    pub a_fund: u64,
    pub c_min: u32,
    pub volume_target: u32,
    pub doubling_rounds: u32,
    pub fanout_outputs_per_tx: u32,
    pub concurrent_batches: Vec<u32>,
    pub s4_t_budget_secs: u64,
    pub s5_m: u32,
    pub s5_k: u32,
    pub fee_rate: u64,
    pub base_node_grpc_url: String,
    pub wallet_binary_path: String,
}

impl Default for HarnessConfig {
    fn default() -> Self {
        Self {
            a_fund: 10_000_000_000, // 10,000 tXTM
            c_min: 3,
            volume_target: 512,
            doubling_rounds: 6,
            fanout_outputs_per_tx: 8,
            concurrent_batches: vec![8, 16, 32, 64, 128],
            s4_t_budget_secs: 900, // 15 min
            s5_m: 100,
            s5_k: 10,
            fee_rate: 5,
            base_node_grpc_url: "http://localhost:18142".to_string(),
            wallet_binary_path: "./minotari_console_wallet".to_string(),
        }
    }
}
