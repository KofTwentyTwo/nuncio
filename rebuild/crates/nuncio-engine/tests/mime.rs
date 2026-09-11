#![allow(clippy::unwrap_used)]
use nuncio_engine::domain::mail::{decode_mime, MailError};

#[test]
fn original_attachment_bytes_and_explicit_body_types_survive_decoding() {
    let raw = include_str!("../../nuncio-test-support/fixtures/mime/multipart.eml")
        .replace("ACCOUNT", "alpha@example.test")
        .replace('\n', "\r\n");
    let decoded = decode_mime(raw.as_bytes(), 65536).unwrap();
    assert_eq!(decoded.subject.as_deref(), Some("Multipart fixture"));
    assert!(decoded.text.as_deref().unwrap().contains("searchable text"));
    assert!(decoded.html.is_none());
    assert_eq!(decoded.attachments.len(), 1);
    assert_eq!(
        decoded.attachments[0].filename.as_deref(),
        Some("sample.pdf")
    );
    assert_eq!(
        decoded.attachments[0].bytes,
        b"%PDF-1.4\nSynthetic PDF fixture\n%%EOF\n"
    );
    assert_eq!(decoded.attachments[0].mime_type, "application/pdf");
    let html = include_str!("../../nuncio-test-support/fixtures/mime/html-only.eml")
        .replace("ACCOUNT", "alpha@example.test")
        .replace('\n', "\r\n");
    let decoded = decode_mime(html.as_bytes(), 65536).unwrap();
    assert!(
        decoded.text.is_none(),
        "HTML-derived display text is not an original text/plain body"
    );
    assert!(decoded.html.is_some());
    assert!(!decoded.search_text.is_empty());
}

#[test]
fn non_utf8_text_attachment_download_is_transfer_decoded_without_charset_conversion() {
    let raw=b"Subject: =?ISO-8859-1?Q?caf=E9?=\r\nX-Duplicate: one\r\nX-Duplicate: two\r\nMIME-Version: 1.0\r\nContent-Type: multipart/mixed; boundary=b\r\n\r\n--b\r\nContent-Type: text/plain; charset=iso-8859-1\r\nContent-Transfer-Encoding: quoted-printable\r\nContent-Disposition: attachment; filename=cafe.txt\r\n\r\ncaf=E9\r\n--b--\r\n";
    let decoded = decode_mime(raw, 65536).unwrap();
    assert_eq!(decoded.subject.as_deref(), Some("café"));
    assert_eq!(
        decoded
            .headers
            .iter()
            .filter(|h| h.name.eq_ignore_ascii_case("x-duplicate"))
            .count(),
        2
    );
    assert_eq!(decoded.attachments[0].bytes, b"caf\xe9");
}

#[test]
fn payload_limits_and_unparseable_mime_are_explicit_errors() {
    assert!(matches!(
        decode_mime(b"Subject: too large\r\n\r\nlarge", 8),
        Err(MailError::TooLarge)
    ));
    assert!(matches!(
        decode_mime(b"", 65536),
        Err(MailError::InvalidMime)
    ));
}
