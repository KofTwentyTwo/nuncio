//! E2E System Test Suite for nuncio-cli.

use nuncio_cli::{
    AccountSubcommand, CalSubcommand, Commands, FolderSubcommand, HeadlessRunner, MailSubcommand,
    SystemSubcommand,
};
use serde_json::Value;

#[tokio::test]
async fn system_test_cli_noun_verb_execution_matrix() {
    let runner = HeadlessRunner::ephemeral().await.expect("runner init");

    // 1. System status
    let out: String = runner
        .execute_command(
            &Commands::System {
                action: SystemSubcommand::Status,
            },
            true,
        )
        .await;
    let json: Value = serde_json::from_str(&out).expect("valid json");
    assert_eq!(json["status"], "ok");

    // 1b. Banner output
    let out: String = runner.execute_command(&Commands::Banner, true).await;
    let json: Value = serde_json::from_str(&out).expect("valid json");
    assert_eq!(json["status"], "ok");
    assert_eq!(json["data"]["name"], "Nuncio");

    // 1c. Licenses output
    let out: String = runner.execute_command(&Commands::Licenses, true).await;
    let json: Value = serde_json::from_str(&out).expect("valid json");
    assert_eq!(json["status"], "ok");

    // 2. Account list & add
    let out: String = runner
        .execute_command(
            &Commands::Account {
                action: AccountSubcommand::List,
            },
            true,
        )
        .await;
    let json: Value = serde_json::from_str(&out).expect("valid json");
    assert_eq!(json["status"], "ok");

    let out: String = runner
        .execute_command(
            &Commands::Account {
                action: AccountSubcommand::Add {
                    email: "test@nuncio.mx".to_string(),
                    imap_host: "mail.nuncio.mx".to_string(),
                    imap_port: 993,
                    smtp_host: "mail.nuncio.mx".to_string(),
                    smtp_port: 465,
                    imap_mode: "implicit_tls".to_string(),
                    smtp_mode: "implicit_tls".to_string(),
                },
            },
            true,
        )
        .await;
    let json: Value = serde_json::from_str(&out).expect("valid json");
    assert_eq!(json["status"], "ok");

    // 3. Folder list
    let out: String = runner
        .execute_command(
            &Commands::Folder {
                action: FolderSubcommand::List,
            },
            true,
        )
        .await;
    let json: Value = serde_json::from_str(&out).expect("valid json");
    assert_eq!(json["status"], "ok");

    // 4. Mail list & search & read
    let out: String = runner
        .execute_command(
            &Commands::Mail {
                action: MailSubcommand::List {
                    folder: "INBOX".to_string(),
                },
            },
            true,
        )
        .await;
    let json: Value = serde_json::from_str(&out).expect("valid json");
    assert_eq!(json["status"], "ok");

    let out: String = runner
        .execute_command(
            &Commands::Mail {
                action: MailSubcommand::Search {
                    query: "Architecture".to_string(),
                },
            },
            true,
        )
        .await;
    let json: Value = serde_json::from_str(&out).expect("valid json");
    assert_eq!(json["status"], "ok");

    // 5. Calendar list & sync
    let out: String = runner
        .execute_command(
            &Commands::Cal {
                action: CalSubcommand::List,
            },
            true,
        )
        .await;
    let json: Value = serde_json::from_str(&out).expect("valid json");
    assert_eq!(json["status"], "ok");
}
