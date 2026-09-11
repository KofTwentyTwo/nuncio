#![allow(clippy::unwrap_used)]
use nuncio_engine::domain::drafts::{DraftContent, Recipient};

fn draft() -> DraftContent {
    serde_json::from_value(serde_json::json!({"to":[{"address":"person+tag@example.test","name":"Zoë Person"}],"subject":"Draft","text":"Untrusted body\r\nBcc: this is body text"})).unwrap()
}
#[test]
fn drafts_allow_incomplete_composition_but_reject_header_and_recipient_injection() {
    let valid = draft();
    assert!(valid.validate().is_ok());
    assert!(DraftContent::default().validate().is_ok());
    assert!(Recipient {
        address: "\"quoted local\"@example.test".into(),
        name: None
    }
    .validate()
    .is_ok());
    for address in [
        "person@example.test\r\nBcc: injected@example.test",
        "Name <person@example.test>",
        "one@example.test,two@example.test",
        "no-at-sign",
        "a..b@example.test",
        "person@",
        " person@example.test ",
    ] {
        let mut candidate = draft();
        candidate.to[0].address = address.into();
        assert!(
            candidate.validate().is_err(),
            "accepted invalid recipient {address:?}"
        );
    }
    for subject in [
        "subject\r\nBcc: attacker@example.test",
        "bad\0subject",
        "bad\u{7f}subject",
    ] {
        let mut candidate = draft();
        candidate.subject = subject.into();
        assert!(candidate.validate().is_err());
    }
    let mut candidate = draft();
    candidate.to[0].name = Some("Name\r\nFrom: attacker@example.test".into());
    assert!(candidate.validate().is_err());
    let mut candidate = draft();
    candidate.to = vec![candidate.to[0].clone(); 1001];
    assert!(candidate.validate().is_err());
    let mut candidate = draft();
    candidate.text = Some("x".repeat(1024 * 1024));
    candidate.html = Some("y".into());
    assert!(candidate.validate().is_err());
    assert!(
        serde_json::from_value::<DraftContent>(serde_json::json!({"from":"attacker@example.test"}))
            .is_err(),
        "the sending identity must come from the explicit local account"
    );
}
