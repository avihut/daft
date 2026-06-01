pub mod merge;

use anyhow::Result;

pub fn run() -> Result<()> {
    let args: Vec<String> = std::env::args().collect();
    // Find the "file" arg and route to subcommands after it
    let file_idx = args.iter().position(|a| a == "file").unwrap_or(1);
    let sub_args: Vec<String> = args[(file_idx + 1)..].to_vec();

    if sub_args.is_empty() {
        show_usage();
        return Ok(());
    }

    match sub_args[0].as_str() {
        "merge" => merge::run(&sub_args[1..]),
        "--help" | "-h" => {
            show_usage();
            Ok(())
        }
        other => {
            anyhow::bail!(
                "Unknown file subcommand: '{}'\n\nUsage: daft file merge",
                other
            );
        }
    }
}

fn show_usage() {
    eprintln!("Usage: daft file <subcommand>");
    eprintln!();
    eprintln!("Available subcommands:");
    eprintln!("  merge    Merge a source daft.yml into a target daft.yml");
    eprintln!();
    eprintln!("Run 'daft file <subcommand> --help' for details.");
}
