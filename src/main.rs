//! Command line entry point for the Tari wallet benchmark harness.
//!
//! Typical order of operations:
//!
//! ```text
//! wallet-benchmarks prepare      # generate seeds, print the addresses to fund
//! # ... fund the printed addresses with A_fund and wait for confirmation ...
//! wallet-benchmarks run          # run the protocol and write a result profile
//! ```

use std::{path::PathBuf, sync::Arc};

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use wallet_benchmarks::{
    config::HarnessConfig,
    modes::{NEW_WALLET, OLD_WALLET, PAYMENT_PROCESSOR},
    runner,
    scenarios::{self, ALL},
    seeds::SeedBook,
};

#[derive(Parser, Debug)]
#[command(
    author,
    version,
    about = "Reproducible benchmark harness for the Tari wallets"
)]
struct Cli {
    /// Path to the harness configuration file.
    #[arg(long, short, default_value = "benchmark.toml", global = true)]
    config: PathBuf,

    /// Wallet modes to act on, comma separated.
    #[arg(
        long,
        value_delimiter = ',',
        default_values_t = [
            OLD_WALLET.to_string(),
            NEW_WALLET.to_string(),
            PAYMENT_PROCESSOR.to_string(),
        ],
        global = true
    )]
    modes: Vec<String>,

    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand, Debug)]
enum Command {
    /// Generate any missing seeds and print the addresses that need funding.
    Prepare,
    /// Print the funding addresses of the seeds that already exist.
    Addresses {
        /// Print JSON instead of a table.
        #[arg(long)]
        json: bool,
    },
    /// Print the run plan and the parameters without touching a wallet.
    Plan {
        /// Scenarios to plan, comma separated.
        #[arg(long, value_delimiter = ',')]
        scenarios: Option<Vec<String>>,
    },
    /// Print the summary tables for a profile that was written earlier.
    Show {
        /// Path to a `results/profile-*.json`.
        profile: PathBuf,
    },
    /// Run the benchmark protocol and write a result profile.
    Run {
        /// Scenarios to run, comma separated. Defaults to the full protocol in order.
        #[arg(long, value_delimiter = ',')]
        scenarios: Option<Vec<String>>,
    },
}

#[tokio::main]
async fn main() -> Result<()> {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();
    let cli = Cli::parse();
    let cfg = HarnessConfig::load(&cli.config)?;

    match cli.command {
        Command::Prepare => {
            let book = runner::prepare_seeds(&cfg, &cli.modes)?;
            println!("Seeds written to {}", cfg.seed_file.display());
            println!();
            print_funding_table(&cfg, &book);
        },
        Command::Addresses { json } => {
            let book = SeedBook::load(&cfg.seed_file, &cfg.network)
                .context("Run `wallet-benchmarks prepare` first")?;
            if json {
                println!("{}", serde_json::to_string_pretty(&funding_json(&book))?);
            } else {
                print_funding_table(&cfg, &book);
            }
        },
        Command::Plan { scenarios } => {
            let ids = resolve_scenarios(scenarios)?;
            println!("network        : {}", cfg.network);
            println!("base node      : {}", cfg.base_node_url);
            println!("modes          : {}", cli.modes.join(", "));
            println!("scenarios      : {}", ids.join(", "));
            println!("A_fund         : {} µT", cfg.params.a_fund);
            println!("C_min          : {}", cfg.params.c_min);
            println!("fee rate       : {} µT/gram", cfg.params.fee_rate);
            println!(
                "S1 build-up    : {} doubling rounds then {} x {} outputs -> {} UTXOs",
                cfg.params.doubling_rounds,
                cfg.params.fanout_plan().txs,
                cfg.params.fanout_outputs_per_tx,
                cfg.params.expected_utxos_after_s1()
            );
            println!(
                "S4 ramp        : {:?} (budget {}s per level)",
                cfg.params.concurrent_batches, cfg.params.s4_budget_secs
            );
            println!(
                "S5             : {} recipients, {} per batch tx ({} batch txs)",
                cfg.params.s5_m,
                cfg.params.s5_k,
                cfg.params.s5_batch_txs()
            );
            for id in &ids {
                if let Some(scenario) = scenarios::scenario(id) {
                    println!("  {:<3} {}", scenario.id(), scenario.description());
                }
            }
        },
        Command::Show { profile } => {
            let text = std::fs::read_to_string(&profile)
                .with_context(|| format!("Could not read {}", profile.display()))?;
            let profile: wallet_benchmarks::metrics::ResultProfile = serde_json::from_str(&text)
                .with_context(|| "That file is not a result profile".to_string())?;
            println!(
                "{} on {} ({})\n",
                profile.generated_at.format("%Y-%m-%d %H:%M:%SZ"),
                profile.environment.cpu_model,
                profile.environment.base_node_path
            );
            print!("{}", profile.summary_report());
        },
        Command::Run { scenarios } => {
            let ids = resolve_scenarios(scenarios)?;
            let cfg = Arc::new(cfg);
            let profile = runner::run(Arc::clone(&cfg), &cli.modes, &ids).await?;
            let path = runner::write_profile(&cfg, &profile)?;
            println!();
            for line in profile.summary_lines() {
                println!("{line}");
            }
            println!();
            print!("{}", profile.summary_report());
            println!("Result profile written to {}", path.display());
        },
    }
    Ok(())
}

/// Defaults to the whole protocol, and rejects ids this harness does not implement.
fn resolve_scenarios(requested: Option<Vec<String>>) -> Result<Vec<String>> {
    let ids = requested.unwrap_or_else(|| ALL.iter().map(|s| (*s).to_string()).collect());
    for id in &ids {
        if scenarios::scenario(id).is_none() {
            anyhow::bail!(
                "Unknown scenario `{id}`. This harness implements: {}",
                ALL.join(", ")
            );
        }
    }
    Ok(ids)
}

fn funding_json(book: &SeedBook) -> serde_json::Value {
    serde_json::json!({
        "network": book.network,
        "wallets": book
            .entries
            .iter()
            .map(|e| serde_json::json!({
                "label": e.label,
                "address": e.address,
                "birthday": e.birthday,
                "needs_funding": e.needs_funding,
            }))
            .collect::<Vec<_>>(),
    })
}

fn print_funding_table(cfg: &HarnessConfig, book: &SeedBook) {
    println!(
        "Fund each of these addresses with a single UTXO of at least {} µT (A_fund is a minimum):",
        cfg.params.a_fund
    );
    println!();
    for entry in book.funding_targets() {
        println!("  {:<24} {}", entry.label, entry.address);
    }
    if book
        .funding_targets()
        .iter()
        .any(|e| e.label == PAYMENT_PROCESSOR)
    {
        println!();
        println!(
            "  `{PAYMENT_PROCESSOR}` is the address of the `{}` account inside the minotari\n  \
             daemon, which is the wallet for that mode. The payment processor holds no funds\n  \
             and has no address of its own: it watches that account by view key and asks the\n  \
             daemon to lock its UTXOs.",
            cfg.payment_processor.account_name
        );
    }
    println!();
    println!("These wallets must never be funded (they are the B0 baseline):");
    for entry in book.entries.iter().filter(|e| !e.needs_funding) {
        println!("  {:<24} {}", entry.label, entry.address);
    }
    println!();
    println!(
        "Wait until each funding transaction is at least {} blocks deep, then run \
         `wallet-benchmarks run`.",
        cfg.params.c_min
    );
}
