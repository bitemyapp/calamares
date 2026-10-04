// SPDX-License-Identifier: GPL-3.0-or-later
use anyhow::{Result, ensure};
use calamares_nixos::{
    RawRequest, Settings, disk,
    install::{self, Event, InstallMode},
};
use std::io::Read;
use zeroize::Zeroizing;

fn run() -> Result<()> {
    let args: Vec<_> = std::env::args().skip(1).collect();
    if args == ["discover"] {
        println!("{}", serde_json::to_string(&disk::discover()?)?);
    } else if args == ["install"] || args == ["preflight"] {
        let mut input = Zeroizing::new(Vec::new());
        std::io::stdin().take(1048577).read_to_end(&mut input)?;
        ensure!(input.len() <= 1048576, "Request too large");
        let request: RawRequest = serde_json::from_slice(&input)
            .map_err(|_| anyhow::anyhow!("Invalid installation request"))?;
        drop(input);
        let confirmed = request.parse_confirmed(&Settings::load()?)?;
        let mode = if args == ["preflight"] {
            InstallMode::Preflight
        } else {
            InstallMode::Execute
        };
        install::install(confirmed, mode)?;
    } else {
        anyhow::bail!(
            "Usage: calamares-nixos-helper discover|preflight|install; installation requests are JSON on stdin"
        );
    }
    Ok(())
}
fn main() {
    if let Err(error) = run() {
        install::event(Event::Failed {
            message: format!("{error:#}"),
        });
        std::process::exit(1);
    }
}
