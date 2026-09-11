#![allow(clippy::unwrap_used)]
use nuncio_engine::domain::{
    drafts::Recipient,
    prepare::{prepare, PrepareKind},
    submission::{freeze, SubmissionTransport},
};
#[test]
fn smtp_freezes_a_final_crlf_even_when_the_editor_has_no_trailing_newline() {
    let content = nuncio_engine::domain::drafts::DraftContent {
        to: vec![Recipient {
            address: "beta@example.test".into(),
            name: None,
        }],
        text: Some("One intended delivery".into()),
        ..Default::default()
    };
    let message = freeze(
        "alpha@example.test",
        &content,
        None,
        &[],
        "canonical@nuncio.invalid",
        1000,
        SubmissionTransport::Smtp,
    )
    .unwrap();
    assert!(
        message.wire.ends_with(b"\r\n"),
        "the committed SMTP MIME must include its final CRLF"
    );
}
#[test]
fn frozen_mime_preserves_forward_attachment_bytes_and_keeps_smtp_bcc_only_in_sent_copy() {
    let raw = include_str!("../../nuncio-test-support/fixtures/mime/reply-forward.eml")
        .replace('\n', "\r\n");
    let mut prepared = prepare(
        raw.as_bytes(),
        "alpha@example.test",
        "source",
        Some("old-thread"),
        PrepareKind::Forward,
        Some("Send this".into()),
        vec![Recipient {
            address: "visible@example.test".into(),
            name: Some("Zoë".into()),
        }],
    )
    .unwrap();
    prepared.content.bcc.push(Recipient {
        address: "private@example.test".into(),
        name: None,
    });
    let frozen = freeze(
        "alpha@example.test",
        &prepared.content,
        Some(&prepared.context),
        &prepared.attachments,
        "new-message@nuncio.invalid",
        1772895600000,
        SubmissionTransport::Smtp,
    )
    .unwrap();
    assert_eq!(
        frozen.recipients,
        ["visible@example.test", "private@example.test"]
    );
    assert!(frozen.thread_id.is_none());
    let wire = mail_parser::MessageParser::default()
        .parse(&frozen.wire)
        .unwrap();
    assert!(wire.bcc().is_none());
    assert_eq!(
        wire.from().unwrap().first().unwrap().address.as_deref(),
        Some("alpha@example.test")
    );
    assert_eq!(wire.message_id(), Some("new-message@nuncio.invalid"));
    let sent = mail_parser::MessageParser::default()
        .parse(frozen.sent_copy.as_ref().unwrap())
        .unwrap();
    assert_eq!(
        sent.bcc().unwrap().first().unwrap().address.as_deref(),
        Some("private@example.test")
    );
    let resent = nuncio_engine::domain::submission::reidentify(
        &frozen.wire,
        "resend@nuncio.invalid",
        1772895605000,
    )
    .unwrap();
    let resent_copy = nuncio_engine::domain::submission::reidentify(
        frozen.sent_copy.as_ref().unwrap(),
        "resend@nuncio.invalid",
        1772895605000,
    )
    .unwrap();
    let parsed = mail_parser::MessageParser::default()
        .parse(&resent)
        .unwrap();
    assert_eq!(parsed.message_id(), Some("resend@nuncio.invalid"));
    assert_ne!(parsed.date(), wire.date());
    assert!(parsed.bcc().is_none());
    let body = |bytes: &[u8]| bytes.windows(4).position(|w| w == b"\r\n\r\n").unwrap() + 4;
    assert_eq!(&resent[body(&resent)..], &frozen.wire[body(&frozen.wire)..]);
    assert_eq!(
        &resent_copy[body(&resent_copy)..],
        &frozen.sent_copy.as_ref().unwrap()[body(frozen.sent_copy.as_ref().unwrap())..]
    );
    let resent_copy = mail_parser::MessageParser::default()
        .parse(&resent_copy)
        .unwrap();
    assert_eq!(resent_copy.message_id(), parsed.message_id());
    assert_eq!(resent_copy.bcc(), sent.bcc());
    let decoded = nuncio_engine::domain::mail::decode_mime(&frozen.wire, 64 * 1024 * 1024).unwrap();
    assert_eq!(decoded.attachments.len(), 2);
    assert_eq!(
        decoded
            .attachments
            .iter()
            .find(|a| a.filename.as_deref() == Some("cafe.txt"))
            .unwrap()
            .bytes,
        b"caf\xe9"
    );
    assert_eq!(
        decoded
            .attachments
            .iter()
            .find(|a| a.content_id.as_deref() == Some("logo@example.test"))
            .unwrap()
            .bytes,
        b"\x89PNG\r\n\x1a\n"
    );
    use mail_parser::MimeHeaders;
    let text_part = wire
        .attachments()
        .find(|a| a.attachment_name() == Some("cafe.txt"))
        .unwrap();
    assert_eq!(
        text_part.content_type().unwrap().attribute("charset"),
        Some("windows-1252")
    );
    assert!(!frozen
        .wire
        .windows(b"private@example.test".len())
        .any(|w| w == b"private@example.test"));
    let gmail = freeze(
        "alpha@example.test",
        &prepared.content,
        Some(&prepared.context),
        &prepared.attachments,
        "gmail-message@nuncio.invalid",
        1772895600000,
        SubmissionTransport::Gmail,
    )
    .unwrap();
    assert!(gmail.sent_copy.is_none());
    let gmail = mail_parser::MessageParser::default()
        .parse(&gmail.wire)
        .unwrap();
    assert_eq!(
        gmail.bcc().unwrap().first().unwrap().address.as_deref(),
        Some("private@example.test")
    );
}
