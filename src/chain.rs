//! Base node access shared by both wallet modes.
//!
//! Only chain *heights* come from here — the tip at the start and end of a scenario, and the
//! height a seed birthday resolves to — so both modes are described against the same ruler.
//!
//! Transaction confirmation deliberately does not: whether a transaction has reached `C_min`
//! is asked of the wallet, because the wallet is what decides when the money can be spent
//! again, and the gap between the two views is one of the things the harness measures.

use anyhow::{anyhow, Result};
use minotari::http::WalletHttpClient;
use tari_transaction_components::rpc::models::TxQueryResponse;
use url::Url;

/// Thin wrapper over the `minotari` base node HTTP client.
#[derive(Clone)]
pub struct ChainClient {
    client: std::sync::Arc<WalletHttpClient>,
    url: String,
}

impl ChainClient {
    pub fn new(base_node_url: &str) -> Result<Self> {
        let url = Url::parse(base_node_url)
            .map_err(|e| anyhow!("Invalid base_node_url `{base_node_url}`: {e}"))?;
        Ok(Self {
            client: std::sync::Arc::new(WalletHttpClient::new(url)?),
            url: base_node_url.to_string(),
        })
    }

    pub fn url(&self) -> &str {
        &self.url
    }

    /// Best block height the base node knows about.
    pub async fn tip_height(&self) -> Result<u64> {
        let tip = self.client.get_tip_info().await?;
        tip.metadata
            .map(|m| m.best_block_height())
            .ok_or_else(|| anyhow!("Base node returned no chain metadata"))
    }

    /// Whether the base node considers itself synced. Recorded for disclosure.
    pub async fn is_synced(&self) -> Result<bool> {
        Ok(self.client.get_tip_info().await?.is_synced)
    }

    /// Block height at a point in time; used to turn a seed birthday into `H_birth`.
    pub async fn height_at_time(&self, epoch_secs: u64) -> Result<u64> {
        self.client.get_height_at_time(epoch_secs).await
    }

    /// Where the base node believes a transaction is, by its kernel's excess signature.
    ///
    /// The one place the harness asks the *chain* about a transaction rather than the wallet.
    /// It is used only for post-run enrichment of the `payment_processor` mode, which cannot
    /// answer for its own transactions: the processor reports no transaction id and the
    /// daemon's API exposes no kernel, so this is the only authoritative confirmation
    /// available for that mode. Nothing waits on it and no scenario's verdict depends on it —
    /// confirmation is still the wallet's call, as it is for the other two modes.
    ///
    /// Note this is keyed on the excess *signature*, not the excess. The daemon reports the
    /// excess and nothing else, so the signature has to come from the transaction the daemon
    /// stored.
    pub async fn transaction_location(
        &self,
        excess_sig_nonce: &[u8],
        excess_sig: &[u8],
    ) -> Result<TxQueryResponse> {
        self.client
            .transaction_query(excess_sig_nonce, excess_sig)
            .await
    }
}
