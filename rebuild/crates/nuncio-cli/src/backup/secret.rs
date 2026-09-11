use crate::output::AppError;
use std::io::{IsTerminal, Read};
use zeroize::{Zeroize, ZeroizeOnDrop, Zeroizing};
#[derive(serde::Deserialize, Zeroize, ZeroizeOnDrop)]
#[serde(deny_unknown_fields)]
struct Input {
    passphrase: String,
}
pub(super) fn read() -> Result<nuncio_proto::v2::RecoverySecret, AppError> {
    let stdin = std::io::stdin();
    if stdin.is_terminal() {
        return Err(AppError{code:"recovery_input",message:"Pipe recovery-passphrase JSON from a secure source; terminal input is refused to prevent secret echo",exit:2,operation:None,sync_run:None,recovery:None});
    }
    let mut bytes = Zeroizing::new(Vec::new());
    stdin
        .lock()
        .take(32769)
        .read_to_end(&mut bytes)
        .map_err(|_| AppError::invalid())?;
    decode(&bytes)
}
fn decode(bytes: &[u8]) -> Result<nuncio_proto::v2::RecoverySecret, AppError> {
    if bytes.len() > 32768 {
        return Err(AppError::invalid());
    }
    let mut value: Input = serde_json::from_slice(bytes).map_err(|_| AppError::invalid())?;
    let phrase = &value.passphrase;
    let raw = phrase
        .strip_prefix("x'")
        .or_else(|| phrase.strip_prefix("X'"))
        .and_then(|v| v.strip_suffix('\''))
        .is_some_and(|v| {
            matches!(v.len(), 64 | 96 | 160) && v.bytes().all(|b| b.is_ascii_hexdigit())
        });
    if phrase.len() > 4096
        || phrase.chars().count() < 12
        || phrase.trim().is_empty()
        || phrase.contains('\0')
        || raw
    {
        return Err(AppError::invalid());
    }
    Ok(nuncio_proto::v2::RecoverySecret {
        passphrase: std::mem::take(&mut value.passphrase),
    })
}
#[cfg(test)]
mod tests {
    #[test]
    fn secret_input_rejects_unknown_fields_and_raw_key_literals() {
        assert!(super::decode(br#"{"passphrase":"synthetic secure recovery phrase"}"#).is_ok());
        for invalid in [
            br#"{"passphrase":"short"}"#.as_slice(),
            br#"{"passphrase":"synthetic secure recovery phrase","token":"unexpected"}"#,
            b"not-json",
        ] {
            assert!(super::decode(invalid).is_err());
        }
        for prefix in ["x", "X"] {
            for n in [64, 96, 160] {
                let value = serde_json::json!({"passphrase":format!("{prefix}'{}'","1".repeat(n))});
                assert!(super::decode(value.to_string().as_bytes()).is_err());
            }
        }
    }
}
