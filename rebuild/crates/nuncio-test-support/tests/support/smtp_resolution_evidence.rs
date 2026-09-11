use base64::{engine::general_purpose::STANDARD, Engine as _};
use mailparse::MailHeaderMap;
use nuncio_test_support::{imap::MockMailPlus, TestError};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

pub struct Observed {
    pub raw: Vec<u8>,
    pub message_id: String,
    uid: Value,
    epoch: Value,
}
impl Observed {
    pub fn check_placement(&self, provider_id: &str, account: &str) -> Result<(), TestError> {
        let placement: Value = serde_json::from_str(provider_id)?;
        assert_eq!(placement[0], 1);
        assert_eq!(placement[1], "imap");
        assert_eq!(placement[2]["account_id"], account);
        assert!(placement[2]["mailbox_id"].as_str().is_some());
        assert_eq!(placement[2]["uid"], self.uid);
        assert_eq!(placement[2]["uid_validity"], self.epoch);
        Ok(())
    }
}

pub async fn observe(mock: &mut MockMailPlus, server_sent: bool) -> Result<Observed, TestError> {
    let snapshot = mock.control(json!({"command":"snapshot"})).await?;
    assert_eq!(snapshot["requests"]["smtp DATA"], 1);
    assert_eq!(snapshot["accepted"]["smtp DATA"], 1);
    assert_eq!(snapshot["smtp_deliveries"].as_array().unwrap().len(), 1);
    for kind in ["requests", "accepted"] {
        assert_eq!(
            snapshot[kind]["imap APPEND"].as_u64().unwrap_or(0),
            u64::from(!server_sent)
        );
    }
    let envelope = &snapshot["smtp_deliveries"][0];
    assert_eq!(envelope["sender"], "alpha@example.test");
    assert_eq!(
        envelope["recipients"],
        json!(["beta@example.test", "blind@example.test"])
    );
    let smtp = mock.control(json!({"command":"smtp"})).await?;
    assert_eq!(smtp["total"], 1);
    assert_eq!(smtp["messages"][0]["Username"], "alpha@example.test");
    let capture = mock
        .control(
            json!({"command":"smtp_raw","transport":"starttls","id":smtp["messages"][0]["ID"]}),
        )
        .await?;
    let capture = STANDARD.decode(capture["raw_base64"].as_str().unwrap())?;
    let size = envelope["wire_size"].as_u64().unwrap() as usize;
    assert!(capture.len() >= size);
    let wire = &capture[capture.len() - size..];
    assert_eq!(
        format!("{:x}", Sha256::digest(wire)),
        envelope["wire_sha256"]
    );
    let sent = mock
        .control(json!({"command":"mailbox","account":"alpha@example.test","mailbox":"Sent"}))
        .await?;
    assert_eq!(sent["messages"].as_array().unwrap().len(), 1);
    let uid = sent["messages"][0]["uid"].clone();
    let raw = mock
        .control(json!({"command":"raw","account":"alpha@example.test","mailbox":"Sent","uid":uid}))
        .await?;
    let raw = STANDARD.decode(raw["raw_base64"].as_str().unwrap())?;
    assert_eq!(
        format!("{:x}", Sha256::digest(&raw)),
        sent["messages"][0]["sha256"]
    );
    let parsed = mailparse::parse_mail(&raw)?;
    let submitted = mailparse::parse_mail(wire)?;
    assert!(submitted.headers.get_first_value("Bcc").is_none());
    assert_eq!(
        parsed.headers.get_first_value("Message-ID"),
        submitted.headers.get_first_value("Message-ID")
    );
    for message in [&parsed, &submitted] {
        assert_eq!(
            message.headers.get_first_value("Subject").as_deref(),
            Some("SMTP resolution evidence")
        );
        assert_eq!(
            message.get_body()?.replace("\r\n", "\n"),
            "Independent acceptance and copy\n.leading dot\n"
        );
    }
    let message_id = parsed.headers.get_first_value("Message-ID").unwrap();
    let message_id = message_id
        .strip_prefix('<')
        .unwrap()
        .strip_suffix('>')
        .unwrap()
        .to_owned();
    Ok(Observed {
        raw,
        message_id,
        uid,
        epoch: sent["uidvalidity"].clone(),
    })
}
