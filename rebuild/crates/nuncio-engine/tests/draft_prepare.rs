#![allow(clippy::unwrap_used)]
use nuncio_engine::domain::{
    drafts::Recipient,
    prepare::{prepare, PrepareKind},
};
fn original() -> Vec<u8> {
    include_str!("../../nuncio-test-support/fixtures/mime/reply-forward.eml")
        .replace('\n', "\r\n")
        .into_bytes()
}
#[test]
fn untrusted_decoded_headers_fail_closed_and_single_parent_in_reply_to_is_retained() {
    let raw = original();
    let raw = String::from_utf8(raw).unwrap().replace(
        "References: <root@example.test> <parent@example.test>",
        "In-Reply-To: <single-parent@example.test>",
    );
    let prepared = prepare(
        raw.as_bytes(),
        "alpha@example.test",
        "source",
        None,
        PrepareKind::Reply,
        None,
        vec![],
    )
    .unwrap();
    assert_eq!(
        prepared.context.references,
        ["single-parent@example.test", "incoming@example.test"]
    );
    let raw = raw.replace(
        "Subject: Team question",
        "Subject: =?UTF-8?Q?Injected=0D=0ABcc:_attacker@example.test?=",
    );
    assert!(prepare(
        raw.as_bytes(),
        "alpha@example.test",
        "source",
        None,
        PrepareKind::Reply,
        None,
        vec![]
    )
    .is_err());
}
#[test]
fn reply_all_uses_reply_to_arrays_excludes_self_bcc_and_duplicates_and_keeps_references() {
    let raw = original();
    let prepared = prepare(
        &raw,
        "alpha@example.test",
        "local-original",
        Some("provider-thread"),
        PrepareKind::ReplyAll,
        Some("My response".into()),
        vec![],
    )
    .unwrap();
    assert_eq!(
        prepared
            .content
            .to
            .iter()
            .map(|r| r.address.as_str())
            .collect::<Vec<_>>(),
        [
            "alice@example.test",
            "bob@example.test",
            "carol@example.test"
        ]
    );
    assert_eq!(
        prepared
            .content
            .cc
            .iter()
            .map(|r| r.address.as_str())
            .collect::<Vec<_>>(),
        ["david@example.test"]
    );
    assert!(prepared.content.bcc.is_empty());
    assert!(prepared.attachments.is_empty());
    assert_eq!(prepared.content.text.as_deref(), Some("My response"));
    assert_eq!(prepared.content.subject, "Re: Team question");
    assert_eq!(
        prepared.context.in_reply_to.as_deref(),
        Some("incoming@example.test")
    );
    assert_eq!(
        prepared.context.references,
        [
            "root@example.test",
            "parent@example.test",
            "incoming@example.test"
        ]
    );
    assert_eq!(
        prepared.context.thread_id.as_deref(),
        Some("provider-thread")
    );
    let reply = prepare(
        &raw,
        "alpha@example.test",
        "local-original",
        Some("thread"),
        PrepareKind::Reply,
        None,
        vec![],
    )
    .unwrap();
    assert_eq!(reply.content.to.len(), 2);
    assert!(reply.content.cc.is_empty());
}
#[test]
fn forward_preserves_text_attachment_encoding_semantics_inline_cid_and_original_bodies() {
    let prepared = prepare(
        &original(),
        "alpha@example.test",
        "source",
        Some("old-thread"),
        PrepareKind::Forward,
        Some("See below.".into()),
        vec![Recipient {
            address: "new@example.test".into(),
            name: None,
        }],
    )
    .unwrap();
    assert_eq!(prepared.content.to[0].address, "new@example.test");
    assert_eq!(prepared.content.subject, "Fwd: Team question");
    assert!(prepared
        .content
        .text
        .as_deref()
        .unwrap()
        .contains("Original plain body."));
    assert!(prepared
        .content
        .html
        .as_deref()
        .unwrap()
        .contains("cid:logo@example.test"));
    assert!(prepared.context.thread_id.is_none());
    assert!(prepared.context.in_reply_to.is_none());
    assert!(prepared.context.references.is_empty());
    assert_eq!(prepared.attachments.len(), 2);
    assert_eq!(prepared.attachments[0].bytes, b"caf\xe9");
    assert_eq!(
        prepared.attachments[0].parameters["charset"],
        "windows-1252"
    );
    assert_eq!(prepared.attachments[0].parameters["format"], "fixed");
    assert_eq!(prepared.attachments[1].disposition, "inline");
    assert_eq!(
        prepared.attachments[1].content_id.as_deref(),
        Some("logo@example.test")
    );
    assert_eq!(prepared.attachments[1].bytes, b"\x89PNG\r\n\x1a\n");
}
