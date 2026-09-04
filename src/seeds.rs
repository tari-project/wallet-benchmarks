//! Seed word management.
//!
//! S0–S6 all run against one specific, funded seed per mode, and B0 runs against a seed that
//! must never be funded. Those seeds are generated once, written to a git-ignored seed file
//! and reused by every later run, so a wallet only has to be funded once. `wallet-benchmarks
//! prepare` creates the file and prints the addresses to fund before a run starts.
//!
//! The seed file is also how scan scenarios get a genesis birthday. A wallet derives its
//! scan-start height from the birthday inside the `CipherSeed` it was given and from nothing
//! else, so [`SeedEntry::cipher_seed_for`] zeroes that birthday for a genesis scan and the
//! wallet is started from the result — as seed words for the console wallet, as a
//! `CipherSeed` for the library wallet. The entropy is untouched, so it stays the same
//! wallet.

use std::{path::Path, str::FromStr};

use anyhow::{anyhow, Context, Result};
use serde::{Deserialize, Serialize};
use tari_common::configuration::Network;
use tari_common_types::{
    seeds::{
        cipher_seed::{CipherSeed, BIRTHDAY_GENESIS_FROM_UNIX_EPOCH, SECONDS_PER_DAY},
        mnemonic::{Mnemonic, MnemonicLanguage},
        seed_words::SeedWords,
    },
    tari_address::{TariAddress, TariAddressFeatures},
};
use tari_crypto::compressed_key::CompressedKey;
use tari_transaction_components::key_manager::{
    wallet_types::{SeedWordsWallet, WalletType},
    KeyManager,
    TransactionKeyManagerInterface,
};

/// Suffix of the never-funded wallet each mode uses for the B0 baseline scan.
pub const BASELINE_SUFFIX: &str = "-baseline";

/// One wallet's seed, as stored in the seed file.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SeedEntry {
    /// Unique label, e.g. `old_wallet` or `old_wallet-baseline`.
    pub label: String,
    /// English mnemonic. Secret: the seed file must stay out of version control.
    pub seed_words: String,
    /// Birthday encoded in the seed, in days since 2022-01-01.
    pub birthday: u16,
    /// One-sided address derived from the seed; this is what the operator funds.
    pub address: String,
    /// Whether this wallet has to be funded before a run (false for the B0 baseline seed).
    pub needs_funding: bool,
}

impl SeedEntry {
    /// Unix timestamp of the encoded birthday, used to resolve `H_birth` against the chain.
    pub fn birthday_epoch_secs(&self) -> u64 {
        self.scan_start_epoch_secs(0)
    }

    /// Unix timestamp a wallet actually begins scanning from.
    ///
    /// Both wallets step a safety offset back from the encoded birthday before resolving it
    /// to a height, so this is not the same as [`SeedEntry::birthday_epoch_secs`] and using
    /// the latter would under-report `blocks_scanned` by the offset.
    pub fn scan_start_epoch_secs(&self, offset_days: u64) -> u64 {
        BIRTHDAY_GENESIS_FROM_UNIX_EPOCH +
            u64::from(self.birthday).saturating_sub(offset_days) * SECONDS_PER_DAY
    }

    /// The `CipherSeed` this wallet should be started from for the given birthday policy.
    ///
    /// This is where a genesis scan is actually requested. Neither wallet takes a scan-start
    /// height from its caller: each one reads the birthday out of the `CipherSeed` it was
    /// given and derives the start height from that. The console wallet copies
    /// `seed.birthday()` into its `WalletBirthday` setting when it stores the master seed,
    /// and `minotari-cli` copies `wallet.get_birthday()` into the account's `birthday`
    /// column when the account is created. So scanning from genesis means handing the wallet
    /// a seed whose birthday *is* 0 — [`CipherSeed::change_birthday`] — and nothing else.
    ///
    /// Only the birthday field changes. The entropy is untouched, so this is the same
    /// wallet, with the same keys and the same address, either way.
    pub fn cipher_seed_for(&self, birthday: crate::modes::Birthday) -> Result<CipherSeed> {
        let mut seed = cipher_seed_from_words(&self.seed_words)?;
        if birthday == crate::modes::Birthday::Genesis {
            seed.change_birthday(0);
        }
        Ok(seed)
    }

    /// The same seed as [`SeedEntry::cipher_seed_for`], encoded as a mnemonic for a wallet
    /// that can only be handed seed words (the console wallet's `--seed-words`).
    pub fn words_for(&self, birthday: crate::modes::Birthday) -> Result<String> {
        seed_to_words(&self.cipher_seed_for(birthday)?)
    }
}

/// The set of seeds a run uses, persisted as JSON.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SeedBook {
    /// Network the addresses were derived for. Addresses are network specific.
    pub network: String,
    /// One entry per wallet.
    pub entries: Vec<SeedEntry>,
}

impl SeedBook {
    pub fn new(network: &str) -> Self {
        Self {
            network: network.to_string(),
            entries: Vec::new(),
        }
    }

    /// Loads the seed file, or returns an empty book if it does not exist yet.
    pub fn load_or_new(path: &Path, network: &str) -> Result<Self> {
        if !path.exists() {
            return Ok(Self::new(network));
        }
        let text = std::fs::read_to_string(path)
            .with_context(|| format!("Could not read seed file {}", path.display()))?;
        let book: Self = serde_json::from_str(&text)
            .with_context(|| format!("Could not parse seed file {}", path.display()))?;
        if book.network != network {
            return Err(anyhow!(
                "Seed file {} was created for network `{}` but the harness is configured for \
                 `{}`. Point `seed_file` at a different path or delete it.",
                path.display(),
                book.network,
                network
            ));
        }
        Ok(book)
    }

    /// Loads the seed file, failing if it has not been created yet.
    pub fn load(path: &Path, network: &str) -> Result<Self> {
        if !path.exists() {
            return Err(anyhow!(
                "Seed file {} does not exist. Run `wallet-benchmarks prepare` first.",
                path.display()
            ));
        }
        Self::load_or_new(path, network)
    }

    pub fn save(&self, path: &Path) -> Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("Could not create {}", parent.display()))?;
        }
        let text = serde_json::to_string_pretty(self)?;
        std::fs::write(path, text)
            .with_context(|| format!("Could not write seed file {}", path.display()))?;
        crate::secrets::restrict_file(path)?;
        Ok(())
    }

    pub fn get(&self, label: &str) -> Option<&SeedEntry> {
        self.entries.iter().find(|e| e.label == label)
    }

    pub fn require(&self, label: &str) -> Result<&SeedEntry> {
        self.get(label).ok_or_else(|| {
            anyhow!("No seed for `{label}` in the seed file. Run `wallet-benchmarks prepare`.")
        })
    }

    /// Adds a wallet if it is not present yet. Existing entries are never regenerated: the
    /// funded seed must survive across runs.
    pub fn ensure(
        &mut self,
        label: &str,
        network: Network,
        needs_funding: bool,
    ) -> Result<&SeedEntry> {
        if self.get(label).is_none() {
            let seed_words = generate_seed_words()?;
            let entry = describe_seed(label, &seed_words, network, needs_funding)?;
            self.entries.push(entry);
        }
        Ok(self
            .get(label)
            .expect("entry was just inserted when it was missing"))
    }

    /// Every wallet the operator has to fund before a run.
    pub fn funding_targets(&self) -> Vec<&SeedEntry> {
        self.entries.iter().filter(|e| e.needs_funding).collect()
    }
}

/// The B0 wallet label for a mode.
pub fn baseline_label(mode: &str) -> String {
    format!("{mode}{BASELINE_SUFFIX}")
}

/// Generates a fresh mnemonic with today's birthday.
pub fn generate_seed_words() -> Result<String> {
    let seed = CipherSeed::random();
    seed_to_words(&seed)
}

/// Builds the seed file entry for a mnemonic, deriving the address the operator funds.
pub fn describe_seed(
    label: &str,
    seed_words: &str,
    network: Network,
    needs_funding: bool,
) -> Result<SeedEntry> {
    let seed = cipher_seed_from_words(seed_words)?;
    let address = derive_one_sided_address(seed_words, network)?;
    Ok(SeedEntry {
        label: label.to_string(),
        seed_words: seed_words.to_string(),
        birthday: seed.birthday(),
        address: address.to_base58(),
        needs_funding,
    })
}

/// Parses a mnemonic into a [`CipherSeed`].
pub fn cipher_seed_from_words(words: &str) -> Result<CipherSeed> {
    let mnemonic = SeedWords::from_str(words).map_err(|e| anyhow!("Invalid seed words: {e}"))?;
    CipherSeed::from_mnemonic(&mnemonic, None).map_err(|e| anyhow!("Invalid seed words: {e}"))
}

fn seed_to_words(seed: &CipherSeed) -> Result<String> {
    let words = seed
        .to_mnemonic(MnemonicLanguage::English, None)
        .map_err(|e| anyhow!("Could not encode seed as mnemonic: {e}"))?;
    Ok(words.join(" ").reveal().to_string())
}

/// Re-encodes a mnemonic with a different birthday, keeping the entropy (and therefore the
/// keys and the address) unchanged. Passing 0 is what makes a wallet scan from genesis.
pub fn words_with_birthday(words: &str, birthday: u16) -> Result<String> {
    let mut seed = cipher_seed_from_words(words)?;
    seed.change_birthday(birthday);
    seed_to_words(&seed)
}

/// Derives the one-sided address a mnemonic receives payments on.
pub fn derive_one_sided_address(words: &str, network: Network) -> Result<TariAddress> {
    let seed = cipher_seed_from_words(words)?;
    let wallet = SeedWordsWallet::construct_new(seed).map_err(|e| anyhow!("Invalid seed: {e}"))?;
    let key_manager = KeyManager::new(WalletType::SeedWords(wallet))
        .map_err(|e| anyhow!("Could not build key manager: {e}"))?;
    let view_key = key_manager.get_private_view_key();
    let spend_key = key_manager.get_spend_key();
    let public_view_key = CompressedKey::from_secret_key(&view_key);
    TariAddress::new_dual_address(
        public_view_key,
        spend_key.pub_key.clone(),
        network,
        TariAddressFeatures::create_one_sided_only(),
        None,
    )
    .map_err(|e| anyhow!("Could not derive address: {e}"))
}

/// Generates `count` throwaway addresses to pay in S5. Only the addresses matter; nobody
/// needs to spend from them.
pub fn generate_recipient_addresses(count: usize, network: Network) -> Result<Vec<String>> {
    (0..count)
        .map(|_| {
            let words = generate_seed_words()?;
            Ok(derive_one_sided_address(&words, network)?.to_base58())
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::modes::Birthday;

    const NETWORK: Network = Network::Esmeralda;

    #[test]
    fn generated_seeds_round_trip() {
        let words = generate_seed_words().unwrap();
        assert_eq!(words.split_whitespace().count(), 24);
        let seed = cipher_seed_from_words(&words).unwrap();
        assert!(seed.birthday() > 0, "a fresh seed carries today's birthday");
    }

    #[test]
    fn zeroing_the_birthday_keeps_the_address() {
        let words = generate_seed_words().unwrap();
        let genesis_words = words_with_birthday(&words, 0).unwrap();
        assert_ne!(words, genesis_words, "the encoded birthday changed");
        assert_eq!(
            cipher_seed_from_words(&genesis_words).unwrap().birthday(),
            0
        );
        assert_eq!(
            derive_one_sided_address(&words, NETWORK)
                .unwrap()
                .to_base58(),
            derive_one_sided_address(&genesis_words, NETWORK)
                .unwrap()
                .to_base58(),
            "the same wallet must be recovered by the re-encoded seed"
        );
    }

    #[test]
    fn address_derivation_is_deterministic_and_network_bound() {
        let words = generate_seed_words().unwrap();
        let a = derive_one_sided_address(&words, NETWORK).unwrap();
        let b = derive_one_sided_address(&words, NETWORK).unwrap();
        assert_eq!(a.to_base58(), b.to_base58());
        assert_eq!(a.network(), NETWORK);

        let main = derive_one_sided_address(&words, Network::MainNet).unwrap();
        assert_ne!(a.to_base58(), main.to_base58());
    }

    #[test]
    fn a_genesis_scan_is_requested_by_zeroing_the_cipher_seed_birthday() {
        let words = generate_seed_words().unwrap();
        let entry = describe_seed("old_wallet", &words, NETWORK, true).unwrap();

        // The seed handed to the wallet is what decides where the scan starts, so this is
        // the assertion that a genesis scan is really a genesis scan.
        let genesis_seed = entry.cipher_seed_for(Birthday::Genesis).unwrap();
        assert_eq!(genesis_seed.birthday(), 0);

        let birthday_seed = entry.cipher_seed_for(Birthday::FromSeed).unwrap();
        assert_eq!(birthday_seed.birthday(), entry.birthday);
        assert!(
            birthday_seed.birthday() > 0,
            "a funded seed has a real birthday"
        );

        // Same wallet either way: only the birthday field moved.
        assert_eq!(genesis_seed.entropy(), birthday_seed.entropy());
        assert!(entry.birthday_epoch_secs() >= BIRTHDAY_GENESIS_FROM_UNIX_EPOCH);
    }

    #[test]
    fn the_mnemonic_handed_to_the_console_wallet_carries_the_same_birthday() {
        let words = generate_seed_words().unwrap();
        let entry = describe_seed("old_wallet", &words, NETWORK, true).unwrap();

        // The console wallet only accepts seed words, so the birthday has to survive the
        // round trip through the mnemonic.
        let genesis_words = entry.words_for(Birthday::Genesis).unwrap();
        assert_eq!(
            cipher_seed_from_words(&genesis_words).unwrap().birthday(),
            0
        );
        assert_ne!(genesis_words, words, "the encoded birthday changed");

        let birthday_words = entry.words_for(Birthday::FromSeed).unwrap();
        assert_eq!(
            birthday_words, words,
            "an unmodified birthday re-encodes identically"
        );
        assert_eq!(
            derive_one_sided_address(&genesis_words, NETWORK)
                .unwrap()
                .to_base58(),
            entry.address,
            "the genesis-birthday seed must recover the funded wallet"
        );
    }

    #[test]
    fn the_scan_start_steps_back_from_the_birthday_by_the_wallets_offset() {
        let entry = SeedEntry {
            label: "old_wallet".to_string(),
            seed_words: String::new(),
            birthday: 1_000,
            address: String::new(),
            needs_funding: true,
        };
        assert_eq!(
            entry.birthday_epoch_secs(),
            BIRTHDAY_GENESIS_FROM_UNIX_EPOCH + 1_000 * SECONDS_PER_DAY
        );
        assert_eq!(
            entry.scan_start_epoch_secs(2),
            BIRTHDAY_GENESIS_FROM_UNIX_EPOCH + 998 * SECONDS_PER_DAY,
            "both wallets start two days before the birthday, and blocks_scanned has to say so"
        );

        // A genesis-birthday seed cannot be pushed below the epoch by the offset.
        let genesis = SeedEntry {
            birthday: 0,
            ..entry
        };
        assert_eq!(
            genesis.scan_start_epoch_secs(2),
            BIRTHDAY_GENESIS_FROM_UNIX_EPOCH
        );
    }

    #[test]
    fn ensure_is_idempotent_and_load_round_trips() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("seeds.json");

        let mut book = SeedBook::load_or_new(&path, "esmeralda").unwrap();
        let first = book.ensure("old_wallet", NETWORK, true).unwrap().clone();
        let again = book.ensure("old_wallet", NETWORK, true).unwrap().clone();
        assert_eq!(first, again, "an existing seed is never regenerated");
        book.ensure(&baseline_label("old_wallet"), NETWORK, false)
            .unwrap();
        book.save(&path).unwrap();

        let loaded = SeedBook::load(&path, "esmeralda").unwrap();
        assert_eq!(loaded, book);
        assert_eq!(loaded.funding_targets().len(), 1);
        assert_eq!(loaded.funding_targets()[0].label, "old_wallet");
        assert!(loaded.require("old_wallet-baseline").is_ok());
        assert!(loaded.require("missing").is_err());
    }

    #[test]
    fn loading_a_seed_file_from_another_network_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("seeds.json");
        SeedBook::new("esmeralda").save(&path).unwrap();
        assert!(SeedBook::load(&path, "mainnet").is_err());
    }

    #[test]
    fn recipient_addresses_are_distinct() {
        let addresses = generate_recipient_addresses(5, NETWORK).unwrap();
        let unique: std::collections::HashSet<_> = addresses.iter().collect();
        assert_eq!(unique.len(), 5);
    }
}
