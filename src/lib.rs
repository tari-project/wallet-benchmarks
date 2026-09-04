//! Reproducible performance benchmark harness for the Tari wallets.
//!
//! The harness measures two wallet implementations against the same protocol:
//!
//! * **old wallet** — `minotari_console_wallet`, driven over gRPC ([`modes::old_wallet`])
//! * **new wallet** — `minotari-cli` used as a library, signing offline ([`modes::new_wallet`])
//!
//! Both are hidden behind the [`modes::WalletMode`] trait, and every benchmark is a
//! [`scenarios::Scenario`]. A run is therefore the cross product of the requested modes and
//! the requested scenarios; see [`runner`].
//!
//! The harness deliberately does not retry, back off, throttle or pre-partition UTXOs.
//! Stalls, lock contention and construction failures are results, not problems to engineer
//! around.

pub mod binaries;
pub mod chain;
pub mod config;
pub mod metrics;
pub mod modes;
pub mod runner;
pub mod scenarios;
pub mod secrets;
pub mod seeds;
pub mod sysprobe;
