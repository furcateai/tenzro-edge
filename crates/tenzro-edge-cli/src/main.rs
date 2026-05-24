// SPDX-License-Identifier: Apache-2.0

//! `tenzro-edge` CLI — Pi-class Tenzro participation runtime.
//!
//! Mirrors the shape of the `minima-attest` CLI: `login`, `status`, `replay`,
//! `commit`, `verify`. Designed for unattended boot via env-var creds.

use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(
    name = "tenzro-edge",
    version,
    about = "Pi-class Tenzro participation runtime"
)]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// One-shot login; persists token at `~/.furcate/tenzro/token.json` (0600).
    Login {
        /// Account identifier; defaults to the Tenzro SDK default.
        #[arg(long)]
        account: Option<String>,
    },
    /// Show token TTL + last sync.
    Status,
    /// Flush any offline-buffered events.
    Replay,
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    match cli.cmd {
        Cmd::Login { account: _ } => {
            // Wiring lands in v0.1.x.
            eprintln!("tenzro-edge login: not yet implemented (v0.1.0 scaffold)");
        }
        Cmd::Status => {
            eprintln!("tenzro-edge status: not yet implemented (v0.1.0 scaffold)");
        }
        Cmd::Replay => {
            eprintln!("tenzro-edge replay: not yet implemented (v0.1.0 scaffold)");
        }
    }
    Ok(())
}
