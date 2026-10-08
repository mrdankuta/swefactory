use clap::{Parser, Subcommand};
use serde::Serialize;

/// swefactory: the software factory as a product (single static binary).
#[derive(Parser, Debug)]
#[command(
    name = "factory",
    version,
    about = "swefactory — production-grade software, consumer-simple UX"
)]
struct Cli {
    /// Emit machine-parseable JSON for every command that produces output.
    #[arg(long, global = true)]
    json: bool,

    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand, Debug)]
enum Commands {
    /// Print the factory version.
    Version,
}

#[derive(Serialize)]
struct VersionOutput<'a> {
    version: &'a str,
}

fn main() {
    let cli = Cli::parse();

    match cli.command {
        Commands::Version => {
            let version = env!("CARGO_PKG_VERSION");
            if cli.json {
                println!(
                    "{}",
                    serde_json::to_string(&VersionOutput { version })
                        .expect("version JSON serialization")
                );
            } else {
                println!("{version}");
            }
        }
    }
}
