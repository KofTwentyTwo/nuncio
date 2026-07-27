use clap::Parser;
use nuncio_cli::{args::Cli, AccountSubcommand, Commands, HeadlessRunner, PasswordArg};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut cli = Cli::parse();

    // `account add`'s password is deliberately never a Clap-parsed CLI
    // flag (see `PasswordArg`'s doc comment): read it here, interactively
    // and without echo, immediately after parsing argv and before any
    // other work, then inject it into the already-parsed command. This is
    // the ONLY place in the whole `nuncio-cli` binary that ever reads a raw
    // password from the terminal.
    if let Commands::Account {
        action: AccountSubcommand::Add { password, .. },
    } = &mut cli.command
    {
        let entered = rpassword::prompt_password("Account password: ")
            .map_err(|e| format!("failed to read password: {e}"))?;
        *password = PasswordArg(entered);
    }

    // `account edit --rotate-password` follows the exact same pattern:
    // only prompt when the caller actually asked to rotate the credential,
    // so a plain settings edit (hostname/port/etc.) never touches the
    // terminal for a password it isn't changing.
    if let Commands::Account {
        action:
            AccountSubcommand::Edit {
                rotate_password: true,
                password,
                ..
            },
    } = &mut cli.command
    {
        let entered = rpassword::prompt_password("New account password: ")
            .map_err(|e| format!("failed to read password: {e}"))?;
        *password = PasswordArg(entered);
    }

    let runner = HeadlessRunner::ephemeral().await?;

    let output_str = runner.execute_command(&cli.command, cli.json).await;
    println!("{}", output_str);

    Ok(())
}
