pub mod remote_sync;

use anyhow::Result;

pub fn run() -> Result<()> {
    let args: Vec<String> = std::env::args().collect();
    // Find the "config" arg and route to subcommands after it
    let config_idx = args.iter().position(|a| a == "config").unwrap_or(1);
    let sub_args: Vec<String> = args[(config_idx + 1)..].to_vec();

    if sub_args.is_empty() {
        show_usage();
        return Ok(());
    }

    match sub_args[0].as_str() {
        "remote-sync" => remote_sync::run(&sub_args[1..]),
        "--help" | "-h" => {
            show_usage();
            Ok(())
        }
        other => {
            anyhow::bail!(
                "Unknown config subcommand: '{}'\n\nUsage: daft config remote-sync",
                other
            );
        }
    }
}

fn show_usage() {
    eprintln!("Usage: daft config <subcommand>");
    eprintln!();
    eprintln!("Available subcommands:");
    eprintln!("  remote-sync    Configure remote sync behavior");
    eprintln!();
    eprintln!("Run 'daft config <subcommand> --help' for details.");
}
