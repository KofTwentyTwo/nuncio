use clap::Parser;
use nuncio_cli::{args::Cli, HeadlessRunner};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let cli = Cli::parse();
    let runner = HeadlessRunner::ephemeral().await?;

    let output_str = runner.execute_command(&cli.command, cli.json).await;
    println!("{}", output_str);

    Ok(())
}
