//! Nuncio CLI main application entry point.

mod args;
mod output;
mod runner;

use args::Cli;
use clap::Parser;
use runner::HeadlessRunner;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let cli = Cli::parse();
    let runner = HeadlessRunner::ephemeral().await?;

    let output_str = runner.execute_command(&cli.command, cli.json).await;
    println!("{}", output_str);

    Ok(())
}
