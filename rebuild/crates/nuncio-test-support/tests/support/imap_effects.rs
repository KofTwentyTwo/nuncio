use nuncio_test_support::{imap::MockMailPlus, TestError};
use serde_json::{json, Value};

pub(crate) async fn effects(mock: &mut MockMailPlus) -> Result<Value, TestError> {
    let state = mock.control(json!({"command":"snapshot"})).await?;
    let mut writes = serde_json::Map::new();
    for verb in [
        "imap UID STORE",
        "imap UID COPY",
        "imap UID MOVE",
        "imap UID EXPUNGE",
        "imap EXPUNGE",
        "imap APPEND",
        "smtp DATA",
    ] {
        writes.insert(
            verb.into(),
            json!([
                state["requests"][verb].as_u64().unwrap_or(0),
                state["accepted"][verb].as_u64().unwrap_or(0)
            ]),
        );
    }
    let mut mailboxes = serde_json::Map::new();
    for mailbox in ["INBOX", "Archive", "Trash", "Sent"] {
        mailboxes.insert(
            mailbox.into(),
            mock.control(
                json!({"command":"mailbox","account":"alpha@example.test","mailbox":mailbox}),
            )
            .await?,
        );
    }
    let other = mock
        .control(json!({"command":"mailbox","account":"beta@example.test","mailbox":"INBOX"}))
        .await?;
    Ok(
        json!({"writes":writes,"mailboxes":mailboxes,"other":other,"deliveries":state["smtp_deliveries"]}),
    )
}
