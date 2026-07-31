//! Cryptographic payload encryption and `age` streaming attachment cipher.

use aes_gcm::aead::{Aead, KeyInit, OsRng};
use aes_gcm::{Aes256Gcm, Nonce};
use std::io::{Read, Write};
use thiserror::Error;

/// Cryptographic cipher errors.
#[derive(Error, Debug, PartialEq, Eq)]
pub enum CipherError {
    /// Encryption payload processing failure.
    #[error("payload encryption failed: {0}")]
    EncryptionFailed(String),
    /// Decryption payload processing or authentication failure.
    #[error("payload decryption failed: {0}")]
    DecryptionFailed(String),
}

/// Cryptographic cipher manager handling record-level AES-256-GCM and `age` attachment ciphers.
pub struct PayloadCipher;

impl PayloadCipher {
    /// Nonce byte length for AES-256-GCM (12 bytes / 96 bits).
    pub const NONCE_LEN: usize = 12;

    /// Encrypt a byte slice using AES-256-GCM with a 256-bit symmetric key.
    /// The 12-byte random nonce is prepended to the returned ciphertext payload.
    pub fn encrypt_bytes(key: &[u8; 32], plaintext: &[u8]) -> Result<Vec<u8>, CipherError> {
        let cipher = Aes256Gcm::new(key.into());
        let mut nonce_bytes = [0u8; Self::NONCE_LEN];
        use aes_gcm::aead::rand_core::RngCore;
        OsRng.fill_bytes(&mut nonce_bytes);
        let nonce = Nonce::from_slice(&nonce_bytes);

        let mut encrypted = cipher
            .encrypt(nonce, plaintext)
            .map_err(|e| CipherError::EncryptionFailed(e.to_string()))?;

        let mut output = Vec::with_capacity(Self::NONCE_LEN + encrypted.len());
        output.extend_from_slice(&nonce_bytes);
        output.append(&mut encrypted);

        Ok(output)
    }

    /// Decrypt a byte slice created by [`PayloadCipher::encrypt_bytes`].
    pub fn decrypt_bytes(key: &[u8; 32], ciphertext: &[u8]) -> Result<Vec<u8>, CipherError> {
        if ciphertext.len() < Self::NONCE_LEN {
            return Err(CipherError::DecryptionFailed(
                "ciphertext payload too short".to_string(),
            ));
        }

        let (nonce_bytes, payload) = ciphertext.split_at(Self::NONCE_LEN);
        let cipher = Aes256Gcm::new(key.into());
        let nonce = Nonce::from_slice(nonce_bytes);

        cipher
            .decrypt(nonce, payload)
            .map_err(|e| CipherError::DecryptionFailed(e.to_string()))
    }

    /// Encrypt text payload for database column storage at rest using a caller-supplied
    /// AES-256-GCM key. The key MUST be sourced from a [`crate::vault::SecretManager`]-backed
    /// vault (never a compiled-in default) so that at-rest ciphertext cannot be decrypted
    /// without access to the OS keyring.
    pub fn encrypt_text_at_rest(key: &[u8; 32], text: &str) -> Result<String, CipherError> {
        if text.is_empty() {
            return Ok(String::new());
        }
        let encrypted = Self::encrypt_bytes(key, text.as_bytes())?;
        Ok(hex::encode(encrypted))
    }

    /// Decrypt text payload from database column storage at rest using a caller-supplied
    /// AES-256-GCM key. See [`PayloadCipher::encrypt_text_at_rest`].
    ///
    /// Fails closed: a hex-decode error, an AEAD authentication failure (tampered or
    /// corrupted ciphertext), or a non-UTF-8 decrypted payload is returned as an error
    /// rather than silently read back as an empty body -- an empty stored ciphertext is
    /// the only case that legitimately yields `Ok(String::new())`.
    pub fn decrypt_text_at_rest(key: &[u8; 32], stored: &str) -> Result<String, CipherError> {
        if stored.is_empty() {
            return Ok(String::new());
        }
        let bytes = hex::decode(stored)
            .map_err(|e| CipherError::DecryptionFailed(format!("invalid hex encoding: {e}")))?;
        let decrypted = Self::decrypt_bytes(key, &bytes)?;
        String::from_utf8(decrypted)
            .map_err(|e| CipherError::DecryptionFailed(format!("invalid utf-8 payload: {e}")))
    }

    /// Encrypt binary attachment payload using `age` passphrase encryption.
    pub fn encrypt_attachment_stream(
        passphrase: &str,
        input: &[u8],
    ) -> Result<Vec<u8>, CipherError> {
        let encryptor = age::Encryptor::with_user_passphrase(secret_service_passphrase(passphrase));
        let mut encrypted_output = Vec::new();
        let mut writer = encryptor
            .wrap_output(&mut encrypted_output)
            .map_err(|e| CipherError::EncryptionFailed(e.to_string()))?;

        writer
            .write_all(input)
            .map_err(|e| CipherError::EncryptionFailed(e.to_string()))?;

        writer
            .finish()
            .map_err(|e| CipherError::EncryptionFailed(e.to_string()))?;

        Ok(encrypted_output)
    }

    /// Decrypt binary attachment payload encrypted by [`PayloadCipher::encrypt_attachment_stream`].
    pub fn decrypt_attachment_stream(
        passphrase: &str,
        ciphertext: &[u8],
    ) -> Result<Vec<u8>, CipherError> {
        let decryptor = match age::Decryptor::new(ciphertext) {
            Ok(age::Decryptor::Passphrase(d)) => d,
            Ok(_) => {
                return Err(CipherError::DecryptionFailed(
                    "unexpected age header format".to_string(),
                ))
            }
            Err(e) => return Err(CipherError::DecryptionFailed(e.to_string())),
        };

        let mut reader = decryptor
            .decrypt(&secret_service_passphrase(passphrase), None)
            .map_err(|e| CipherError::DecryptionFailed(e.to_string()))?;

        let mut plaintext = Vec::new();
        reader
            .read_to_end(&mut plaintext)
            .map_err(|e| CipherError::DecryptionFailed(e.to_string()))?;

        Ok(plaintext)
    }
}

fn secret_service_passphrase(raw: &str) -> age::secrecy::SecretString {
    age::secrecy::SecretString::new(raw.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn aes_256_gcm_encrypt_decrypt_roundtrip() {
        let key = [42u8; 32];
        let plaintext = b"Nuncio encrypted payload text";

        let ciphertext =
            PayloadCipher::encrypt_bytes(&key, plaintext).expect("encryption succeeds");
        assert_ne!(ciphertext, plaintext);
        assert!(ciphertext.len() > PayloadCipher::NONCE_LEN);

        let decrypted =
            PayloadCipher::decrypt_bytes(&key, &ciphertext).expect("decryption succeeds");
        assert_eq!(decrypted, plaintext);
    }

    #[test]
    fn aes_256_gcm_short_ciphertext_fails() {
        let key = [42u8; 32];
        let short_payload = [0u8; 5];
        let err = PayloadCipher::decrypt_bytes(&key, &short_payload)
            .expect_err("should fail short payload");
        assert_eq!(
            err,
            CipherError::DecryptionFailed("ciphertext payload too short".to_string())
        );
    }

    #[test]
    fn aes_256_gcm_tampered_ciphertext_fails() {
        let key = [42u8; 32];
        let plaintext = b"Sensitive email body payload";
        let mut ciphertext = PayloadCipher::encrypt_bytes(&key, plaintext).unwrap();

        // Corrupt last byte of tag
        let last_idx = ciphertext.len() - 1;
        ciphertext[last_idx] ^= 0xFF;

        let err =
            PayloadCipher::decrypt_bytes(&key, &ciphertext).expect_err("should fail tamper check");
        assert!(matches!(err, CipherError::DecryptionFailed(_)));
    }

    #[test]
    fn age_attachment_stream_encrypt_decrypt_roundtrip() {
        let passphrase = "master_vault_passphrase_99";
        let attachment_data = b"PDF invoice binary attachment data payload";

        let encrypted = PayloadCipher::encrypt_attachment_stream(passphrase, attachment_data)
            .expect("age encryption succeeds");

        let decrypted = PayloadCipher::decrypt_attachment_stream(passphrase, &encrypted)
            .expect("age decryption succeeds");

        assert_eq!(decrypted, attachment_data);
    }

    #[test]
    fn age_attachment_stream_invalid_passphrase_fails() {
        let passphrase = "correct_passphrase";
        let wrong_passphrase = "wrong_passphrase";
        let data = b"Confidential document";

        let encrypted = PayloadCipher::encrypt_attachment_stream(passphrase, data).unwrap();
        let err = PayloadCipher::decrypt_attachment_stream(wrong_passphrase, &encrypted)
            .expect_err("should fail with wrong passphrase");

        assert!(matches!(err, CipherError::DecryptionFailed(_)));
    }

    #[test]
    fn age_attachment_stream_invalid_header_fails() {
        let err = PayloadCipher::decrypt_attachment_stream("pass", b"invalid_age_bytes")
            .expect_err("should fail invalid header");

        assert!(matches!(err, CipherError::DecryptionFailed(_)));
    }

    #[test]
    fn error_display_formatting() {
        let enc_err = CipherError::EncryptionFailed("failed".to_string());
        assert_eq!(enc_err.to_string(), "payload encryption failed: failed");

        let dec_err = CipherError::DecryptionFailed("failed".to_string());
        assert_eq!(dec_err.to_string(), "payload decryption failed: failed");
    }

    #[test]
    fn text_at_rest_encryption_decryption_roundtrip() {
        let key = [7u8; 32];
        let text = "Sensitive Email Body Payload";
        let encrypted =
            PayloadCipher::encrypt_text_at_rest(&key, text).expect("encryption succeeds");
        assert_ne!(text, encrypted);

        let decrypted =
            PayloadCipher::decrypt_text_at_rest(&key, &encrypted).expect("decryption succeeds");
        assert_eq!(text, decrypted);

        assert_eq!(
            PayloadCipher::encrypt_text_at_rest(&key, "").expect("empty text is not encrypted"),
            ""
        );
        assert_eq!(
            PayloadCipher::decrypt_text_at_rest(&key, "").expect("empty ciphertext decrypts"),
            ""
        );
    }

    #[test]
    fn text_at_rest_wrong_key_fails_closed() {
        // Proves there is no compiled-in default key: ciphertext produced under one
        // caller-supplied key cannot be recovered using a different key, and the failure
        // is surfaced as an error rather than a silently-empty body.
        let key_a = [1u8; 32];
        let key_b = [2u8; 32];
        let text = "Confidential ledger payload";

        let encrypted =
            PayloadCipher::encrypt_text_at_rest(&key_a, text).expect("encryption succeeds");
        let err = PayloadCipher::decrypt_text_at_rest(&key_b, &encrypted)
            .expect_err("wrong key must fail closed");

        assert!(matches!(err, CipherError::DecryptionFailed(_)));
    }

    #[test]
    fn text_at_rest_tampered_ciphertext_fails_closed() {
        // A corrupted-but-valid-hex ciphertext column must surface as an error, not read
        // back as an empty body -- this is the read-path twin of the AEAD integrity
        // guarantee: tampering must be detectable, not silently masked.
        let key = [9u8; 32];
        let text = "Do not silently drop this on tamper";
        let encrypted =
            PayloadCipher::encrypt_text_at_rest(&key, text).expect("encryption succeeds");

        let mut bytes = hex::decode(&encrypted).expect("valid hex");
        let last_idx = bytes.len() - 1;
        bytes[last_idx] ^= 0xFF;
        let tampered = hex::encode(bytes);

        let err = PayloadCipher::decrypt_text_at_rest(&key, &tampered)
            .expect_err("tampered ciphertext must fail closed");
        assert!(matches!(err, CipherError::DecryptionFailed(_)));
    }

    #[test]
    fn text_at_rest_invalid_hex_fails_closed() {
        let key = [3u8; 32];
        let err = PayloadCipher::decrypt_text_at_rest(&key, "not-valid-hex")
            .expect_err("invalid hex must fail closed");
        assert!(matches!(err, CipherError::DecryptionFailed(_)));
    }
}
