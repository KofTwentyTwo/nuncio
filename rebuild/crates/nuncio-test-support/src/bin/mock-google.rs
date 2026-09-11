use clap::{Parser, ValueEnum};
use nuncio_test_support::{
    google::{CursorScope, Fault, GoogleControl, MockGoogle, Seed},
    TestError,
};
use serde::Deserialize;
use serde_json::{json, Value};
use std::{collections::BTreeSet, io::Write, path::PathBuf};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, BufReader};

#[derive(Clone, Copy, ValueEnum)]
enum SeedArg {
    TwoAccounts,
}
#[derive(Parser)]
#[command(about = "Independent local Google OAuth/Gmail/Calendar test service")]
struct Args {
    #[arg(long)]
    ready_file: PathBuf,
    #[arg(long, value_enum, default_value = "two-accounts")]
    seed: SeedArg,
}
#[derive(Deserialize)]
#[serde(tag = "op", rename_all = "snake_case", deny_unknown_fields)]
enum Command {
    SetCalendarRole {
        account: String,
        calendar: String,
        role: String,
    },
    OmitCalendarFields {
        account: String,
        id: String,
        fields: Vec<String>,
    },
    RemoveCalendar {
        account: String,
        id: String,
    },
    OmitMessageFields {
        account: String,
        id: String,
        fields: Vec<String>,
    },
    Snapshot,
    Reset,
    Shutdown,
    PageCap {
        size: usize,
    },
    PageOverlap {
        enabled: bool,
    },
    Reverse {
        enabled: bool,
    },
    Advance {
        seconds: u64,
    },
    Revoke {
        account: String,
    },
    DenyConsent {
        deny: bool,
    },
    DenyScope {
        scope: String,
    },
    RotateRefreshTokens {
        enabled: bool,
    },
    ExpireCursor {
        account: String,
        scope: CursorScope,
    },
    Inject {
        fault: Fault,
    },
    Release {
        barrier: String,
    },
    DeleteMessage {
        account: String,
        id: String,
    },
    AddMessage {
        account: String,
        id: String,
        thread: String,
        raw: Vec<u8>,
        labels: BTreeSet<String>,
    },
    ChangeLabels {
        account: String,
        id: String,
        add: Vec<String>,
        remove: Vec<String>,
    },
    PutEvent {
        account: String,
        calendar: String,
        event: Value,
    },
    DeleteEvent {
        account: String,
        calendar: String,
        id: String,
    },
}

#[tokio::main]
async fn main() -> std::process::ExitCode {
    match run(Args::parse()).await {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(_) => {
            eprintln!("mock-google: failed to start or operate local test service");
            std::process::ExitCode::FAILURE
        }
    }
}
async fn run(args: Args) -> Result<(), TestError> {
    let mock = MockGoogle::start(Seed::TwoAccounts).await?;
    let ready = json!({"event":"ready","base_url":mock.base_url(),"seed":"two-accounts","pid":std::process::id()});
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(&args.ready_file)?;
    serde_json::to_writer(&mut file, &ready)?;
    file.write_all(b"\n")?;
    file.sync_all()?;
    output(&ready)?;
    let mut reader = BufReader::new(tokio::io::stdin());
    loop {
        let mut bytes = Vec::new();
        let read = async {
            let mut bounded = (&mut reader).take(2 * 1024 * 1024);
            bounded.read_until(b'\n', &mut bytes).await
        };
        tokio::select! {
            result=read=>{
                let count=result?;
                if count==0 { break; }
                if bytes.last()!=Some(&b'\n') { return Err(std::io::Error::new(std::io::ErrorKind::InvalidInput,"control line too long or unterminated").into()); }
                match serde_json::from_slice::<Command>(&bytes) {
                    Ok(Command::Shutdown)=>{ output(&json!({"schema_version":1,"result":{"shutdown":true}}))?; break; }
                    Ok(command)=>match execute(mock.control(),command).await {
                        Ok(result)=>output(&json!({"schema_version":1,"result":result}))?,
                        Err(_)=>output(&json!({"schema_version":1,"error":{"code":"invalid_control","message":"control could not be applied"}}))?,
                    }
                    Err(_)=>output(&json!({"schema_version":1,"error":{"code":"invalid_control","message":"invalid control command"}}))?,
                }
            }
            result=tokio::signal::ctrl_c()=>{ result?; break; }
        }
    }
    mock.shutdown().await?;
    Ok(())
}
fn output(value: &Value) -> Result<(), TestError> {
    let mut stdout = std::io::stdout().lock();
    serde_json::to_writer(&mut stdout, value)?;
    stdout.write_all(b"\n")?;
    stdout.flush()?;
    Ok(())
}
async fn execute(control: GoogleControl, command: Command) -> Result<Value, TestError> {
    match command {
        Command::OmitCalendarFields {
            account,
            id,
            fields,
        } => {
            control
                .omit_calendar_fields(
                    &account,
                    &id,
                    &fields.iter().map(String::as_str).collect::<Vec<_>>(),
                )
                .await?
        }
        Command::RemoveCalendar { account, id } => control.remove_calendar(&account, &id).await?,
        Command::OmitMessageFields {
            account,
            id,
            fields,
        } => {
            control
                .omit_message_fields(
                    &account,
                    &id,
                    &fields.iter().map(String::as_str).collect::<Vec<_>>(),
                )
                .await?
        }
        Command::Snapshot => return Ok(serde_json::to_value(control.snapshot().await)?),
        Command::Reset => control.reset(Seed::TwoAccounts).await?,
        Command::PageCap { size } => control.set_page_cap(size).await?,
        Command::PageOverlap { enabled } => control.set_page_overlap(enabled).await,
        Command::Reverse { enabled } => control.reverse_results(enabled).await,
        Command::Advance { seconds } => {
            control
                .advance(std::time::Duration::from_secs(seconds))
                .await
        }
        Command::Revoke { account } => control.revoke(&account).await,
        Command::DenyConsent { deny } => control.deny_consent(deny).await,
        Command::DenyScope { scope } => control.deny_scope(&scope).await,
        Command::RotateRefreshTokens { enabled } => control.rotate_refresh_tokens(enabled).await,
        Command::ExpireCursor { account, scope } => control.expire_cursor(&account, scope).await?,
        Command::Inject { fault } => control.inject(fault).await,
        Command::Release { barrier } => control.release_barrier(&barrier).await,
        Command::DeleteMessage { account, id } => control.delete_message(&account, &id).await?,
        Command::AddMessage {
            account,
            id,
            thread,
            raw,
            labels,
        } => {
            control
                .add_message(&account, &id, &thread, raw, labels)
                .await?
        }
        Command::ChangeLabels {
            account,
            id,
            add,
            remove,
        } => control.change_labels(&account, &id, &add, &remove).await?,
        Command::PutEvent {
            account,
            calendar,
            event,
        } => control.put_event(&account, &calendar, event).await?,
        Command::SetCalendarRole {
            account,
            calendar,
            role,
        } => {
            control
                .set_calendar_role(&account, &calendar, &role)
                .await?
        }
        Command::DeleteEvent {
            account,
            calendar,
            id,
        } => control.delete_event(&account, &calendar, &id).await?,
        Command::Shutdown => {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "shutdown is handled by owner",
            )
            .into())
        }
    }
    Ok(json!({"applied":true}))
}
