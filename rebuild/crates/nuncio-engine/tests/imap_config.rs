use nuncio_engine::domain::imap_account::{ImapAccountConfig, ImapCredentials};
use serde_json::{json, Value};

fn fixture() -> Value {
    json!({
        "address":"alpha@example.test",
        "imap":{"host":"NAS.Example.test.","port":993,"tls":"implicit","username":"alpha"},
        "smtp":{"host":"NAS.Example.test","port":587,"tls":"start_tls","username":"alpha"},
        "sent_policy":"client_append","sent_folder":"Sent",
        "archive_folder":"Archive 日本語","trash_folder":"Trash"
    })
}

#[test]
fn configuration_has_explicit_tls_and_principal_identity_without_passwords(
) -> Result<(), Box<dyn std::error::Error>> {
    let config: ImapAccountConfig = serde_json::from_value(fixture())?;
    let config = config.canonicalized()?;
    assert_eq!(config.imap.host, "nas.example.test");
    assert_eq!(config.smtp.host, "nas.example.test");
    assert_eq!(config.archive_folder.as_deref(), Some("Archive 日本語"));
    let identity = config.identity()?;
    let mut changed = fixture();
    changed["imap"]["host"] = json!("nas.example.test");
    let same: ImapAccountConfig = serde_json::from_value(changed.clone())?;
    assert_eq!(same.canonicalized()?.identity()?, identity);
    changed["imap"]["username"] = json!("Alpha");
    let different: ImapAccountConfig = serde_json::from_value(changed)?;
    assert_ne!(different.canonicalized()?.identity()?, identity);
    let mut leaked = fixture();
    leaked["password"] = json!("SYNTHETIC-CREDENTIAL-CANARY");
    assert!(serde_json::from_value::<ImapAccountConfig>(leaked).is_err());
    for bad in ["plaintext", "opportunistic", "none"] {
        let mut value = fixture();
        value["imap"]["tls"] = json!(bad);
        assert!(serde_json::from_value::<ImapAccountConfig>(value).is_err());
    }
    Ok(())
}

#[test]
fn malformed_endpoints_folders_and_secret_inputs_fail_without_debug_disclosure(
) -> Result<(), Box<dyn std::error::Error>> {
    for host in [
        "",
        "https://nas.example.test",
        "user@nas.example.test",
        "nas..example",
        "nas.example\r\nLOGIN x",
        "999.1.1.1",
    ] {
        let mut value = fixture();
        value["imap"]["host"] = json!(host);
        let config: ImapAccountConfig = serde_json::from_value(value)?;
        assert!(config.canonicalized().is_err());
    }
    for (path, value) in [
        ("/imap/port", json!(0)),
        ("/smtp/username", json!("")),
        ("/address", json!("Display <alpha@example.test>")),
        ("/sent_folder", json!("Sent\r\na2 EXPUNGE")),
        ("/trusted_ca_pem", json!("X".repeat(128 * 1024 + 1))),
    ] {
        let mut input = fixture();
        if path == "/trusted_ca_pem" {
            input["trusted_ca_pem"] = value;
        } else {
            *input.pointer_mut(path).ok_or("missing test field")? = value;
        }
        let config: ImapAccountConfig = serde_json::from_value(input)?;
        assert!(config.canonicalized().is_err());
    }
    let secret = "SYNTHETIC-PASSWORD-DO-NOT-DISPLAY";
    let credentials: ImapCredentials =
        serde_json::from_value(json!({"imap_password":secret,"smtp_password":secret}))?;
    credentials.validate()?;
    assert!(!format!("{credentials:?}").contains(secret));
    let invalid: ImapCredentials =
        serde_json::from_value(json!({"imap_password":"","smtp_password":"abc\u{0}def"}))?;
    assert!(invalid.validate().is_err());
    Ok(())
}
