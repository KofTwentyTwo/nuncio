# Encryption compatibility fixtures

These synthetic payloads were generated independently with the previously locked
`aes-gcm = 0.10.3` and `age = 0.10.1`, before testing the upgraded adapters.
Both decrypt to the ASCII bytes `Nuncio synthetic migration fixture`.

- `aes-gcm-0.10.bin` uses the existing storage format: a twelve-byte nonce
  (`012345678901`) followed by AES-256-GCM ciphertext and tag, with a 32-byte key
  whose every byte is 42. The fixed nonce is test data only.
- `age-0.10.bin` uses age passphrase encryption with the public test passphrase
  `synthetic-test-passphrase-only`.

No credentials or user data are present. Compatibility tests consume these
fixed old-format bytes instead of generating them with the version under test.
The generator and its dependency lock are retained in the local verification
receipt `rebuild/test-results/dependency-security/legacy-fixture-generator/`.
