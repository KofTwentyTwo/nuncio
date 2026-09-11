#![allow(clippy::unwrap_used)]
use nuncio_engine::domain::{
    identity::{AccountId, ImapMailboxId},
    imap::{new_uid_range, uid_batches, ImapFlags, ImapPlacement, MailboxName},
};
use std::num::NonZeroU32;

#[test]
fn mailbox_codec_uses_rfc3501_modified_utf7_without_identity_guessing() {
    for (decoded, wire) in [
        ("~peter/mail/台北/日本語", "~peter/mail/&U,BTFw-/&ZeVnLIqe-"),
        ("R&D", "R&-D"),
        ("A + \\\" folder", "A + \\\" folder"),
        ("收件箱😀", "&ZTZO9nux2D3eAA-"),
    ] {
        let name = MailboxName::from_unicode(decoded).unwrap();
        assert_eq!(name.wire_name(), wire);
        assert_eq!(MailboxName::from_wire(wire).unwrap().as_str(), decoded);
    }
    assert_eq!(MailboxName::from_wire("iNbOx").unwrap().as_str(), "INBOX");
    assert_ne!(
        MailboxName::from_unicode("Sent").unwrap(),
        MailboxName::from_unicode("sent").unwrap()
    );
    for wire in [
        "",
        "&",
        "&Jjo!",
        "&U,BTFw-&ZeVnLIqe-",
        "&AGE-",
        "&2AA-",
        "&AAAA-",
        "Bad\r\nA LOGOUT",
        "台北",
    ] {
        assert!(MailboxName::from_wire(wire).is_err(), "{wire:?}");
    }
}
#[test]
fn placement_identity_changes_with_account_mailbox_epoch_and_uid() {
    let account = AccountId::generate();
    let folder = ImapMailboxId::generate();
    let original = ImapPlacement::new(account, folder, 9001, 7).unwrap();
    let key = original.provider_id().unwrap();
    assert_eq!(
        ImapPlacement::from_provider_id(account, &key).unwrap(),
        original
    );
    for other in [
        ImapPlacement::new(AccountId::generate(), folder, 9001, 7),
        ImapPlacement::new(account, ImapMailboxId::generate(), 9001, 7),
        ImapPlacement::new(account, folder, 9002, 7),
        ImapPlacement::new(account, folder, 9001, 8),
    ] {
        assert_ne!(other.unwrap().provider_id().unwrap(), key);
    }
    assert!(ImapPlacement::from_provider_id(AccountId::generate(), &key).is_err());
    assert!(ImapPlacement::new(account, folder, 0, 7).is_err());
    assert!(ImapPlacement::new(account, folder, 9001, 0).is_err());
    assert!(ImapPlacement::from_provider_id(account, &format!("[{key}]")).is_err());
}
#[test]
fn uid_ranges_never_fetch_empty_sets_or_wrap_the_largest_uid() {
    assert!(uid_batches(&[], 100).unwrap().is_empty());
    let ids = [1, 2, 4, 4, u32::MAX].map(|n| NonZeroU32::new(n).unwrap());
    assert_eq!(uid_batches(&ids, 2).unwrap(), ["1:2", "4,4294967295"]);
    assert_eq!(new_uid_range(7, 8).unwrap(), None);
    assert_eq!(new_uid_range(7, 9).unwrap().as_deref(), Some("8:8"));
    assert_eq!(new_uid_range(0, 1).unwrap(), None);
    assert_eq!(new_uid_range(u32::MAX, u32::MAX).unwrap(), None);
    assert!(new_uid_range(7, 0).is_err());
    assert!(uid_batches(&ids, 0).is_err());
}
#[test]
fn flag_changes_preserve_unknown_keywords_and_reject_command_injection() {
    let flags = ImapFlags::new(vec![
        "\\seen".into(),
        "Project.Blue".into(),
        "\\Flagged".into(),
        "\\Seen".into(),
    ])
    .unwrap();
    assert!(flags.seen() && flags.flagged());
    assert!(flags.values().contains(&"Project.Blue".into()));
    assert_eq!(flags.values().len(), 3);
    for flag in [
        "",
        "\\Seen\r\nA EXPUNGE",
        "keyword name",
        "\\*",
        "{17}",
        "(\\Deleted)",
    ] {
        assert!(ImapFlags::new(vec![flag.into()]).is_err());
    }
}
