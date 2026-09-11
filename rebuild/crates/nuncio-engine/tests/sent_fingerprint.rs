#![allow(clippy::unwrap_used)]
use nuncio_engine::domain::submission::SentFingerprint;

const ORIGINAL: &str="From: Alpha <alpha@example.test>\r\nTo: beta@example.test\r\nBcc: blind@example.test\r\nMessage-ID: <one@nuncio.invalid>\r\nDate: Thu, 10 Sep 2026 12:00:00 +0000\r\nSubject: Frozen\r\nMIME-Version: 1.0\r\nContent-Type: text/plain; charset=utf-8\r\n\r\nBody\r\n";
#[test]
fn server_sent_content_requires_identity_headers_and_exact_body_but_accepts_trace_headers() {
    let expected = SentFingerprint::from_mime(ORIGINAL.as_bytes()).unwrap();
    assert!(
        SentFingerprint::from_mime(ORIGINAL.replace("Subject: Frozen", "Subject:").as_bytes())
            .is_ok()
    );
    let remote =
        format!("Received: from synthetic by local; Thu, 10 Sep 2026 12:01:00 +0000\r\n{ORIGINAL}");
    assert!(expected.matches(&SentFingerprint::from_mime(remote.as_bytes()).unwrap()));
    for (old, new) in [
        ("Alpha <alpha@example.test>", "Other <other@example.test>"),
        ("beta@example.test", "another@example.test"),
        ("one@nuncio.invalid", "two@nuncio.invalid"),
        ("12:00:00", "12:00:01"),
        ("Subject: Frozen", "Subject: changed"),
        ("charset=utf-8", "charset=iso-8859-1"),
        ("Body\r\n", "Other body\r\n"),
    ] {
        assert!(
            !expected.matches(
                &SentFingerprint::from_mime(ORIGINAL.replace(old, new).as_bytes()).unwrap()
            ),
            "{old}"
        );
    }
    let without_bcc = ORIGINAL.replace("Bcc: blind@example.test\r\n", "");
    assert!(
        expected.matches(&SentFingerprint::from_mime(without_bcc.as_bytes()).unwrap()),
        "a server may omit private envelope recipients from its Sent MIME"
    );
    assert!(
        !SentFingerprint::from_mime(without_bcc.as_bytes())
            .unwrap()
            .matches(&expected),
        "unexpected private recipients are not silently accepted"
    );
    assert!(!expected.matches(
        &SentFingerprint::from_mime(
            ORIGINAL
                .replace("blind@example.test", "other-blind@example.test")
                .as_bytes()
        )
        .unwrap()
    ));
}
#[test]
fn server_sent_fingerprint_rejects_duplicate_identity_headers_and_is_versioned() {
    for header in [
        "Message-ID: <other@nuncio.invalid>\r\n",
        "From: other@example.test\r\n",
        "Bcc: other@example.test\r\n",
    ] {
        assert!(SentFingerprint::from_mime(format!("{header}{ORIGINAL}").as_bytes()).is_err());
    }
    let expected = SentFingerprint::from_mime(ORIGINAL.as_bytes()).unwrap();
    let encoded = serde_json::to_value(&expected).unwrap();
    assert_eq!(encoded["version"], 1);
    assert!(!encoded.to_string().contains("alpha@example.test"));
    let mut future = encoded;
    future["version"] = 2.into();
    let future = serde_json::from_value(future).unwrap();
    assert!(!expected.matches(&future));
}
